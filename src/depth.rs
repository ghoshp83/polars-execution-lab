use crate::model::BookLevel;
use anyhow::{anyhow, Result};
use polars::prelude::*;
use serde::Serialize;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' depth metrics compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// Session-level L2 depth microstructure over an order-book replay.
///
/// The engine first reduces every snapshot (all rows sharing a `ts_ns`) to its
/// resting depth on each side and its top-of-book spread, then averages those
/// per-snapshot quantities over the window:
/// - `avg_bid_depth` / `avg_ask_depth` -- mean total resting size across the
///   captured levels of each side (how much liquidity is standing).
/// - `avg_depth_imbalance` -- mean `(bid_depth - ask_depth) / (bid_depth +
///   ask_depth)` in `[-1, 1]`: positive means the book rests heavier on the bid.
/// - `avg_spread` -- mean top-of-book `best_ask - best_bid`.
#[derive(Debug, Serialize)]
pub struct DepthSummary {
    pub product: String,
    pub snapshots: usize,
    pub avg_bid_depth: f64,
    pub avg_ask_depth: f64,
    pub avg_depth_imbalance: f64,
    pub avg_spread: f64,
}

/// Compute session depth metrics from L2 book levels with the Polars engine.
/// Mirrors `depth_metrics` in `python/xexeclab/engine.py` expression for
/// expression, including the sort that fixes the per-snapshot summation order so
/// the cross-language means are bit-for-bit identical.
pub fn depth_metrics(levels: &[BookLevel], product: &str) -> Result<DepthSummary> {
    if levels.is_empty() {
        return Err(anyhow!("no book levels"));
    }
    let ts_ns: Vec<i64> = levels.iter().map(|l| l.ts_ns).collect();
    let side: Vec<String> = levels.iter().map(|l| l.side.clone()).collect();
    let level: Vec<i64> = levels.iter().map(|l| l.level).collect();
    let price: Vec<f64> = levels.iter().map(|l| l.price).collect();
    let size: Vec<f64> = levels.iter().map(|l| l.size).collect();

    // Stage 1: collapse each snapshot to its per-side depth and top-of-book.
    let per_snap = df!(
        "ts_ns" => ts_ns,
        "side" => side,
        "level" => level,
        "price" => price,
        "size" => size,
    )?
    .lazy()
    .group_by([col("ts_ns")])
    .agg([
        col("size")
            .filter(col("side").eq(lit("bid")))
            .sum()
            .alias("bid_depth"),
        col("size")
            .filter(col("side").eq(lit("ask")))
            .sum()
            .alias("ask_depth"),
        col("price")
            .filter(col("side").eq(lit("bid")).and(col("level").eq(lit(0i64))))
            .first()
            .alias("best_bid"),
        col("price")
            .filter(col("side").eq(lit("ask")).and(col("level").eq(lit(0i64))))
            .first()
            .alias("best_ask"),
    ])
    .sort_by_exprs([col("ts_ns")], SortMultipleOptions::default())
    .collect()?;
    let snapshots = per_snap.height();

    // Stage 2: average the per-snapshot quantities over the window.
    let bid_depth = col("bid_depth");
    let ask_depth = col("ask_depth");
    let out = per_snap
        .lazy()
        .select([
            bid_depth.clone().mean().alias("avg_bid_depth"),
            ask_depth.clone().mean().alias("avg_ask_depth"),
            ((bid_depth.clone() - ask_depth.clone()) / (bid_depth + ask_depth))
                .mean()
                .alias("avg_depth_imbalance"),
            (col("best_ask") - col("best_bid"))
                .mean()
                .alias("avg_spread"),
        ])
        .collect()?;

    let f = |name: &str| -> Result<f64> {
        out.column(name)?
            .f64()?
            .get(0)
            .ok_or_else(|| anyhow!("null {name}"))
    };
    Ok(DepthSummary {
        product: product.to_string(),
        snapshots,
        avg_bid_depth: r8(f("avg_bid_depth")?),
        avg_ask_depth: r8(f("avg_ask_depth")?),
        avg_depth_imbalance: r8(f("avg_depth_imbalance")?),
        avg_spread: r8(f("avg_spread")?),
    })
}

