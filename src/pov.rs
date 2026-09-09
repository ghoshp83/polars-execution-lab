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
    if ticks.is_empty() {
        return Err(anyhow!("no ticks"));
    }
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

    let scalar = |name: &str| -> Result<f64> {
        totals
            .column(name)?
            .f64()?
            .get(0)
            .ok_or_else(|| anyhow!("null {name}"))
    };
    let take = |name: &str| -> Result<Vec<f64>> {
        Ok(priced
            .column(name)?
            .f64()?
            .into_no_null_iter()
            .collect::<Vec<f64>>())
    };

    let session = crate::execution::session_vwap(ticks)?;
    let pov_price = scalar("pov_price")?;
    let twap_price = scalar("twap_price")?;
    let bps = |p: f64| -> f64 { (p - session) / session * 1e4 };
    let pov_impact = scalar("temp")? + scalar("perm")?;
    let twap_impact = scalar("twap_temp")? + scalar("twap_perm")?;
    let twap_max = scalar("twap_max")?;

    let bkt = priced.column("bucket_ns")?.i64()?;
    let share = take("share")?;
    let vwap = take("vwap")?;
    let size = take("size")?;
    let temp = take("temp_bps")?;
    let perm = take("perm_bps")?;
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
