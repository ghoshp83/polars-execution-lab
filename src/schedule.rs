use anyhow::{anyhow, Result};
use polars::prelude::*;
use serde::Serialize;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' schedules compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// The urgency grid the optimiser searches: 0.0 to 4.0 in steps of 0.1, so
/// index 0 is exactly a TWAP. Fixed rather than adaptive because the chosen
/// point must be identical in both engines.
const URGENCY_STEPS: usize = 41;

/// One slice of a planned execution schedule.
#[derive(Debug, Serialize)]
pub struct ScheduleSlice {
    pub slice: i64,
    /// Fraction of the parent order this slice trades.
    pub weight: f64,
    /// Absolute size of the child order.
    pub size: f64,
    /// `size` as a fraction of the volume available in the interval.
    pub participation: f64,
    /// Temporary (square-root) impact this slice contributes to the parent's
    /// cost, in basis points of the parent notional.
    pub temp_bps: f64,
    /// Permanent (linear) impact this slice contributes, same units.
    pub perm_bps: f64,
    /// Fraction of the parent still unexecuted after this slice -- the exposure
    /// the timing-risk term prices.
    pub remaining: f64,
}

/// **The schedule the calibrated book implies** -- the trade-off between paying
/// impact and carrying risk, solved over a front-loading grid.
///
/// The rest of this crate measures ([`crate::sweep`]), calibrates
/// ([`crate::calibrate`], [`crate::curve`]) and prices ([`crate::impact`]) a
/// schedule someone else chose. This one *chooses* it. Trading the parent order
/// fast concentrates size into few intervals and pays more impact; trading it
/// slowly leaves inventory exposed to the mid wandering away. Almgren-Chriss is
/// the statement that those two costs trade off and the total has a minimum.
///
/// A candidate schedule is an exponential front-load with urgency `k`: slice
/// `i` of `n` gets weight proportional to `exp(-k * i / n)`, normalised to sum
/// to one. `k = 0` is exactly a TWAP, and larger `k` trades earlier. Each
/// candidate is priced with the *same* two-term model the rest of the repo
/// uses -- temporary `coef_bps * sqrt(participation)` and permanent
/// `perm_coef_bps * participation`, each weighted by the fraction of the parent
/// that slice trades so the totals are in basis points of the parent -- plus a
/// timing-risk term `sigma_bps * sqrt(mean(remaining^2))`, the standard
/// Almgren-Chriss variance of the unexecuted inventory. The urgency that
/// minimises `impact_bps + risk_bps` wins, and the summary reports the TWAP it
/// was measured against and the `saving_bps` between them.
///
/// The classic closed form assumes *linear* temporary impact, which this repo's
/// measured cost curve contradicts. Rather than import a solution derived under
/// an assumption the book does not honour, the trajectory is searched over the
/// grid under the concave law the book actually charges.
#[derive(Debug, Serialize)]
pub struct ScheduleSummary {
    pub product: String,
    pub slices: usize,
    pub total_size: f64,
    /// Volume available to trade against in one interval; the denominator of
    /// every slice's `participation`.
    pub per_slice_volume: f64,
    pub coef_bps: f64,
    pub perm_coef_bps: f64,
    /// Mid-price volatility over one interval, in basis points.
    pub sigma_bps: f64,
    /// The chosen front-loading rate; 0.0 means a TWAP was optimal.
    pub urgency: f64,
    pub impact_bps: f64,
    pub risk_bps: f64,
    pub total_bps: f64,
    pub twap_impact_bps: f64,
    pub twap_risk_bps: f64,
    pub twap_total_bps: f64,
    /// `twap_total_bps - total_bps`: what the front-load bought. Never
    /// negative, because the TWAP is itself a candidate.
    pub saving_bps: f64,
    pub schedule: Vec<ScheduleSlice>,
}

/// One priced candidate trajectory.
struct Plan {
    weight: Vec<f64>,
    size: Vec<f64>,
    participation: Vec<f64>,
    temp_bps: Vec<f64>,
    perm_bps: Vec<f64>,
    remaining: Vec<f64>,
    impact_bps: f64,
    risk_bps: f64,
    total_bps: f64,
    max_participation: f64,
}