/// Session-level top-of-book **queue position**: the size resting at the touch
/// (`level == 0`) on each side, the quantity that governs passive fill
/// priority. A new passive order joins the back of this queue and only fills
/// once the size ahead of it trades through, so a heavier queue means a longer
/// wait and a lower fill probability for a maker on that side.
///
/// This is deliberately distinct from [`DepthSummary`], which sums resting size
/// across *all* captured levels: queue position is the size at the *best* level
/// alone.
/// - `avg_bid_queue` / `avg_ask_queue` -- mean size resting at the best bid /
///   best ask across the window.
/// - `avg_queue_imbalance` -- mean `(bid_queue - ask_queue) / (bid_queue +
///   ask_queue)` in `[-1, 1]`: positive means the heavier queue rests on the
///   bid, so a passive bid faces a longer queue (worse fill priority) than a
///   passive ask. This is the fill-priority signal.
#[derive(Debug, Serialize)]
pub struct QueueSummary {
    pub product: String,
    pub snapshots: usize,
    pub avg_bid_queue: f64,
    pub avg_ask_queue: f64,
    pub avg_queue_imbalance: f64,
}

/// Compute session top-of-book queue metrics from L2 book levels with the
/// Polars engine. Mirrors `queue_metrics` in `python/xexeclab/engine.py`
/// expression for expression, including the sort that fixes the per-snapshot
/// order so the cross-language means are bit-for-bit identical.
pub fn queue_metrics(levels: &[BookLevel], product: &str) -> Result<QueueSummary> {
    if levels.is_empty() {
        return Err(anyhow!("no book levels"));
    }
    let ts_ns: Vec<i64> = levels.iter().map(|l| l.ts_ns).collect();
    let side: Vec<String> = levels.iter().map(|l| l.side.clone()).collect();
    let level: Vec<i64> = levels.iter().map(|l| l.level).collect();
    let size: Vec<f64> = levels.iter().map(|l| l.size).collect();

    // Stage 1: reduce each snapshot to the resting size at its best level per side.
    let per_snap = df!(
        "ts_ns" => ts_ns,
        "side" => side,
        "level" => level,
        "size" => size,
    )?
    .lazy()
    .group_by([col("ts_ns")])
    .agg([
        col("size")
            .filter(col("side").eq(lit("bid")).and(col("level").eq(lit(0i64))))
            .first()
            .alias("bid_queue"),
        col("size")
            .filter(col("side").eq(lit("ask")).and(col("level").eq(lit(0i64))))
            .first()
            .alias("ask_queue"),
    ])
    .sort_by_exprs([col("ts_ns")], SortMultipleOptions::default())
    .collect()?;
    let snapshots = per_snap.height();

    // Stage 2: average the per-snapshot queue quantities over the window.
    let bid_queue = col("bid_queue");
    let ask_queue = col("ask_queue");
    let out = per_snap
        .lazy()
        .select([
            bid_queue.clone().mean().alias("avg_bid_queue"),
            ask_queue.clone().mean().alias("avg_ask_queue"),
            ((bid_queue.clone() - ask_queue.clone()) / (bid_queue + ask_queue))
                .mean()
                .alias("avg_queue_imbalance"),
        ])
        .collect()?;

    let f = |name: &str| -> Result<f64> {
        out.column(name)?
            .f64()?
            .get(0)
            .ok_or_else(|| anyhow!("null {name}"))
    };
    Ok(QueueSummary {
        product: product.to_string(),
        snapshots,
        avg_bid_queue: r8(f("avg_bid_queue")?),
        avg_ask_queue: r8(f("avg_ask_queue")?),
        avg_queue_imbalance: r8(f("avg_queue_imbalance")?),
    })
}
