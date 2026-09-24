use crate::model::Tick;
use crate::pov::pov_forecast;
use anyhow::{anyhow, Result};
use polars::prelude::*;
use serde::Serialize;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' sweeps compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// The pooled-versus-naive verdict at one point on the half-life grid.
#[derive(Debug, Serialize)]
pub struct HlSweepPoint {
    pub half_life: f64,
    /// The weight the pooling gave the most recent history session. This is the
    /// half-life in the units a reader can act on: at `1.0` the forecast *is*
    /// the naive plan, and the further below `1.0` it sits the more of the
    /// older sessions the forecast is actually using.
    pub newest_weight: f64,
    /// `naive.filled_impact_bps - forecast.filled_impact_bps` at this half-life.
    pub improvement_bps: f64,
    /// `forecast_unfilled_qty - naive_unfilled_qty` at this half-life.
    pub shortfall_qty: f64,
    pub like_for_like: bool,
    pub breakeven_bps: Option<f64>,
    /// A shortfall exists and yet no non-negative rate reverses the verdict.
    pub dominated: bool,
}

/// **Is the gain from pooling a property of the sessions, or of a number I
/// picked?**
///
/// [`crate::capsweep`] sweeps the cap, which is a parameter the desk *owns*: a
/// desk knows it runs at 15% of volume, it just has no way to compare that
/// answer with another desk's. `half_life` is not like that. Nobody knows how
/// fast a market forgets its own volume profile, nothing in this repo fits it,
/// and it is passed in because it has to be passed in. A verdict quoted at one
/// half-life is therefore quoted at a guess, and the only honest thing to do
/// with a guess is to show what happens when it is wrong.
///
/// The grid runs between two limits the report names rather than implies.
///
/// 1. **A short half-life is the naive plan.** As `half_life` falls the weight
///    collapses onto the most recent session, `newest_weight` goes to one, the
///    pooled forecast becomes the plan it is being compared against, and
///    `improvement_bps` goes to zero. A sweep whose left-hand end is not near
///    zero has not been run short enough to see its own floor.
/// 2. **A long half-life is the flat pool.** As `half_life` grows the weights
///    even out and the forecast converges on the equal-weight pooling that
///    `half_life = 0` computes directly. `flat_improvement_bps` is that limit,
///    run once and reported beside the grid so the reader can see which end of
///    it the grid is approaching.
///
/// Between those two limits the number that matters is `sign_stable`: whether
/// pooling was worth something at *every* half-life on the grid, or whether the
/// grid contains both a half-life at which pooling paid and one at which it
/// cost. A `false` there is not a defect in the sweep — it is the finding. It
/// says the sign of the headline `improvement_bps` was decided by the guess and
/// not by the sessions, and a desk that read one run would have had no way to
/// know.
///
/// `improvement_span_bps` puts a size on that: the width of the range a caller
/// would have landed anywhere inside, purely by choosing differently.
///
/// Like [`crate::capsweep`] this takes no `shortfall_bps` — a caller who holds a
/// rate reads `net_bps` at their own settings — and every point is the `capped`
/// (forward-carry) arm, for the same reason: under the `spread` reshape the
/// shortfall is a property of the session rather than of the forecast, so it
/// would not respond to the parameter being swept.
#[derive(Debug, Serialize)]
pub struct HlSweepReport {
    pub product: String,
    pub bucket_ns: i64,
    pub buckets: usize,
    /// History sessions pooled into the forecast.
    pub sessions: usize,
    pub parent_qty: f64,
    pub cap: f64,
    pub coef_bps: f64,
    pub perm_coef_bps: f64,
    pub points: Vec<HlSweepPoint>,
    /// `improvement_bps` at `half_life = 0` — the equal-weight pool, the limit
    /// the long end of the grid approaches.
    pub flat_improvement_bps: f64,
    pub improvement_min_bps: f64,
    pub improvement_max_bps: f64,
    /// `improvement_max_bps - improvement_min_bps`: how much of the headline
    /// figure the choice of half-life was responsible for.
    pub improvement_span_bps: f64,
    /// False when the grid holds both a half-life at which pooling paid and one
    /// at which it cost. See [`HlSweepReport`].
    pub sign_stable: bool,
    /// The grid half-life with the largest `improvement_bps`, smallest first on
    /// a tie. Reported as the shape of the grid, **not** as a recommendation:
    /// it is fitted on the very session being scored, so trading it would be
    /// choosing the parameter with the answer already in hand.
    pub best_half_life: f64,
    /// The grid half-life with the smallest `improvement_bps`.
    pub worst_half_life: f64,
}

