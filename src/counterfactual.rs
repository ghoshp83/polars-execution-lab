use crate::model::Fill;
use anyhow::{anyhow, Result};
use polars::prelude::*;
use serde::Serialize;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' counterfactuals compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// One interval under one allocation.
#[derive(Debug, Serialize)]
pub struct StrategyLeg {
    pub ts_ns: i64,
    /// Quantity this strategy would have put into this interval.
    pub qty: f64,
    pub price: f64,
    pub interval_volume: f64,
    /// `qty` as a fraction of the volume the interval actually traded.
    pub participation: f64,
    pub weight: f64,
    /// Signed drift from the arrival price to this interval's price, in bps.
    pub drift_bps: f64,
    /// What the two-term impact law charges a fill this size here.
    pub impact_bps: f64,
    pub cost_bps: f64,
}

/// The cost of one allocation of the same quantity over the same intervals.
#[derive(Debug, Serialize)]
pub struct StrategyCost {
    pub name: String,
    pub qty: f64,
    /// Quantity-weighted drift vs the arrival price: where the strategy chose
    /// to be in time.
    pub drift_bps: f64,
    /// Quantity-weighted impact: what the strategy's own size cost it.
    pub impact_bps: f64,
    /// `drift_bps + impact_bps`.
    pub cost_bps: f64,
    pub legs: Vec<StrategyLeg>,
}

/// **Counterfactual scheduling** -- was the schedule itself worth anything?
///
/// [`crate::shortfall`] splits a realised execution into the cost its size
/// always implied and the cost it did not. That residual is a bucket, and the
/// obvious next question is how much of it the *schedule* earned: would a plain
/// TWAP, or a volume-following participation, have paid more or less over the
/// very same intervals?
///
/// Each benchmark is handed the quantity that actually filled and spread across
/// the same intervals, priced on the same prices and the same traded volumes,
/// through the same two-term law -- `coef_bps * sqrt(participation)` temporary
/// plus `perm_coef_bps * participation` permanent. Holding quantity fixed is
/// what makes the comparison fair: the unfilled remainder is identical under
/// every strategy, so its opportunity cost cancels and is deliberately not
/// reported here.
///
/// `edge_bps` is the headline: `best_alternative - realised`, so a positive
/// number means the realised schedule beat the best simple benchmark.
#[derive(Debug, Serialize)]
pub struct CounterfactualReport {
    pub product: String,
    pub side: String,
    pub intervals: usize,
    /// The quantity every strategy is made to trade.
    pub filled_qty: f64,
    pub arrival_price: f64,
    pub realised: StrategyCost,
    pub alternatives: Vec<StrategyCost>,
    /// Name of the cheapest alternative.
    pub best_alternative: String,
    /// `best_alternative.cost_bps - realised.cost_bps`; positive means the
    /// realised schedule was the better one.
    pub edge_bps: f64,
}

/// Price one allocation. `qty_col` is the column holding that strategy's
/// quantity per interval.
fn price_strategy(
    frame: &DataFrame,
    qty_col: &str,
    name: &str,
    filled_qty: f64,
    arrival_price: f64,
    sign: f64,
    coef_bps: f64,
    perm_coef_bps: f64,
) -> Result<StrategyCost> {
    let priced = frame
        .clone()
        .lazy()
        .select([
            col("ts_ns"),
            col(qty_col).alias("qty"),
            col("price"),
            col("interval_volume"),
        ])
        .with_columns([
            (col("qty") / col("interval_volume")).alias("participation"),
            (col("qty") / lit(filled_qty)).alias("weight"),
            ((col("price") - lit(arrival_price)) / lit(arrival_price) * lit(1e4) * lit(sign))
                .alias("drift_bps"),
        ])
        .with_column(
            (col("participation").sqrt() * lit(coef_bps)
                + col("participation") * lit(perm_coef_bps))
            .alias("impact_bps"),
        )
        .with_column((col("drift_bps") + col("impact_bps")).alias("cost_bps"))
        .collect()?;

    // Pricing a counterfactual that takes more than the interval ever traded
    // would be fiction: the impact law is not defined above full participation.
    let over = priced
        .clone()
        .lazy()
        .select([(col("participation").max()).alias("max_participation")])
        .collect()?;
    let max_participation = over
        .column("max_participation")?
        .f64()?
        .get(0)
        .ok_or_else(|| anyhow!("null max_participation"))?;
    if max_participation > 1.0 {
        return Err(anyhow!(
            "the {name} allocation would take {max_participation} of an interval's volume; \
             refusing to price a counterfactual the market could not have filled"
        ));
    }

    let totals = priced
        .clone()
        .lazy()
        .select([
            (col("weight") * col("drift_bps")).sum().alias("drift"),
            (col("weight") * col("impact_bps")).sum().alias("impact"),
            col("qty").sum().alias("qty"),
        ])
        .collect()?;
    let g = |name: &str| -> Result<f64> {
        totals
            .column(name)?
            .f64()?
            .get(0)
            .ok_or_else(|| anyhow!("null {name}"))
    };
    let drift_bps = g("drift")?;
    let impact_bps = g("impact")?;
    let qty = g("qty")?;

    let col_f64 = |name: &str| -> Result<Vec<f64>> {
        Ok(priced
            .column(name)?
            .f64()?
            .into_no_null_iter()
            .collect::<Vec<f64>>())
    };
    let out_ts: Vec<i64> = priced.column("ts_ns")?.i64()?.into_no_null_iter().collect();
    let out_qty = col_f64("qty")?;
    let out_price = col_f64("price")?;
    let out_volume = col_f64("interval_volume")?;
    let out_part = col_f64("participation")?;
    let out_weight = col_f64("weight")?;
    let out_drift = col_f64("drift_bps")?;
    let out_impact = col_f64("impact_bps")?;
    let out_cost = col_f64("cost_bps")?;

    let legs = (0..out_ts.len())
        .map(|i| StrategyLeg {
            ts_ns: out_ts[i],
            qty: r8(out_qty[i]),
            price: r8(out_price[i]),
            interval_volume: r8(out_volume[i]),
            participation: r8(out_part[i]),
            weight: r8(out_weight[i]),
            drift_bps: r8(out_drift[i]),
            impact_bps: r8(out_impact[i]),
            cost_bps: r8(out_cost[i]),
        })
        .collect();

    Ok(StrategyCost {
        name: name.to_string(),
        qty: r8(qty),
        drift_bps: r8(drift_bps),
        impact_bps: r8(impact_bps),
        cost_bps: r8(drift_bps + impact_bps),
        legs,
    })
}

