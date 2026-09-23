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

/// The capped-replay verdict at one point on the cap grid.
#[derive(Debug, Serialize)]
pub struct CapSweepPoint {
    pub cap: f64,
    /// What the pooled plan left unfilled under the forward carry.
    pub forecast_unfilled_qty: f64,
    /// What the naive plan left unfilled under the same carry.
    pub naive_unfilled_qty: f64,
    /// `naive.filled_impact_bps - forecast.filled_impact_bps` at this cap.
    pub improvement_bps: f64,
    /// `forecast_unfilled_qty - naive_unfilled_qty` at this cap.
    pub shortfall_qty: f64,
    pub like_for_like: bool,
    /// The rate at which the two cancel here, or `null` for one of the two
    /// reasons [`crate::pov::CappedGain`] names.
    pub breakeven_bps: Option<f64>,
    /// `shortfall_qty` is non-zero but no non-negative rate reverses the
    /// verdict: the pooled plan is both cheaper per unit filled and missed
    /// less, or dearer and missed more. See [`CapSweepReport`].
    pub dominated: bool,
}

/// **The shape of the breakeven rate, not one number.**
///
/// [`crate::pov::CappedGain::breakeven_bps`] answers "what would a missed unit
/// have to be worth for this verdict to flip?" — but only at the one cap it was
/// run with. The cap is a desk's own risk parameter, not a property of the
/// market, so a threshold quoted at a single cap is quoted at an arbitrary
/// point: a desk that runs at 15% of volume and a desk that runs at 30% are
/// asking the same question about the same session and getting answers that are
/// not comparable. Re-running across a grid of caps is what makes the number
/// readable, exactly as [`crate::sensitivity`] does for the impact coefficient.
///
/// Two things fall out that a single run cannot show.
///
/// 1. **Where the cap stops mattering.** `like_for_like_from_cap` is the
///    smallest grid cap at or above which every point leaves the same quantity
///    unfilled. Above it the pooled and naive plans are directly comparable and
///    `improvement_bps` is the whole story; below it they are not.
/// 2. **Where there is no trade-off to price at all.** `dominated_points`
///    counts the caps at which a shortfall exists and yet no non-negative rate
///    reverses the verdict. This is not a degenerate case to be tidied away: it
///    is a property of the forward carry. A plan that gets more away early both
///    misses less *and* pays less per unit filled, because the extra fill lands
///    in the low-participation slots and raises their weight in the average. So
///    where only a thin tail binds, one plan dominates outright and a rate has
///    nothing to trade off. A genuine trade-off needs the cap to bind in the
///    *even* buckets too, which is what the low end of a grid reaches.
///
/// The three counts partition the grid: every point is `like_for_like`,
/// `dominated`, or has a `breakeven_bps`, and never two of those at once.
///
/// There is deliberately no `shortfall_bps` here. The sweep exists for the
/// caller who has no rate — one who had a rate would read `net_bps` at their own
/// cap and be done — so accepting one would be answering a question nobody
/// holding this report is asking.
///
/// Every point is the `capped` (forward-carry) arm. The `spread` arm fills the
/// whole parent whenever `parent_qty <= cap * exec_volume` whatever shape the
/// forecast has, so under the reshape the shortfall is a property of the session
/// and not of the forecast — sweeping the cap would describe the session, not
/// the choice the sweep is here to inform.
#[derive(Debug, Serialize)]
pub struct CapSweepReport {
    pub product: String,
    pub bucket_ns: i64,
    pub buckets: usize,
    /// History sessions pooled into the forecast.
    pub sessions: usize,
    pub half_life: f64,
    pub parent_qty: f64,
    pub coef_bps: f64,
    pub perm_coef_bps: f64,
    pub points: Vec<CapSweepPoint>,
    /// Points where both plans missed the same quantity.
    pub like_for_like_points: usize,
    /// Points where one plan is better on both counts, so no rate is needed.
    pub dominated_points: usize,
    /// Points where a rate genuinely decides it, and `breakeven_bps` is that rate.
    pub traded_off_points: usize,
    /// The smallest grid cap at or above which every point is `like_for_like`;
    /// `null` when the largest cap in the grid still leaves the plans differing.
    pub like_for_like_from_cap: Option<f64>,
    /// The range the breakeven rate spans over the points that have one; both
    /// `null` when no point does.
    pub breakeven_min_bps: Option<f64>,
    pub breakeven_max_bps: Option<f64>,
}

