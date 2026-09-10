use crate::model::Tick;
use anyhow::{anyhow, Result};
use polars::prelude::*;
use serde::Serialize;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' plans compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// One bucket of a participation-of-volume plan.
#[derive(Debug, Serialize)]
pub struct PovSlice {
    pub bucket_ns: i64,
    /// Volume the market traded in this bucket.
    pub volume: f64,
    /// This bucket's share of the capture's total volume; also the fraction of
    /// the parent order allocated to it.
    pub volume_share: f64,
    /// VWAP the market itself printed in this bucket.
    pub vwap: f64,
    /// Absolute size of the child order.
    pub size: f64,
    /// `size` as a fraction of the volume traded in this bucket.
    pub participation: f64,
    pub temp_bps: f64,
    pub perm_bps: f64,
}

/// **The schedule the capture's own volume implies.**
///
/// [`crate::schedule`] chooses a trajectory against a *parameter*:
/// `per_slice_volume`, one number assumed to hold for every interval. Real
/// volume is not flat, and a schedule that ignores that pays for it — trading a
/// clock-uniform slice into a thin interval is a large share of a small market.
/// This plan is derived from measured volume instead: the capture is bucketed,
/// and the parent order is allocated to each bucket in proportion to the volume
/// that bucket actually traded.
///
/// Two consequences follow from that allocation, and both are reported rather
/// than assumed:
///
/// 1. **Participation is constant by construction.** Allocating
///    `parent_qty * volume_i / total_volume` to a bucket whose volume is
///    `volume_i` gives `parent_qty / total_volume` in every bucket, whatever the
///    volume profile looks like. So the participation cap is a single scalar
///    check, not a per-bucket clamp — there is nothing to redistribute.
/// 2. **It tracks the session VWAP exactly.** The plan's achieved price is
///    `sum(share_i * vwap_i)`, which is the volume-weighted mean of the bucket
///    VWAPs — the session VWAP itself. `pov_tracking_bps` is therefore zero up
///    to rounding, and it is reported so that a regression shows up as a number
///    rather than as silence.
///
/// The clock-uniform allocation of the same quantity over the same buckets is
/// priced alongside it as the benchmark, with the same two-term impact law the
/// rest of the repo uses. That one has *neither* property: its participation
/// varies bucket by bucket, and its price is the unweighted mean of the bucket
/// VWAPs, so `twap_tracking_bps` is generally non-zero.
#[derive(Debug, Serialize)]
pub struct PovPlan {
    pub product: String,
    pub bucket_ns: i64,
    pub buckets: usize,
    pub parent_qty: f64,
    /// The largest share of any bucket's volume the plan is allowed to take.
    pub cap: f64,
    pub total_volume: f64,
    /// `parent_qty / total_volume` — the same in every bucket, see above.
    pub participation: f64,
    pub coef_bps: f64,
    pub perm_coef_bps: f64,
    pub session_vwap: f64,
    pub pov_price: f64,
    /// Distance from the session VWAP in basis points; zero by construction.
    pub pov_tracking_bps: f64,
    pub pov_impact_bps: f64,
    pub twap_price: f64,
    pub twap_tracking_bps: f64,
    pub twap_impact_bps: f64,
    /// The worst bucket the clock-uniform allocation runs into.
    pub twap_max_participation: f64,
    /// Whether the benchmark itself respects the cap. When false its impact is
    /// an extrapolation of the square-root law past full participation, and is
    /// reported for comparison only.
    pub twap_feasible: bool,
    /// `twap_impact_bps - pov_impact_bps`: what following the volume bought.
    pub edge_bps: f64,
    pub schedule: Vec<PovSlice>,
}

/// One bucket of a volume plan replayed against a capture it was not built on.
#[derive(Debug, Serialize)]
pub struct BacktestSlice {
    /// Bucket position counted from the first bucket of each capture.
    pub slot: i64,
    /// The fraction of the parent the plan capture assigned to this slot.
    pub plan_share: f64,
    /// The fraction of the execution capture's volume that traded in it.
    pub exec_share: f64,
    pub exec_volume: f64,
    /// VWAP the execution capture printed in this slot.
    pub vwap: f64,
    pub size: f64,
    /// `size` against the volume that actually traded — no longer constant.
    pub participation: f64,
    pub temp_bps: f64,
    pub perm_bps: f64,
}

