use crate::model::Tick;
use anyhow::{anyhow, Context, Result};
use polars::prelude::*;
use serde::Serialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' streamed sessions compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// **Streamed session** -- the session benchmarks over a capture that is never
/// held in memory.
///
/// Every other command in this crate starts by reading the whole replay into a
/// `Vec` ([`crate::replay`]) and the Python side by calling `read_ticks`, which
/// materialises the whole file. That is fine for a fixture and wrong for a real
/// capture: a day of trades on a liquid product does not fit the machine that
/// wants the VWAP of it.
///
/// So the file is folded in chunks of `chunk_rows`. Each chunk is aggregated
/// with the Polars engine and reduced into a running accumulator; only one chunk
/// plus one carried tick is ever resident. `peak_rows_in_memory` reports that
/// bound, so the claim is a number in the output rather than a sentence in the
/// README.
///
/// The result is the same session VWAP, TWAP and order flow that
/// [`crate::execution::summary`] computes in one pass -- the fold is a different
/// route to the same answer, not a different answer.
///
/// A streamed pass cannot sort what it has not seen, so the capture must already
/// be in time order. One that is not is refused rather than silently mis-priced.
#[derive(Debug, Serialize)]
pub struct StreamReport {
    pub product: String,
    pub rows: usize,
    /// How many chunks the fold took.
    pub chunks: usize,
    pub chunk_rows: usize,
    /// The most ticks resident at once: one chunk, plus the tick carried across
    /// a chunk boundary to price the sample-and-hold interval that spans it.
    pub peak_rows_in_memory: usize,
    pub first_ts_ns: i64,
    pub last_ts_ns: i64,
    pub volume: f64,
    pub notional: f64,
    pub vwap: f64,
    pub twap: f64,
    pub buy_volume: f64,
    pub sell_volume: f64,
    pub imbalance: f64,
}

/// The running totals of the fold. Fixed size, whatever the file's size.
#[derive(Default)]
struct Acc {
    pv: f64,
    volume: f64,
    buy: f64,
    sell: f64,
    twap_num: f64,
    twap_den: f64,
}

/// Reduce one chunk of ticks into the accumulator with the Polars engine.
///
/// The price/size/side sums are the same expressions
/// [`crate::execution::session_vwap`] and [`crate::execution::order_flow`] use
/// over the whole frame; summing chunk sums is what makes the fold associative.
fn fold_chunk(buf: &[Tick], acc: &mut Acc) -> Result<()> {
    let price: Vec<f64> = buf.iter().map(|t| t.price).collect();
    let size: Vec<f64> = buf.iter().map(|t| t.size).collect();
    let side: Vec<String> = buf.iter().map(|t| t.side.clone()).collect();
    let out = df!("price" => price, "size" => size, "side" => side)?
        .lazy()
        .select([
            (col("price") * col("size")).sum().alias("pv"),
            col("size").sum().alias("v"),
            col("size")
                .filter(col("side").eq(lit("buy")))
                .sum()
                .alias("buy"),
            col("size")
                .filter(col("side").eq(lit("sell")))
                .sum()
                .alias("sell"),
        ])
        .collect()?;
    let g = |name: &str| -> f64 {
        out.column(name)
            .ok()
            .and_then(|c| c.f64().ok().and_then(|c| c.get(0)))
            .unwrap_or(0.0)
    };
    acc.pv += g("pv");
    acc.volume += g("v");
    acc.buy += g("buy");
    acc.sell += g("sell");
    Ok(())
}

/// Fold a session's benchmarks out of an NDJSON capture in bounded memory.
///
/// Mirrors `stream_session` in `python/xexeclab/engine.py` operation for
/// operation, so the two engines report identical streamed sessions.
pub fn stream_session<P: AsRef<Path>>(path: P, chunk_rows: usize) -> Result<StreamReport> {
    if chunk_rows == 0 {
        return Err(anyhow!("chunk_rows must be >= 1"));
    }
    let path = path.as_ref();
    // Parquet is a columnar sink, not a line-oriented stream: folding it needs a
    // row-group reader, which this crate's minimal Polars feature set does not
    // build. Say so rather than fail somewhere further down on bad UTF-8.
    let name = path.to_string_lossy().to_string();
    if !(name.ends_with(".ndjson") || name.ends_with(".jsonl")) {
        return Err(anyhow!("streaming reads NDJSON only, got {name}"));
    }
    let file = File::open(path).with_context(|| format!("opening replay file {path:?}"))?;

    let mut acc = Acc::default();
    let mut buf: Vec<Tick> = Vec::with_capacity(chunk_rows);
    let mut rows = 0usize;
    let mut chunks = 0usize;
    let mut product = String::new();
    let mut first_ts = 0i64;
    // The last tick of the previous chunk: its sample-and-hold interval runs
    // into the next chunk, so it is carried across the boundary rather than
    // dropped. Without it a chunked TWAP would silently lose one interval per
    // boundary and drift away from the in-memory answer.
    let mut carry: Option<Tick> = None;

    for (i, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let tick: Tick = serde_json::from_str(trimmed)
            .with_context(|| format!("parsing record on line {}", i + 1))?;
        if rows == 0 {
            product = tick.product.clone();
            first_ts = tick.ts_ns;
        }
        if let Some(prev) = carry.as_ref() {
            if tick.ts_ns < prev.ts_ns {
                return Err(anyhow!(
                    "replay is not in time order at row {}: {} follows {}",
                    rows + 1,
                    tick.ts_ns,
                    prev.ts_ns
                ));
            }
            let dt = (tick.ts_ns - prev.ts_ns) as f64;
            acc.twap_num += prev.price * dt;
            acc.twap_den += dt;
        }
        carry = Some(tick.clone());
        buf.push(tick);
        rows += 1;
        if buf.len() == chunk_rows {
            fold_chunk(&buf, &mut acc)?;
            chunks += 1;
            buf.clear();
        }
    }
    if !buf.is_empty() {
        fold_chunk(&buf, &mut acc)?;
        chunks += 1;
    }

    if rows == 0 {
        return Err(anyhow!("no ticks in {path:?}"));
    }
    if rows < 2 {
        return Err(anyhow!("need >= 2 ticks for twap"));
    }
    if acc.volume == 0.0 {
        return Err(anyhow!("zero total size"));
    }
    if acc.twap_den == 0.0 {
        return Err(anyhow!("zero elapsed time"));
    }

    let total = acc.buy + acc.sell;
    let imbalance = if total == 0.0 {
        0.0
    } else {
        (acc.buy - acc.sell) / total
    };
    let last_ts = carry.as_ref().map(|t| t.ts_ns).unwrap_or(first_ts);
    let peak = chunk_rows.min(rows) + usize::from(chunks > 1);

    Ok(StreamReport {
        product,
        rows,
        chunks,
        chunk_rows,
        peak_rows_in_memory: peak,
        first_ts_ns: first_ts,
        last_ts_ns: last_ts,
        volume: r8(acc.volume),
        notional: r8(acc.pv),
        vwap: r8(acc.pv / acc.volume),
        twap: r8(acc.twap_num / acc.twap_den),
        buy_volume: r8(acc.buy),
        sell_volume: r8(acc.sell),
        imbalance: r8(imbalance),
    })
}