/// Re-run [`pov_forecast`] across a grid of caps and line up the breakeven rate.
///
/// Mirrors `cap_sweep` in `python/xexeclab/engine.py` operation for operation,
/// so the two engines report bit-for-bit identical sweeps.
#[allow(clippy::too_many_arguments)]
pub fn cap_sweep(
    history: &[Vec<Tick>],
    exec: &[Tick],
    bucket_ns: i64,
    parent_qty: f64,
    cap_grid: &[f64],
    coef_bps: f64,
    perm_coef_bps: f64,
    half_life: f64,
) -> Result<CapSweepReport> {
    // One point is not a sweep, and an unsorted grid would make
    // `like_for_like_from_cap` meaningless -- it reads the grid as an ordering.
    if cap_grid.len() < 2 {
        return Err(anyhow!(
            "cap_grid needs at least 2 points, got {}",
            cap_grid.len()
        ));
    }
    for v in cap_grid {
        if !v.is_finite() || *v <= 0.0 || *v > 1.0 {
            return Err(anyhow!("cap_grid values must be in (0, 1], got {v}"));
        }
    }
    for w in cap_grid.windows(2) {
        if w[1] <= w[0] {
            return Err(anyhow!(
                "cap_grid must be strictly increasing, got {} then {}",
                w[0],
                w[1]
            ));
        }
    }

    let mut points: Vec<CapSweepPoint> = Vec::with_capacity(cap_grid.len());
    let mut product = String::new();
    let mut buckets = 0usize;
    let mut sessions = 0usize;
    for c in cap_grid {
        // No `shortfall_bps`: the sweep is for the caller who has no rate.
        let r = pov_forecast(
            history,
            exec,
            bucket_ns,
            parent_qty,
            *c,
            coef_bps,
            perm_coef_bps,
            half_life,
            None,
        )?;
        let gain = &r.capped_improvement.capped;
        product = r.product.clone();
        buckets = r.buckets;
        sessions = r.sessions;
        points.push(CapSweepPoint {
            cap: r8(*c),
            forecast_unfilled_qty: r.forecast_capped.capped.unfilled_qty,
            naive_unfilled_qty: r.naive_capped.capped.unfilled_qty,
            improvement_bps: gain.improvement_bps,
            shortfall_qty: gain.shortfall_qty,
            like_for_like: gain.like_for_like,
            breakeven_bps: gain.breakeven_bps,
            dominated: !gain.like_for_like && gain.breakeven_bps.is_none(),
        });
    }

    let like_for_like_points = points.iter().filter(|p| p.like_for_like).count();
    let dominated_points = points.iter().filter(|p| p.dominated).count();
    let traded_off_points = points.iter().filter(|p| p.breakeven_bps.is_some()).count();

    // The smallest cap from which the tail of the grid is like-for-like all the
    // way up. Scanning backwards is what makes it a threshold rather than the
    // first of several disconnected stretches.
    let mut like_for_like_from_cap: Option<f64> = None;
    for p in points.iter().rev() {
        if !p.like_for_like {
            break;
        }
        like_for_like_from_cap = Some(p.cap);
    }

    let rates: Vec<f64> = points.iter().filter_map(|p| p.breakeven_bps).collect();
    let (breakeven_min_bps, breakeven_max_bps) = if rates.is_empty() {
        (None, None)
    } else {
        let agg = df!("breakeven_bps" => rates)?
            .lazy()
            .select([
                col("breakeven_bps").min().alias("lo"),
                col("breakeven_bps").max().alias("hi"),
            ])
            .collect()?;
        let g = |name: &str| -> Result<f64> {
            agg.column(name)?
                .f64()?
                .get(0)
                .ok_or_else(|| anyhow!("null {name}"))
        };
        (Some(r8(g("lo")?)), Some(r8(g("hi")?)))
    };

    Ok(CapSweepReport {
        product,
        bucket_ns,
        buckets,
        sessions,
        half_life: r8(half_life),
        parent_qty: r8(parent_qty),
        coef_bps: r8(coef_bps),
        perm_coef_bps: r8(perm_coef_bps),
        points,
        like_for_like_points,
        dominated_points,
        traded_off_points,
        like_for_like_from_cap,
        breakeven_min_bps,
        breakeven_max_bps,
    })
}