/// **A volume plan, judged out of sample.**
///
/// [`pov_schedule`] allocates against the volume a capture *already* traded, so
/// both of its properties — constant participation and exact VWAP tracking —
/// hold only in the capture the plan was built from. Used as a forward
/// schedule, the plan meets a different session. This replays the allocation
/// derived from one capture (the plan) against another (the execution), aligned
/// by time into the session, and reports what the forecast error did.
///
/// The comparison point is the **oracle**: the volume-following plan built on
/// the execution capture itself, as though its volume had been known in
/// advance. Under the two-term law the impact per unit of parent is
/// `sum(share_i * (coef * sqrt(p_i) + perm * p_i))`, and minimising either term
/// subject to the shares summing to one gives `p_i` equal in every bucket —
/// the proportional allocation. So the oracle is the cheapest allocation this
/// cost model admits, and `forecast_cost_bps` (`impact_bps - oracle_impact_bps`)
/// is never negative: a mis-forecast can only cost, never help. It is reported
/// rather than assumed, so a regression shows up as a negative number.
#[derive(Debug, Serialize)]
pub struct PovBacktest {
    pub product: String,
    pub bucket_ns: i64,
    pub buckets: usize,
    pub parent_qty: f64,
    pub cap: f64,
    pub coef_bps: f64,
    pub perm_coef_bps: f64,
    pub plan_volume: f64,
    pub exec_volume: f64,
    /// Total-variation distance between the two volume profiles: half the sum
    /// of absolute share differences, `0` for identical shapes, `1` for
    /// profiles with no bucket in common.
    pub profile_distance: f64,
    /// Session VWAP of the execution capture.
    pub session_vwap: f64,
    pub price: f64,
    /// Zero in sample; out of sample, what the profile error did to tracking.
    pub tracking_bps: f64,
    pub impact_bps: f64,
    pub max_participation: f64,
    /// Whether every bucket stayed inside the cap once the real volume arrived.
    pub feasible: bool,
    pub oracle_participation: f64,
    pub oracle_impact_bps: f64,
    pub oracle_feasible: bool,
    /// `impact_bps - oracle_impact_bps`: the price of not knowing the volume.
    pub forecast_cost_bps: f64,
    pub schedule: Vec<BacktestSlice>,
}

/// Per-bucket volume and VWAP, ascending by bucket, reduced with Polars.
///
/// The ticks are sorted first, exactly as [`crate::execution::bars`] does, so
/// the within-bucket summation order does not depend on the order the capture
/// happened to arrive in.
fn volume_profile(ticks: &[Tick], bucket_ns: i64) -> Result<DataFrame> {
    let mut sorted: Vec<&Tick> = ticks.iter().collect();
    sorted.sort_by_key(|t| t.ts_ns);
    let bucket: Vec<i64> = sorted
        .iter()
        .map(|t| (t.ts_ns / bucket_ns) * bucket_ns)
        .collect();
    let price: Vec<f64> = sorted.iter().map(|t| t.price).collect();
    let size: Vec<f64> = sorted.iter().map(|t| t.size).collect();

    Ok(df!(
        "bucket_ns" => bucket,
        "price" => price,
        "size" => size,
    )?
    .lazy()
    .group_by([col("bucket_ns")])
    .agg([
        col("size").sum().alias("volume"),
        (col("price") * col("size")).sum().alias("pv"),
    ])
    .sort_by_exprs([col("bucket_ns")], SortMultipleOptions::default())
    .with_column((col("pv") / col("volume")).alias("vwap"))
    .collect()?)
}

