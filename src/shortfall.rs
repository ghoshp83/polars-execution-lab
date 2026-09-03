use crate::model::Fill;
use anyhow::{anyhow, Result};
use polars::prelude::*;
use serde::Serialize;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' attributions compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// One realised fill, priced against the decision price and against the model.
#[derive(Debug, Serialize)]
pub struct ShortfallSlice {
    pub ts_ns: i64,
    pub qty: f64,
    pub price: f64,
    /// `qty` as a fraction of the volume available in this fill's interval.
    pub participation: f64,
    /// `qty` as a fraction of the quantity actually filled.
    pub weight: f64,
    /// What this fill actually paid versus the arrival price, in basis points,
    /// signed so that positive is always a cost to the parent.
    pub realised_bps: f64,
    /// What the repo's impact model says a fill this size should have paid.
    pub modelled_bps: f64,
    /// `realised_bps - modelled_bps`: the part the impact model does not explain.
    pub residual_bps: f64,
}

/// **Post-trade attribution** -- decompose what an execution actually cost into
/// the part the impact model predicts and the part it does not.
///
/// [`crate::schedule`] chooses a trajectory before the order goes out; this is
/// the other half of that loop, run after the fills come back. The realised
/// implementation shortfall of the filled quantity is measured against the
/// arrival (decision) price, then each fill is priced *again* through the same
/// two-term law the rest of this repo uses -- `coef_bps * sqrt(participation)`
/// temporary plus `perm_coef_bps * participation` permanent -- and the two are
/// differenced.
///
/// The difference is the number worth reading. `modelled_bps` is the cost the
/// order was always going to pay for its size: no algorithm avoids it, and a
/// desk should not be praised or blamed for it. `residual_bps` is what is left
/// -- venue selection, timing, spread capture, adverse selection, luck. Judging
/// an execution on `realised_bps` alone rewards whoever happened to be given the
/// small orders.
///
/// Quantity that never filled is not silently dropped. It is charged as
/// `opportunity_bps` -- the parent-weighted drift from the arrival price to the
/// last price seen -- so an algorithm that improves its average price by simply
/// not finishing does not come out ahead. `realised_bps` is quoted on the filled
/// notional; `total_bps` is quoted on the parent, and is the honest headline.
#[derive(Debug, Serialize)]
pub struct ShortfallSummary {
    pub product: String,
    pub side: String,
    pub fills: usize,
    pub parent_qty: f64,
    pub filled_qty: f64,
    pub unfilled_qty: f64,
    pub fill_rate: f64,
    pub arrival_price: f64,
    /// Quantity-weighted average price of the fills.
    pub avg_price: f64,
    /// Price of the last fill: where the market had gone by the time the
    /// unfilled remainder was abandoned.
    pub final_price: f64,
    /// Realised shortfall of the *filled* quantity vs arrival, in bps.
    pub realised_bps: f64,
    /// The part of `realised_bps` the impact model predicts.
    pub modelled_bps: f64,
    /// The part it does not: `realised_bps - modelled_bps`.
    pub residual_bps: f64,
    /// Parent-weighted cost of the quantity that never filled.
    pub opportunity_bps: f64,
    /// `fill_rate * realised_bps + opportunity_bps` -- the cost on the parent.
    pub total_bps: f64,
    pub slices: Vec<ShortfallSlice>,
}