/// Compare the realised schedule against simple benchmarks over the same path.
///
/// Mirrors `counterfactual` in `python/xexeclab/engine.py` operation for
/// operation, including the sort that fixes the summation order, so the two
/// engines report bit-for-bit identical comparisons.
pub fn counterfactual(
    fills: &[Fill],
    product: &str,
    arrival_price: f64,
    coef_bps: f64,
    perm_coef_bps: f64,
) -> Result<CounterfactualReport> {
    if fills.is_empty() {
        return Err(anyhow!("no fills"));
    }
    if !arrival_price.is_finite() || arrival_price <= 0.0 {
        return Err(anyhow!(
            "arrival_price must be a positive finite number, got {arrival_price}"
        ));
    }
    for (name, v) in [("coef_bps", coef_bps), ("perm_coef_bps", perm_coef_bps)] {
        if !v.is_finite() || v < 0.0 {
            return Err(anyhow!(
                "{name} must be a non-negative finite number, got {v}"
            ));
        }
    }

    // Same rule as post-trade attribution: one parent order has one side.
    let side = fills[0].side.clone();
    if side != "buy" && side != "sell" {
        return Err(anyhow!("side must be buy or sell, got {side}"));
    }
    for f in fills {
        if f.side != side {
            return Err(anyhow!(
                "fills mix sides ({side} and {}); one parent order has one side",
                f.side
            ));
        }
        if !f.qty.is_finite() || f.qty <= 0.0 {
            return Err(anyhow!(
                "fill qty must be positive and finite, got {}",
                f.qty
            ));
        }
        if !f.price.is_finite() || f.price <= 0.0 {
            return Err(anyhow!(
                "fill price must be positive and finite, got {}",
                f.price
            ));
        }
        if !f.interval_volume.is_finite() || f.interval_volume <= 0.0 {
            return Err(anyhow!(
                "interval_volume must be positive and finite, got {}",
                f.interval_volume
            ));
        }
        if f.qty > f.interval_volume {
            return Err(anyhow!(
                "a fill of {} took more than the {} available in its interval",
                f.qty,
                f.interval_volume
            ));
        }
    }

    let sign = if side == "buy" { 1.0 } else { -1.0 };
    let ts_ns: Vec<i64> = fills.iter().map(|f| f.ts_ns).collect();
    let qty: Vec<f64> = fills.iter().map(|f| f.qty).collect();
    let price: Vec<f64> = fills.iter().map(|f| f.price).collect();
    let volume: Vec<f64> = fills.iter().map(|f| f.interval_volume).collect();

    let base = df!(
        "ts_ns" => ts_ns,
        "qty" => qty,
        "price" => price,
        "interval_volume" => volume,
    )?
    .lazy()
    .sort_by_exprs([col("ts_ns")], SortMultipleOptions::default())
    .collect()?;

    let agg = base
        .clone()
        .lazy()
        .select([
            col("qty").sum().alias("filled_qty"),
            col("interval_volume").sum().alias("total_volume"),
        ])
        .collect()?;
    let ga = |name: &str| -> Result<f64> {
        agg.column(name)?
            .f64()?
            .get(0)
            .ok_or_else(|| anyhow!("null {name}"))
    };
    let filled_qty = ga("filled_qty")?;
    let total_volume = ga("total_volume")?;
    let n = base.height() as f64;

    // Every strategy trades the same quantity over the same intervals. The
    // unfilled remainder is identical under all of them, so it cancels.
    let frame = base
        .lazy()
        .with_columns([
            lit(filled_qty / n).alias("twap_qty"),
            (col("interval_volume") / lit(total_volume) * lit(filled_qty)).alias("volume_qty"),
        ])
        .collect()?;

    let realised = price_strategy(
        &frame,
        "qty",
        "realised",
        filled_qty,
        arrival_price,
        sign,
        coef_bps,
        perm_coef_bps,
    )?;
    let mut alternatives = Vec::new();
    for (col_name, label) in [("twap_qty", "twap"), ("volume_qty", "volume")] {
        alternatives.push(price_strategy(
            &frame,
            col_name,
            label,
            filled_qty,
            arrival_price,
            sign,
            coef_bps,
            perm_coef_bps,
        )?);
    }

    let best = alternatives
        .iter()
        .min_by(|a, b| a.cost_bps.total_cmp(&b.cost_bps))
        .ok_or_else(|| anyhow!("no alternatives"))?;
    let best_alternative = best.name.clone();
    let edge_bps = best.cost_bps - realised.cost_bps;

    Ok(CounterfactualReport {
        product: product.to_string(),
        side,
        intervals: fills.len(),
        filled_qty: r8(filled_qty),
        arrival_price: r8(arrival_price),
        realised,
        alternatives,
        best_alternative,
        edge_bps: r8(edge_bps),
    })
}