/// The argument checks every volume plan shares. Error wordings match the
/// Python engine exactly.
fn validate(
    bucket_ns: i64,
    parent_qty: f64,
    cap: f64,
    coef_bps: f64,
    perm_coef_bps: f64,
) -> Result<()> {
    if bucket_ns < 1 {
        return Err(anyhow!("bucket_ns must be >= 1, got {bucket_ns}"));
    }
    if !parent_qty.is_finite() || parent_qty <= 0.0 {
        return Err(anyhow!(
            "parent_qty must be a positive finite number, got {parent_qty}"
        ));
    }
    if !cap.is_finite() || cap <= 0.0 || cap > 1.0 {
        return Err(anyhow!("cap must be in (0, 1], got {cap}"));
    }
    for (name, v) in [("coef_bps", coef_bps), ("perm_coef_bps", perm_coef_bps)] {
        if !v.is_finite() || v < 0.0 {
            return Err(anyhow!(
                "{name} must be a non-negative finite number, got {v}"
            ));
        }
    }
    Ok(())
}

/// A capture's volume profile, its per-bucket volumes and their total —
/// refusing a capture no plan can be priced on.
fn measured(ticks: &[Tick], bucket_ns: i64) -> Result<(DataFrame, Vec<f64>, f64)> {
    if ticks.is_empty() {
        return Err(anyhow!("no ticks"));
    }
    let profile = volume_profile(ticks, bucket_ns)?;
    let volume: Vec<f64> = profile
        .column("volume")?
        .f64()?
        .into_no_null_iter()
        .collect();
    let total_volume: f64 = volume.iter().sum();
    if total_volume <= 0.0 {
        return Err(anyhow!("zero traded volume"));
    }
    // A bucket that traded nothing has no VWAP to execute at, so the plan
    // cannot be priced there. Buckets only exist where trades landed, so this
    // means a bucket whose trades all had zero size.
    if let Some(i) = volume.iter().position(|v| *v <= 0.0) {
        let bkt = profile.column("bucket_ns")?.i64()?.get(i);
        return Err(anyhow!(
            "bucket {} traded no volume and cannot be priced",
            bkt.unwrap_or_default()
        ));
    }
    Ok((profile, volume, total_volume))
}

/// Bucket positions counted from the capture's first bucket, so captures from
/// different sessions line up by time into the session, not by wall clock.
fn slots(profile: &DataFrame, bucket_ns: i64) -> Result<Vec<i64>> {
    let bucket: Vec<i64> = profile
        .column("bucket_ns")?
        .i64()?
        .into_no_null_iter()
        .collect();
    Ok(bucket.iter().map(|b| (b - bucket[0]) / bucket_ns).collect())
}

fn column_f64(df: &DataFrame, name: &str) -> Result<Vec<f64>> {
    Ok(df.column(name)?.f64()?.into_no_null_iter().collect())
}

fn scalar_f64(df: &DataFrame, name: &str) -> Result<f64> {
    df.column(name)?
        .f64()?
        .get(0)
        .ok_or_else(|| anyhow!("null {name}"))
}