/// Re-run [`pov_forecast`] across a grid of pooling half-lives at a fixed cap.
///
/// Mirrors `hl_sweep` in `python/xexeclab/engine.py` operation for operation, so
/// the two engines report bit-for-bit identical sweeps.
#[allow(clippy::too_many_arguments)]
pub fn hl_sweep(
    history: &[Vec<Tick>],
    exec: &[Tick],
    bucket_ns: i64,
    parent_qty: f64,
    cap: f64,
    hl_grid: &[f64],
    coef_bps: f64,
    perm_coef_bps: f64,
) -> Result<HlSweepReport> {
    // One point is not a sweep, and the grid is read as an ordering: the report
    // describes it as running from the naive end to the flat end.
    if hl_grid.len() < 2 {
        return Err(anyhow!(
            "half_life_grid needs at least 2 points, got {}",
            hl_grid.len()
        ));
    }
    for v in hl_grid {
        // Zero is rejected rather than accepted as "no decay". `pov_forecast`
        // reads `0` as the equal-weight pool, which is the limit the *long* end
        // of this grid approaches -- so admitting it would put the grid's
        // right-hand limit at its left-hand end and invert the ordering the
        // rest of the report depends on. It is reported as
        // `flat_improvement_bps` instead.
        if !v.is_finite() || *v <= 0.0 {
            return Err(anyhow!(
                "half_life_grid values must be positive and finite, got {v} \
                 (0 means no decay and is reported as flat_improvement_bps)"
            ));
        }
    }
    for w in hl_grid.windows(2) {
        if w[1] <= w[0] {
            return Err(anyhow!(
                "half_life_grid must be strictly increasing, got {} then {}",
                w[0],
                w[1]
            ));
        }
    }

    let run = |hl: f64| {
        pov_forecast(
            history,
            exec,
            bucket_ns,
            parent_qty,
            cap,
            coef_bps,
            perm_coef_bps,
            hl,
            // No `shortfall_bps`: the sweep is for the caller who has no rate.
            None,
        )
    };

    let mut points: Vec<HlSweepPoint> = Vec::with_capacity(hl_grid.len());
    let mut product = String::new();
    let mut buckets = 0usize;
    let mut sessions = 0usize;
    for hl in hl_grid {
        let r = run(*hl)?;
        let gain = &r.capped_improvement.capped;
        product = r.product.clone();
        buckets = r.buckets;
        sessions = r.sessions;
        points.push(HlSweepPoint {
            half_life: r8(*hl),
            newest_weight: r8(*r.weights.last().unwrap_or(&0.0)),
            improvement_bps: gain.improvement_bps,
            shortfall_qty: gain.shortfall_qty,
            like_for_like: gain.like_for_like,
            breakeven_bps: gain.breakeven_bps,
            dominated: !gain.like_for_like && gain.breakeven_bps.is_none(),
        });
    }

    // The equal-weight pool, run once. This is the grid's long-half-life limit,
    // not a grid point, so it is reported beside the points and not among them.
    let flat_improvement_bps = run(0.0)?.capped_improvement.capped.improvement_bps;

    let gains: Vec<f64> = points.iter().map(|p| p.improvement_bps).collect();
    let agg = df!("improvement_bps" => gains.clone())?
        .lazy()
        .select([
            col("improvement_bps").min().alias("lo"),
            col("improvement_bps").max().alias("hi"),
        ])
        .collect()?;
    let g = |name: &str| -> Result<f64> {
        agg.column(name)?
            .f64()?
            .get(0)
            .ok_or_else(|| anyhow!("null {name}"))
    };
    let improvement_min_bps = r8(g("lo")?);
    let improvement_max_bps = r8(g("hi")?);

    // A zero does not flip a verdict, so only a strict sign on both sides counts
    // as unstable.
    let sign_stable = !(gains.iter().any(|v| *v > 0.0) && gains.iter().any(|v| *v < 0.0));

    // Smallest half-life first on a tie: the grid is increasing, so scanning
    // forward with a strict comparison keeps the earliest of equal points.
    let pick = |want: f64| -> f64 {
        points
            .iter()
            .find(|p| p.improvement_bps == want)
            .map(|p| p.half_life)
            .unwrap_or(0.0)
    };

    Ok(HlSweepReport {
        product,
        bucket_ns,
        buckets,
        sessions,
        parent_qty: r8(parent_qty),
        cap: r8(cap),
        coef_bps: r8(coef_bps),
        perm_coef_bps: r8(perm_coef_bps),
        best_half_life: pick(improvement_max_bps),
        worst_half_life: pick(improvement_min_bps),
        points,
        flat_improvement_bps,
        improvement_min_bps,
        improvement_max_bps,
        improvement_span_bps: r8(improvement_max_bps - improvement_min_bps),
        sign_stable,
    })
}
