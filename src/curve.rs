use crate::model::BookLevel;
use crate::sweep::sweep_cost;
use anyhow::{anyhow, Result};
use polars::prelude::*;
use serde::Serialize;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' curves compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// One point of the measured cost curve: what an order of this size actually
/// paid sweeping the book, and what the fitted square-root law says it should
/// have paid.
#[derive(Debug, Serialize)]
pub struct CurvePoint {
    pub order_size: f64,
    /// `order_size` as a fraction of the mean resting depth on the swept side.
    pub participation: f64,
    /// Mean sweep slippage actually paid, in basis points.
    pub measured_bps: f64,
    /// `coef_bps * sqrt(participation)` -- the fitted model's prediction.
    pub modelled_bps: f64,
    /// `measured_bps - modelled_bps`: positive where the book is dearer than
    /// the concave law expects.
    pub residual_bps: f64,
    /// Mean filled fraction across snapshots; below 1 the book was too thin.
    pub fill_ratio: f64,
    /// Whether this point was used to fit the coefficient.
    pub fitted: bool,
}

/// **Impact calibration from the book alone** -- sweep the captured book at a
/// ladder of order sizes and fit the Almgren-Chriss temporary-impact
/// coefficient to the costs the book itself charges.
///
/// [`crate::calibrate`] recovers the impact coefficients from a desk's own
/// realised fills. That is the right answer when you have fills -- but a new
/// venue, a new product, or a pre-trade "what would this cost us here?" question
/// has none. This does the same job with only an L2 capture: each order size in
/// the ladder is swept through the book by [`crate::sweep::sweep_cost`], its
/// cost is expressed against the mean resting depth as a participation rate, and
/// the concave law `measured_bps = coef_bps * sqrt(participation)` is fitted
/// through the origin over those points.
///
/// The fit also *tests* the model rather than assuming it. `r_squared` says how
/// much of the book's own cost shape the square-root law explains, and each
/// point carries the `residual_bps` it misses by -- so a book that charges a
/// different shape shows up as a poor fit and a visible residual pattern instead
/// of being silently averaged away.
///
/// A size the captured book cannot fill has an understated cost -- it paid only
/// for the liquidity that was there -- so such points are reported with their
/// short `fill_ratio` but **excluded from the fit**, never quietly regressed.
#[derive(Debug, Serialize)]
pub struct SweepCurveSummary {
    pub product: String,
    pub side: String,
    pub snapshots: usize,
    /// Mean total resting size on the swept side, the denominator of
    /// `participation`.
    pub avg_depth: f64,
    pub points: usize,
    pub fitted_points: usize,
    /// Fitted temporary-impact coefficient: cost in bps at 100% participation.
    pub coef_bps: f64,
    /// Root-mean-square residual over the fitted points, in bps.
    pub rmse_bps: f64,
    /// Fraction of the measured-cost variance the square-root law explains.
    pub r_squared: f64,
    pub curve: Vec<CurvePoint>,
}