/// Plan a participation-of-volume execution against a measured capture.
///
/// Mirrors `pov_schedule` in `python/xexeclab/engine.py` operation for
/// operation, including the bucket ordering that fixes the summation order.
pub fn pov_schedule(
    ticks: &[Tick],
    product: &str,
    bucket_ns: i64,
    parent_qty: f64,
    cap: f64,
    coef_bps: f64,
    perm_coef_bps: f64,
) -> Result<PovPlan> {
    validate(bucket_ns, parent_qty, cap, coef_bps, perm_coef_bps)?;
    let (profile, volume, total_volume) = measured(ticks, bucket_ns)?;

    let participation = parent_qty / total_volume;
    if participation > cap {
        return Err(anyhow!(
            "a volume-proportional schedule takes {participation:.4} of the traded volume, above the cap {cap:.4}; use a smaller order or raise the cap"
        ));
    }

    let n = profile.height();
    let priced = profile
        .lazy()
        .with_column((col("volume") / lit(total_volume)).alias("share"))
        .with_columns([
            (col("share") * lit(parent_qty)).alias("size"),
            (lit(parent_qty) / lit(n as f64) / col("volume")).alias("twap_participation"),
        ])
        .with_columns([
            (col("share") * lit(coef_bps * participation.sqrt())).alias("temp_bps"),
            (col("share") * lit(perm_coef_bps * participation)).alias("perm_bps"),
            (col("share") * col("vwap")).alias("pov_pv"),
            (col("vwap") / lit(n as f64)).alias("twap_pv"),
        ])
        .with_columns([
            (col("twap_participation").sqrt() * lit(coef_bps / n as f64)).alias("twap_temp_bps"),
            (col("twap_participation") * lit(perm_coef_bps / n as f64)).alias("twap_perm_bps"),
        ])
        .collect()?;

    let totals = priced
        .clone()
        .lazy()
        .select([
            col("temp_bps").sum().alias("temp"),
            col("perm_bps").sum().alias("perm"),
            col("pov_pv").sum().alias("pov_price"),
            col("twap_pv").sum().alias("twap_price"),
            col("twap_temp_bps").sum().alias("twap_temp"),
            col("twap_perm_bps").sum().alias("twap_perm"),
            col("twap_participation").max().alias("twap_max"),
        ])
        .collect()?;

    let session = crate::execution::session_vwap(ticks)?;
    let pov_price = scalar_f64(&totals, "pov_price")?;
    let twap_price = scalar_f64(&totals, "twap_price")?;
    let bps = |p: f64| -> f64 { (p - session) / session * 1e4 };
    let pov_impact = scalar_f64(&totals, "temp")? + scalar_f64(&totals, "perm")?;
    let twap_impact = scalar_f64(&totals, "twap_temp")? + scalar_f64(&totals, "twap_perm")?;
    let twap_max = scalar_f64(&totals, "twap_max")?;

    let bkt = priced.column("bucket_ns")?.i64()?;
    let share = column_f64(&priced, "share")?;
    let vwap = column_f64(&priced, "vwap")?;
    let size = column_f64(&priced, "size")?;
    let temp = column_f64(&priced, "temp_bps")?;
    let perm = column_f64(&priced, "perm_bps")?;
    let schedule = (0..n)
        .map(|i| -> Result<PovSlice> {
            Ok(PovSlice {
                bucket_ns: bkt.get(i).ok_or_else(|| anyhow!("null bucket"))?,
                volume: r8(volume[i]),
                volume_share: r8(share[i]),
                vwap: r8(vwap[i]),
                size: r8(size[i]),
                participation: r8(participation),
                temp_bps: r8(temp[i]),
                perm_bps: r8(perm[i]),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(PovPlan {
        product: product.to_string(),
        bucket_ns,
        buckets: n,
        parent_qty: r8(parent_qty),
        cap: r8(cap),
        total_volume: r8(total_volume),
        participation: r8(participation),
        coef_bps: r8(coef_bps),
        perm_coef_bps: r8(perm_coef_bps),
        session_vwap: session,
        pov_price: r8(pov_price),
        pov_tracking_bps: r8(bps(pov_price)),
        pov_impact_bps: r8(pov_impact),
        twap_price: r8(twap_price),
        twap_tracking_bps: r8(bps(twap_price)),
        twap_impact_bps: r8(twap_impact),
        twap_max_participation: r8(twap_max),
        twap_feasible: r8(twap_max) <= r8(cap),
        edge_bps: r8(twap_impact - pov_impact),
        schedule,
    })
}

/// Replay the volume plan built on `plan` against the `exec` capture.
///
/// The product is read from the captures, which must agree. Unlike
/// [`pov_schedule`], an allocation that breaches the cap is *reported*
/// (`feasible: false`) rather than refused: the point is to show what the
/// forecast did, and a breach is the most important thing it can do. Mirrors
/// `pov_backtest` in `python/xexeclab/engine.py` operation for operation.
pub fn pov_backtest(
    plan: &[Tick],
    exec: &[Tick],
    bucket_ns: i64,
    parent_qty: f64,
    cap: f64,
    coef_bps: f64,
    perm_coef_bps: f64,
) -> Result<PovBacktest> {
    validate(bucket_ns, parent_qty, cap, coef_bps, perm_coef_bps)?;
    let (plan_profile, plan_volume, plan_total) =
        measured(plan, bucket_ns).map_err(|e| anyhow!("plan capture: {e}"))?;
    let (exec_profile, exec_volume, exec_total) =
        measured(exec, bucket_ns).map_err(|e| anyhow!("execution capture: {e}"))?;

    let product = exec[0].product.clone();
    if plan[0].product != product {
        return Err(anyhow!(
            "the plan capture is {} but the execution capture is {product}",
            plan[0].product
        ));
    }
    let plan_slots = slots(&plan_profile, bucket_ns)?;
    let exec_slots = slots(&exec_profile, bucket_ns)?;
    if plan_slots != exec_slots {
        return Err(anyhow!(
            "the captures cover different buckets: the plan traded in slots {plan_slots:?}, the execution in {exec_slots:?}"
        ));
    }

    let n = exec_slots.len();
    let priced = df!(
        "slot" => exec_slots,
        "plan_volume" => plan_volume,
        "exec_volume" => exec_volume,
        "vwap" => column_f64(&exec_profile, "vwap")?,
    )?
    .lazy()
    .with_columns([
        (col("plan_volume") / lit(plan_total)).alias("plan_share"),
        (col("exec_volume") / lit(exec_total)).alias("exec_share"),
    ])
    .with_column((col("plan_share") * lit(parent_qty)).alias("size"))
    .with_column((col("size") / col("exec_volume")).alias("participation"))
    .with_columns([
        (col("plan_share") * col("participation").sqrt() * lit(coef_bps)).alias("temp_bps"),
        (col("plan_share") * col("participation") * lit(perm_coef_bps)).alias("perm_bps"),
        (col("plan_share") * col("vwap")).alias("pv"),
        (col("plan_share") - col("exec_share")).alias("share_diff"),
    ])
    .collect()?;

    let totals = priced
        .clone()
        .lazy()
        .select([
            col("temp_bps").sum().alias("temp"),
            col("perm_bps").sum().alias("perm"),
            col("pv").sum().alias("price"),
            col("participation").max().alias("max_participation"),
            col("share_diff").abs().sum().alias("share_gap"),
        ])
        .collect()?;

    let session = crate::execution::session_vwap(exec)?;
    let price = scalar_f64(&totals, "price")?;
    let impact = scalar_f64(&totals, "temp")? + scalar_f64(&totals, "perm")?;
    let max_participation = scalar_f64(&totals, "max_participation")?;
    let oracle_participation = parent_qty / exec_total;
    let oracle_impact =
        coef_bps * oracle_participation.sqrt() + perm_coef_bps * oracle_participation;

    let slot = priced.column("slot")?.i64()?;
    let plan_share = column_f64(&priced, "plan_share")?;
    let exec_share = column_f64(&priced, "exec_share")?;
    let volume = column_f64(&priced, "exec_volume")?;
    let vwap = column_f64(&priced, "vwap")?;
    let size = column_f64(&priced, "size")?;
    let participation = column_f64(&priced, "participation")?;
    let temp = column_f64(&priced, "temp_bps")?;
    let perm = column_f64(&priced, "perm_bps")?;
    let schedule = (0..n)
        .map(|i| -> Result<BacktestSlice> {
            Ok(BacktestSlice {
                slot: slot.get(i).ok_or_else(|| anyhow!("null slot"))?,
                plan_share: r8(plan_share[i]),
                exec_share: r8(exec_share[i]),
                exec_volume: r8(volume[i]),
                vwap: r8(vwap[i]),
                size: r8(size[i]),
                participation: r8(participation[i]),
                temp_bps: r8(temp[i]),
                perm_bps: r8(perm[i]),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(PovBacktest {
        product,
        bucket_ns,
        buckets: n,
        parent_qty: r8(parent_qty),
        cap: r8(cap),
        coef_bps: r8(coef_bps),
        perm_coef_bps: r8(perm_coef_bps),
        plan_volume: r8(plan_total),
        exec_volume: r8(exec_total),
        profile_distance: r8(scalar_f64(&totals, "share_gap")? / 2.0),
        session_vwap: session,
        price: r8(price),
        tracking_bps: r8((price - session) / session * 1e4),
        impact_bps: r8(impact),
        max_participation: r8(max_participation),
        feasible: r8(max_participation) <= r8(cap),
        oracle_participation: r8(oracle_participation),
        oracle_impact_bps: r8(oracle_impact),
        oracle_feasible: r8(oracle_participation) <= r8(cap),
        forecast_cost_bps: r8(impact - oracle_impact),
        schedule,
    })
}
