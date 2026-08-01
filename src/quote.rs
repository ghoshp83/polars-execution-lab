use crate::model::Quote;
use anyhow::{anyhow, Result};
use polars::prelude::*;
use serde::Serialize;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' quote metrics compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// Session-level top-of-book microstructure metrics over a quote replay.
///
/// Every field is the mean of a per-quote quantity, so the summary answers
/// "what did the book look like on average over this window":
/// - `avg_spread` -- mean `ask - bid` (tightness / liquidity cost).
/// - `avg_mid` -- mean mid-price `(bid + ask) / 2`.
/// - `avg_microprice` -- mean size-weighted fair value
///   `(bid*ask_size + ask*bid_size) / (bid_size + ask_size)`; it leans toward
///   the side with *less* resting size, i.e. where price is more likely to move.
/// - `avg_book_imbalance` -- mean `(bid_size - ask_size) / (bid_size + ask_size)`
///   in `[-1, 1]`: positive means more resting size on the bid.
#[derive(Debug, Serialize)]
pub struct QuoteSummary {
    pub product: String,
    pub quotes: usize,
    pub avg_spread: f64,
    pub avg_mid: f64,
    pub avg_microprice: f64,
    pub avg_book_imbalance: f64,
}

/// Compute session microstructure metrics from top-of-book quotes with the
/// Polars engine. Mirrors `quote_metrics` in `python/xexeclab/engine.py`.
pub fn quote_metrics(quotes: &[Quote], product: &str) -> Result<QuoteSummary> {
    if quotes.is_empty() {
        return Err(anyhow!("no quotes"));
    }
    let bid: Vec<f64> = quotes.iter().map(|q| q.bid).collect();
    let bid_size: Vec<f64> = quotes.iter().map(|q| q.bid_size).collect();
    let ask: Vec<f64> = quotes.iter().map(|q| q.ask).collect();
    let ask_size: Vec<f64> = quotes.iter().map(|q| q.ask_size).collect();

    let depth = col("bid_size") + col("ask_size");
    let out = df!(
        "bid" => bid,
        "bid_size" => bid_size,
        "ask" => ask,
        "ask_size" => ask_size,
    )?
    .lazy()
    .select([
        (col("ask") - col("bid")).mean().alias("avg_spread"),
        ((col("bid") + col("ask")) / lit(2.0))
            .mean()
            .alias("avg_mid"),
        ((col("bid") * col("ask_size") + col("ask") * col("bid_size")) / depth.clone())
            .mean()
            .alias("avg_microprice"),
        ((col("bid_size") - col("ask_size")) / depth)
            .mean()
            .alias("avg_book_imbalance"),
    ])
    .collect()?;

    let f = |name: &str| -> Result<f64> {
        out.column(name)?
            .f64()?
            .get(0)
            .ok_or_else(|| anyhow!("null {name}"))
    };
    Ok(QuoteSummary {
        product: product.to_string(),
        quotes: quotes.len(),
        avg_spread: r8(f("avg_spread")?),
        avg_mid: r8(f("avg_mid")?),
        avg_microprice: r8(f("avg_microprice")?),
        avg_book_imbalance: r8(f("avg_book_imbalance")?),
    })
}
