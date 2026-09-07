use crate::counterfactual::counterfactual;
use crate::model::Fill;
use anyhow::{anyhow, Result};
use polars::prelude::*;
use serde::Serialize;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' sensitivities compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// The counterfactual verdict at one point on the coefficient grid.
#[derive(Debug, Serialize)]
pub struct SensitivityPoint {
    pub coef_bps: f64,
    pub realised_cost_bps: f64,
    /// Which benchmark wins at this coefficient.
    pub best_alternative: String,
    pub best_cost_bps: f64,
    /// `best_cost_bps - realised_cost_bps`; positive means the realised
    /// schedule wins at this coefficient.
    pub edge_bps: f64,
}

/// **Coefficient sensitivity** -- does the counterfactual verdict survive the
/// coefficient it was priced with?
///
/// [`crate::counterfactual`] reports `edge_bps` as though the temporary-impact
/// coefficient were known. It is not: it is fitted, by [`crate::calibrate`] or
/// [`crate::curve`], from noisy data. A desk told "your schedule lost 0.8bps to
/// volume-following" should be able to ask whether that verdict is a property of
/// the execution or of the number that was fed in.
///
/// So the same comparison is re-run across a grid of `coef_bps` and the answers
/// are lined up. `verdict_stable` is the headline: it is true only when every
/// point in the grid picks the same best alternative *and* agrees on the sign of
/// the edge. When the sign does flip, `breakeven_coef_bps` is the coefficient at
/// which it does -- and because the edge is affine in `coef_bps` (drift does not
/// depend on it, and both impact terms are linear in their coefficients),
/// interpolating between the two bracketing grid points is exact rather than
/// approximate.
///
/// `perm_coef_bps` is held fixed: this sweeps one axis, not the plane.
#[derive(Debug, Serialize)]
pub struct SensitivityReport {
    pub product: String,
    pub side: String,
    pub intervals: usize,
    pub arrival_price: f64,
    pub perm_coef_bps: f64,
    pub points: Vec<SensitivityPoint>,
    pub edge_min_bps: f64,
    pub edge_max_bps: f64,
    /// How many adjacent pairs of grid points disagree on the sign of the edge.
    pub sign_flips: usize,
    pub verdict_stable: bool,
    /// The coefficient at which the edge first crosses zero, if it does.
    pub breakeven_coef_bps: Option<f64>,
}

/// Re-run the counterfactual comparison across a grid of impact coefficients.
///
/// Mirrors `sensitivity` in `python/xexeclab/engine.py` operation for operation,
/// so the two engines report bit-for-bit identical sensitivities.
pub fn sensitivity(
    fills: &[Fill],
    product: &str,
    arrival_price: f64,
    coef_grid: &[f64],
    perm_coef_bps: f64,
) -> Result<SensitivityReport> {
    // One point is not a sensitivity, and an unsorted grid would make the
    // bracketing interpolation meaningless.
    if coef_grid.len() < 2 {
        return Err(anyhow!(
            "coef_grid needs at least 2 points, got {}",
            coef_grid.len()
        ));
    }
    for v in coef_grid {
        if !v.is_finite() || *v < 0.0 {
            return Err(anyhow!(
                "coef_grid values must be non-negative and finite, got {v}"
            ));
        }
    }
    for w in coef_grid.windows(2) {
        if w[1] <= w[0] {
            return Err(anyhow!(
                "coef_grid must be strictly increasing, got {} then {}",
                w[0],
                w[1]
            ));
        }
    }

    let mut points: Vec<SensitivityPoint> = Vec::new();
    let mut side = String::new();
    let mut intervals = 0usize;
    for c in coef_grid {
        let r = counterfactual(fills, product, arrival_price, *c, perm_coef_bps)?;
        let best_cost_bps = r
            .alternatives
            .iter()
            .map(|a| a.cost_bps)
            .fold(f64::INFINITY, f64::min);
        side = r.side.clone();
        intervals = r.intervals;
        points.push(SensitivityPoint {
            coef_bps: r8(*c),
            realised_cost_bps: r.realised.cost_bps,
            best_alternative: r.best_alternative,
            best_cost_bps,
            edge_bps: r.edge_bps,
        });
    }

    let grid = df!(
        "coef_bps" => points.iter().map(|p| p.coef_bps).collect::<Vec<f64>>(),
        "edge_bps" => points.iter().map(|p| p.edge_bps).collect::<Vec<f64>>(),
    )?;
    let agg = grid
        .lazy()
        .select([
            col("edge_bps").min().alias("edge_min"),
            col("edge_bps").max().alias("edge_max"),
        ])
        .collect()?;
    let g = |name: &str| -> Result<f64> {
        agg.column(name)?
            .f64()?
            .get(0)
            .ok_or_else(|| anyhow!("null {name}"))
    };
    let edge_min = g("edge_min")?;
    let edge_max = g("edge_max")?;

    // A sign flip between adjacent points is where the verdict changes hands.
    // An edge of exactly zero is a tie, and a tie is not a stable verdict either.
    let mut sign_flips = 0usize;
    let mut breakeven: Option<f64> = None;
    for w in points.windows(2) {
        if w[0].edge_bps * w[1].edge_bps < 0.0 {
            sign_flips += 1;
            if breakeven.is_none() {
                let span = w[1].coef_bps - w[0].coef_bps;
                let frac = -w[0].edge_bps / (w[1].edge_bps - w[0].edge_bps);
                breakeven = Some(w[0].coef_bps + span * frac);
            }
        }
    }
    if breakeven.is_none() {
        breakeven = points
            .iter()
            .find(|p| p.edge_bps == 0.0)
            .map(|p| p.coef_bps);
    }

    let first = points[0].best_alternative.clone();
    let one_winner = points.iter().all(|p| p.best_alternative == first);
    let verdict_stable = one_winner && (edge_min > 0.0 || edge_max < 0.0);

    Ok(SensitivityReport {
        product: product.to_string(),
        side,
        intervals,
        arrival_price: r8(arrival_price),
        perm_coef_bps: r8(perm_coef_bps),
        points,
        edge_min_bps: r8(edge_min),
        edge_max_bps: r8(edge_max),
        sign_flips,
        verdict_stable,
        breakeven_coef_bps: breakeven.map(r8),
    })
}