/// Build and price the exponential-front-load trajectory for one urgency.
///
/// Mirrors `_plan_schedule` in `python/xexeclab/engine.py` operation for
/// operation, including the sort that fixes the summation order.
fn plan(
    slices: usize,
    total_size: f64,
    per_slice_volume: f64,
    coef_bps: f64,
    perm_coef_bps: f64,
    sigma_bps: f64,
    urgency: f64,
) -> Result<Plan> {
    let n = slices as f64;
    let idx: Vec<i64> = (0..slices as i64).collect();
    let frame = df!("slice" => idx)?
        .lazy()
        .sort_by_exprs([col("slice")], SortMultipleOptions::default())
        .with_column(
            (lit(-urgency) * col("slice").cast(DataType::Float64) / lit(n))
                .exp()
                .alias("raw"),
        )
        .with_column((col("raw") / col("raw").sum()).alias("weight"))
        .with_column((col("weight") * lit(total_size)).alias("size"))
        .with_column((col("size") / lit(per_slice_volume)).alias("participation"))
        .with_columns([
            (col("participation").sqrt() * lit(coef_bps) * col("weight")).alias("temp_bps"),
            (col("participation") * lit(perm_coef_bps) * col("weight")).alias("perm_bps"),
            (lit(1.0) - col("weight").cum_sum(false)).alias("remaining"),
        ])
        .collect()?;

    let totals = frame
        .clone()
        .lazy()
        .select([
            col("temp_bps").sum().alias("temp"),
            col("perm_bps").sum().alias("perm"),
            (col("remaining") * col("remaining")).sum().alias("var"),
            col("participation").max().alias("max_participation"),
        ])
        .collect()?;

    let take = |frame: &DataFrame, name: &str| -> Result<Vec<f64>> {
        Ok(frame
            .column(name)?
            .f64()?
            .into_no_null_iter()
            .collect::<Vec<f64>>())
    };
    let scalar = |name: &str| -> Result<f64> {
        totals
            .column(name)?
            .f64()?
            .get(0)
            .ok_or_else(|| anyhow!("null {name}"))
    };

    let impact_bps = scalar("temp")? + scalar("perm")?;
    let risk_bps = sigma_bps * (scalar("var")? / n).sqrt();
    Ok(Plan {
        weight: take(&frame, "weight")?,
        size: take(&frame, "size")?,
        participation: take(&frame, "participation")?,
        temp_bps: take(&frame, "temp_bps")?,
        perm_bps: take(&frame, "perm_bps")?,
        remaining: take(&frame, "remaining")?,
        impact_bps,
        risk_bps,
        total_bps: impact_bps + risk_bps,
        max_participation: scalar("max_participation")?,
    })
}

/// Search the urgency grid for the cheapest execution trajectory.
///
/// Mirrors `optimal_schedule` in `python/xexeclab/engine.py`, including the
/// rounding of the objective and the first-wins tie-break, so both engines pick
/// bit-for-bit the same schedule.
pub fn optimal_schedule(
    product: &str,
    slices: usize,
    total_size: f64,
    per_slice_volume: f64,
    coef_bps: f64,
    perm_coef_bps: f64,
    sigma_bps: f64,
) -> Result<ScheduleSummary> {
    if slices == 0 {
        return Err(anyhow!("need at least one slice"));
    }
    for (name, v) in [
        ("total_size", total_size),
        ("per_slice_volume", per_slice_volume),
    ] {
        if !v.is_finite() || v <= 0.0 {
            return Err(anyhow!("{name} must be a positive finite number, got {v}"));
        }
    }
    for (name, v) in [
        ("coef_bps", coef_bps),
        ("perm_coef_bps", perm_coef_bps),
        ("sigma_bps", sigma_bps),
    ] {
        if !v.is_finite() || v < 0.0 {
            return Err(anyhow!(
                "{name} must be a non-negative finite number, got {v}"
            ));
        }
    }

    let twap = plan(
        slices,
        total_size,
        per_slice_volume,
        coef_bps,
        perm_coef_bps,
        sigma_bps,
        0.0,
    )?;
    // A schedule that cannot be traded is not a cheap schedule. The TWAP is the
    // flattest candidate on the grid, so if even it overruns the interval's
    // volume no urgency can fit and the caller must reslice.
    if twap.max_participation > 1.0 {
        return Err(anyhow!(
            "a uniform schedule takes {:.4} of the volume available per slice; use more slices or a smaller order",
            twap.max_participation
        ));
    }

    // First-wins on a tie over an ascending grid, so the least urgent schedule
    // is preferred when two are priced the same to 8dp.
    let mut best = twap;
    let mut best_urgency = 0.0_f64;
    let mut best_score = r8(best.total_bps);
    for step in 1..URGENCY_STEPS {
        let urgency = step as f64 / 10.0;
        let cand = plan(
            slices,
            total_size,
            per_slice_volume,
            coef_bps,
            perm_coef_bps,
            sigma_bps,
            urgency,
        )?;
        // Front-loading past what the interval can absorb is infeasible, not
        // free: such candidates are skipped rather than silently clipped.
        if cand.max_participation > 1.0 {
            continue;
        }
        let score = r8(cand.total_bps);
        if score < best_score {
            best_score = score;
            best_urgency = urgency;
            best = cand;
        }
    }

    // Re-price the TWAP for the comparison fields; `best` may have replaced it.
    let twap = plan(
        slices,
        total_size,
        per_slice_volume,
        coef_bps,
        perm_coef_bps,
        sigma_bps,
        0.0,
    )?;

    let schedule = (0..slices)
        .map(|i| ScheduleSlice {
            slice: i as i64,
            weight: r8(best.weight[i]),
            size: r8(best.size[i]),
            participation: r8(best.participation[i]),
            temp_bps: r8(best.temp_bps[i]),
            perm_bps: r8(best.perm_bps[i]),
            remaining: r8(best.remaining[i]),
        })
        .collect();

    Ok(ScheduleSummary {
        product: product.to_string(),
        slices,
        total_size: r8(total_size),
        per_slice_volume: r8(per_slice_volume),
        coef_bps: r8(coef_bps),
        perm_coef_bps: r8(perm_coef_bps),
        sigma_bps: r8(sigma_bps),
        urgency: r8(best_urgency),
        impact_bps: r8(best.impact_bps),
        risk_bps: r8(best.risk_bps),
        total_bps: r8(best.total_bps),
        twap_impact_bps: r8(twap.impact_bps),
        twap_risk_bps: r8(twap.risk_bps),
        twap_total_bps: r8(twap.total_bps),
        saving_bps: r8(twap.total_bps - best.total_bps),
        schedule,
    })
}