/// Attribute the realised cost of an execution against the arrival price.
///
/// Mirrors `shortfall` in `python/xexeclab/engine.py` operation for operation,
/// including the sort that fixes the summation order, so the two engines report
/// bit-for-bit identical attributions.
pub fn shortfall(
    fills: &[Fill],
    product: &str,
    parent_qty: f64,
    arrival_price: f64,
    coef_bps: f64,
    perm_coef_bps: f64,
) -> Result<ShortfallSummary> {
    if fills.is_empty() {
        return Err(anyhow!("no fills"));
    }
    for (name, v) in [("parent_qty", parent_qty), ("arrival_price", arrival_price)] {
        if !v.is_finite() || v <= 0.0 {
            return Err(anyhow!("{name} must be a positive finite number, got {v}"));
        }
    }
    for (name, v) in [("coef_bps", coef_bps), ("perm_coef_bps", perm_coef_bps)] {
        if !v.is_finite() || v < 0.0 {
            return Err(anyhow!(
                "{name} must be a non-negative finite number, got {v}"
            ));
        }
    }

    // A parent order has one side. Mixed sides in one file is a data error, not
    // a netting instruction -- signing the shortfall would be meaningless.
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
            return Err(anyhow!("fill qty must be positive and finite, got {}", f.qty));
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

    let frame = df!(
        "ts_ns" => ts_ns,
        "qty" => qty,
        "price" => price,
        "interval_volume" => volume,
    )?
    .lazy()
    .sort_by_exprs([col("ts_ns")], SortMultipleOptions::default())
    .collect()?;

    let filled = frame
        .clone()
        .lazy()
        .select([col("qty").sum().alias("filled_qty")])
        .collect()?;
    let filled_qty = filled
        .column("filled_qty")?
        .f64()?
        .get(0)
        .ok_or_else(|| anyhow!("null filled_qty"))?;
    // Filling more than the parent is a reconciliation error upstream; reporting
    // a fill rate above 1 would hide it.
    if filled_qty > parent_qty {
        return Err(anyhow!(
            "fills total {filled_qty} against a parent of {parent_qty}"
        ));
    }

    let priced = frame
        .lazy()
        .with_columns([
            (col("qty") / col("interval_volume")).alias("participation"),
            (col("qty") / lit(filled_qty)).alias("weight"),
            ((col("price") - lit(arrival_price)) / lit(arrival_price) * lit(1e4) * lit(sign))
                .alias("realised_bps"),
        ])
        .with_column(
            (col("participation").sqrt() * lit(coef_bps)
                + col("participation") * lit(perm_coef_bps))
            .alias("modelled_bps"),
        )
        .with_column((col("realised_bps") - col("modelled_bps")).alias("residual_bps"))
        .collect()?;

    let totals = priced
        .clone()
        .lazy()
        .select([
            (col("weight") * col("realised_bps")).sum().alias("realised"),
            (col("weight") * col("modelled_bps")).sum().alias("modelled"),
            (col("weight") * col("price")).sum().alias("avg_price"),
            col("price").last().alias("final_price"),
        ])
        .collect()?;
    let g = |name: &str| -> Result<f64> {
        totals
            .column(name)?
            .f64()?
            .get(0)
            .ok_or_else(|| anyhow!("null {name}"))
    };
    let realised_bps = g("realised")?;
    let modelled_bps = g("modelled")?;
    let avg_price = g("avg_price")?;
    let final_price = g("final_price")?;

    let unfilled_qty = parent_qty - filled_qty;
    let fill_rate = filled_qty / parent_qty;
    // The remainder is charged the drift it walked away from, weighted by how
    // much of the parent it was. A fully-filled parent pays nothing here.
    let opportunity_bps =
        (unfilled_qty / parent_qty) * sign * (final_price - arrival_price) / arrival_price * 1e4;
    let total_bps = fill_rate * realised_bps + opportunity_bps;

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
    let out_part = col_f64("participation")?;
    let out_weight = col_f64("weight")?;
    let out_realised = col_f64("realised_bps")?;
    let out_modelled = col_f64("modelled_bps")?;
    let out_residual = col_f64("residual_bps")?;

    let slices = (0..out_ts.len())
        .map(|i| ShortfallSlice {
            ts_ns: out_ts[i],
            qty: r8(out_qty[i]),
            price: r8(out_price[i]),
            participation: r8(out_part[i]),
            weight: r8(out_weight[i]),
            realised_bps: r8(out_realised[i]),
            modelled_bps: r8(out_modelled[i]),
            residual_bps: r8(out_residual[i]),
        })
        .collect();

    Ok(ShortfallSummary {
        product: product.to_string(),
        side,
        fills: fills.len(),
        parent_qty: r8(parent_qty),
        filled_qty: r8(filled_qty),
        unfilled_qty: r8(unfilled_qty),
        fill_rate: r8(fill_rate),
        arrival_price: r8(arrival_price),
        avg_price: r8(avg_price),
        final_price: r8(final_price),
        realised_bps: r8(realised_bps),
        modelled_bps: r8(modelled_bps),
        residual_bps: r8(realised_bps - modelled_bps),
        opportunity_bps: r8(opportunity_bps),
        total_bps: r8(total_bps),
        slices,
    })
}