/// Fit the square-root impact law to the book's own sweep costs.
///
/// Mirrors `sweep_curve` in `python/xexeclab/engine.py` operation for operation,
/// including the sort that fixes the summation order, so the two engines recover
/// bit-for-bit identical coefficients.
pub fn sweep_curve(
    levels: &[BookLevel],
    product: &str,
    side: &str,
    sizes: &[f64],
) -> Result<SweepCurveSummary> {
    if levels.is_empty() {
        return Err(anyhow!("no book levels"));
    }
    if sizes.len() < 2 {
        return Err(anyhow!("need at least two order sizes to fit a curve"));
    }
    // The taker crosses the opposite side of the book; `sweep_cost` validates
    // the side too, but the depth denominator below needs it up front.
    let book_side = match side {
        "buy" => "ask",
        "sell" => "bid",
        other => return Err(anyhow!("side must be buy or sell, got {other}")),
    };

    // Validate before sorting: a NaN has no order, so it must be rejected here
    // rather than corrupting the ladder. `sweep_cost` rejects the same set.
    if let Some(bad) = sizes.iter().find(|q| !q.is_finite() || **q <= 0.0) {
        return Err(anyhow!(
            "order sizes must be positive finite numbers, got {bad}"
        ));
    }
    let mut ladder = sizes.to_vec();
    ladder.sort_by(|a, b| a.partial_cmp(b).expect("sizes are validated finite above"));
    for w in ladder.windows(2) {
        if w[0] == w[1] {
            return Err(anyhow!("duplicate order size {}", w[0]));
        }
    }

    // Mean resting size per snapshot on the swept side: the denominator that
    // turns an absolute order size into a participation rate.
    let ts_ns: Vec<i64> = levels.iter().map(|l| l.ts_ns).collect();
    let lside: Vec<String> = levels.iter().map(|l| l.side.clone()).collect();
    let size: Vec<f64> = levels.iter().map(|l| l.size).collect();
    let depth = df!(
        "ts_ns" => ts_ns,
        "side" => lside,
        "size" => size,
    )?
    .lazy()
    .filter(col("side").eq(lit(book_side)))
    .group_by([col("ts_ns")])
    .agg([col("size").sum().alias("depth")])
    .sort_by_exprs([col("ts_ns")], SortMultipleOptions::default())
    .select([
        col("depth").mean().alias("avg_depth"),
        col("depth").count().alias("snapshots"),
    ])
    .collect()?;
    let avg_depth = depth
        .column("avg_depth")?
        .f64()?
        .get(0)
        .ok_or_else(|| anyhow!("no levels on the {book_side} side"))?;
    let snapshots = depth.column("snapshots")?.u32()?.get(0).unwrap_or(0) as usize;

    // Sweep the book once per ladder rung. Each rung is a full walk of every
    // snapshot, so the measured cost is the book's own answer, not a model's.
    let mut order_size = Vec::with_capacity(ladder.len());
    let mut participation = Vec::with_capacity(ladder.len());
    let mut measured_bps = Vec::with_capacity(ladder.len());
    let mut fill_ratio = Vec::with_capacity(ladder.len());
    for q in &ladder {
        let m = sweep_cost(levels, product, side, *q)?;
        order_size.push(*q);
        participation.push(q / avg_depth);
        measured_bps.push(m.avg_slippage_bps);
        fill_ratio.push(m.avg_fill_ratio);
    }

    // Sufficient statistics for the origin-through fit of `y` on the single
    // regressor x = sqrt(participation), over the fully-filled points only. The
    // 1e-9 matches the fill tolerance `sweep_cost` uses.
    let fitted = col("fill_ratio").gt_eq(lit(1.0 - 1e-9));
    let stats = df!(
        "order_size" => order_size.clone(),
        "participation" => participation.clone(),
        "measured_bps" => measured_bps.clone(),
        "fill_ratio" => fill_ratio.clone(),
    )?
    .lazy()
    .sort_by_exprs([col("order_size")], SortMultipleOptions::default())
    .filter(fitted)
    .with_column(col("participation").sqrt().alias("x"))
    .select([
        (col("x") * col("x")).sum().alias("sxx"),
        (col("x") * col("measured_bps")).sum().alias("sxy"),
        (col("measured_bps") * col("measured_bps"))
            .sum()
            .alias("syy"),
        col("measured_bps").sum().alias("sy"),
        col("measured_bps").count().alias("n"),
    ])
    .collect()?;
    let g = |name: &str| -> Result<f64> {
        stats
            .column(name)?
            .f64()?
            .get(0)
            .ok_or_else(|| anyhow!("null {name}"))
    };
    let fitted_points = stats.column("n")?.u32()?.get(0).unwrap_or(0) as usize;
    if fitted_points < 2 {
        return Err(anyhow!(
            "need at least two fully-filled order sizes to fit the curve; the book is too thin for this ladder"
        ));
    }
    let sxx = g("sxx")?;
    let sxy = g("sxy")?;
    let syy = g("syy")?;
    let sy = g("sy")?;
    let n = fitted_points as f64;
    if sxx <= 0.0 {
        return Err(anyhow!(
            "degenerate ladder: every participation rate is zero"
        ));
    }
    let coef = sxy / sxx;

    let ss_res_raw = syy - 2.0 * coef * sxy + coef * coef * sxx;
    let ss_res = if ss_res_raw > 0.0 { ss_res_raw } else { 0.0 };
    let ybar = sy / n;
    let ss_tot = syy - n * ybar * ybar;
    let rmse = (ss_res / n).sqrt();
    let r_squared = if ss_tot == 0.0 {
        0.0
    } else {
        1.0 - ss_res / ss_tot
    };

    let curve = (0..ladder.len())
        .map(|i| {
            let modelled = coef * participation[i].sqrt();
            CurvePoint {
                order_size: order_size[i],
                participation: r8(participation[i]),
                measured_bps: r8(measured_bps[i]),
                modelled_bps: r8(modelled),
                residual_bps: r8(measured_bps[i] - modelled),
                fill_ratio: r8(fill_ratio[i]),
                fitted: fill_ratio[i] >= 1.0 - 1e-9,
            }
        })
        .collect();

    Ok(SweepCurveSummary {
        product: product.to_string(),
        side: side.to_string(),
        snapshots,
        avg_depth: r8(avg_depth),
        points: ladder.len(),
        fitted_points,
        coef_bps: r8(coef),
        rmse_bps: r8(rmse),
        r_squared: r8(r_squared),
        curve,
    })
}
