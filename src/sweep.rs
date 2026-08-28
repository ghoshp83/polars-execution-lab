use crate::model::BookLevel;
use anyhow::{anyhow, Result};
use polars::prelude::*;
use serde::Serialize;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' sweep metrics compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// Session-level **book-sweep cost**: what a marketable order of a fixed size
/// would actually pay if it were sent into each captured snapshot and walked the
/// resting levels until filled.
///
/// [`crate::depth::DepthSummary`] answers "how much size is standing" and
/// [`crate::depth::QueueSummary`] answers "how long is the passive line". Neither
/// prices a *taker*: an order larger than the touch eats level 0, then level 1,
/// then level 2, and its realised price is the size-weighted average of the
/// levels it consumed. That consumption cost is the liquidity cost the impact
/// model in [`crate::impact`] parameterises -- here it is measured directly off
/// the book instead of being modelled, so the two can be compared.
///
/// A `buy` consumes the ask side upward from the best ask; a `sell` consumes the
/// bid side downward from the best bid.
/// - `avg_sweep_vwap` -- mean realised price of the sweep across snapshots.
/// - `avg_slippage_bps` -- mean cost of the sweep against the touch it started
///   from, in basis points and signed so a larger number is always worse:
///   `(vwap - best_ask) / best_ask` for a buy, `(best_bid - vwap) / best_bid`
///   for a sell. An order that rests entirely inside the touch pays 0.
/// - `avg_levels_consumed` -- mean number of book levels the order had to eat.
/// - `avg_fill_ratio` -- mean filled quantity as a fraction of the order size;
///   below 1 the captured book was too thin to complete the order.
/// - `filled_snapshots` -- snapshots where the book fully absorbed the order.
#[derive(Debug, Serialize)]
pub struct SweepSummary {
    pub product: String,
    pub side: String,
    pub order_size: f64,
    pub snapshots: usize,
    pub filled_snapshots: usize,
    pub avg_sweep_vwap: f64,
    pub avg_slippage_bps: f64,
    pub avg_levels_consumed: f64,
    pub avg_fill_ratio: f64,
}

/// Compute session book-sweep cost from L2 book levels with the Polars engine.
///
/// `side` is the taker's side: `"buy"` sweeps the asks, `"sell"` sweeps the bids.
/// Mirrors `sweep_cost` in `python/xexeclab/engine.py` expression for
/// expression, including the sorts that fix the consumption order so the
/// cross-language means are bit-for-bit identical.
pub fn sweep_cost(
    levels: &[BookLevel],
    product: &str,
    side: &str,
    order_size: f64,
) -> Result<SweepSummary> {
    if levels.is_empty() {
        return Err(anyhow!("no book levels"));
    }
    if !(order_size > 0.0) {
        return Err(anyhow!("order_size must be positive"));
    }
    // The taker crosses the opposite side of the book.
    let book_side = match side {
        "buy" => "ask",
        "sell" => "bid",
        other => return Err(anyhow!("side must be buy or sell, got {other}")),
    };

    let ts_ns: Vec<i64> = levels.iter().map(|l| l.ts_ns).collect();
    let lside: Vec<String> = levels.iter().map(|l| l.side.clone()).collect();
    let price: Vec<f64> = levels.iter().map(|l| l.price).collect();
    let size: Vec<f64> = levels.iter().map(|l| l.size).collect();

    // Stage 1: walk each snapshot's book. Levels are consumed in the order the
    // taker meets them -- asks cheapest first, bids dearest first -- so the
    // running total of size *ahead* of a level decides how much of that level
    // the order reaches.
    let cum_before = col("size").cum_sum(false).over([col("ts_ns")]) - col("size");
    let remaining = lit(order_size) - cum_before;
    let alloc = when(remaining.clone().lt_eq(lit(0.0)))
        .then(lit(0.0))
        .otherwise(
            when(remaining.clone().lt(col("size")))
                .then(remaining)
                .otherwise(col("size")),
        )
        .alias("alloc");

    let per_snap = df!(
        "ts_ns" => ts_ns,
        "side" => lside,
        "price" => price,
        "size" => size,
    )?
    .lazy()
    .filter(col("side").eq(lit(book_side)))
    .sort_by_exprs(
        [col("ts_ns"), col("price")],
        SortMultipleOptions::default().with_order_descending_multi([false, side == "sell"]),
    )
    .with_column(alloc)
    .group_by([col("ts_ns")])
    .agg([
        (col("alloc") * col("price")).sum().alias("notional"),
        col("alloc").sum().alias("filled"),
        col("alloc")
            .gt(lit(0.0))
            .sum()
            .cast(DataType::Float64)
            .alias("levels_consumed"),
        col("price").first().alias("touch"),
    ])
    .sort_by_exprs([col("ts_ns")], SortMultipleOptions::default())
    .collect()?;
    let snapshots = per_snap.height();

    // Stage 2: price each snapshot's sweep, then average over the window.
    let vwap = col("notional") / col("filled");
    // Signed so a larger number is always worse for the taker on either side.
    let slippage_bps = if side == "buy" {
        (vwap.clone() - col("touch")) / col("touch") * lit(10_000.0)
    } else {
        (col("touch") - vwap.clone()) / col("touch") * lit(10_000.0)
    };
    let out = per_snap
        .lazy()
        .select([
            vwap.mean().alias("avg_sweep_vwap"),
            slippage_bps.mean().alias("avg_slippage_bps"),
            col("levels_consumed").mean().alias("avg_levels_consumed"),
            (col("filled") / lit(order_size)).mean().alias("avg_fill_ratio"),
            // 1e-9 absorbs float-sum noise: a book that exactly covers the
            // order must count as filled, not miss by an ulp.
            col("filled")
                .gt_eq(lit(order_size - 1e-9))
                .sum()
                .cast(DataType::Float64)
                .alias("filled_snapshots"),
        ])
        .collect()?;

    let f = |name: &str| -> Result<f64> {
        out.column(name)?
            .f64()?
            .get(0)
            .ok_or_else(|| anyhow!("null {name}"))
    };
    Ok(SweepSummary {
        product: product.to_string(),
        side: side.to_string(),
        order_size,
        snapshots,
        filled_snapshots: f("filled_snapshots")? as usize,
        avg_sweep_vwap: r8(f("avg_sweep_vwap")?),
        avg_slippage_bps: r8(f("avg_slippage_bps")?),
        avg_levels_consumed: r8(f("avg_levels_consumed")?),
        avg_fill_ratio: r8(f("avg_fill_ratio")?),
    })
}
