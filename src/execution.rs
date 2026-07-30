use crate::model::Tick;
use anyhow::{anyhow, Result};
use polars::prelude::*;
use serde::Serialize;

/// Round to 8 decimal places, half away from zero. The Python side rounds the
/// same way so the two engines' outputs compare exactly.
fn r8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

/// One OHLCV+VWAP bar for a fixed time bucket.
#[derive(Debug, Serialize, PartialEq)]
pub struct Bar {
    pub bucket_ns: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub vwap: f64,
}

/// A full execution-analytics summary of a replay.
#[derive(Debug, Serialize)]
pub struct Summary {
    pub product: String,
    pub bucket_ns: i64,
    pub ticks: usize,
    pub vwap: f64,
    pub twap: f64,
    pub bars: Vec<Bar>,
}

fn sorted_by_ts(ticks: &[Tick]) -> Vec<&Tick> {
    let mut v: Vec<&Tick> = ticks.iter().collect();
    v.sort_by_key(|t| t.ts_ns);
    v
}

/// OHLCV + VWAP bars per fixed time bucket, aggregated with the Polars engine.
pub fn bars(ticks: &[Tick], bucket_ns: i64) -> Result<Vec<Bar>> {
    if ticks.is_empty() {
        return Ok(vec![]);
    }
    let sorted = sorted_by_ts(ticks);
    let bucket: Vec<i64> = sorted
        .iter()
        .map(|t| (t.ts_ns / bucket_ns) * bucket_ns)
        .collect();
    let price: Vec<f64> = sorted.iter().map(|t| t.price).collect();
    let size: Vec<f64> = sorted.iter().map(|t| t.size).collect();

    let df = df!(
        "bucket_ns" => bucket,
        "price" => price,
        "size" => size,
    )?;

    let out = df
        .lazy()
        .group_by([col("bucket_ns")])
        .agg([
            col("price").first().alias("open"),
            col("price").max().alias("high"),
            col("price").min().alias("low"),
            col("price").last().alias("close"),
            col("size").sum().alias("volume"),
            (col("price") * col("size")).sum().alias("pv"),
        ])
        .with_column((col("pv") / col("volume")).alias("vwap"))
        .select([
            col("bucket_ns"),
            col("open"),
            col("high"),
            col("low"),
            col("close"),
            col("volume"),
            col("vwap"),
        ])
        .collect()?;

    let bkt = out.column("bucket_ns")?.i64()?;
    let open = out.column("open")?.f64()?;
    let high = out.column("high")?.f64()?;
    let low = out.column("low")?.f64()?;
    let close = out.column("close")?.f64()?;
    let volume = out.column("volume")?.f64()?;
    let vwap = out.column("vwap")?.f64()?;

    let get = |c: &Float64Chunked, i: usize| -> Result<f64> {
        c.get(i)
            .ok_or_else(|| anyhow!("unexpected null in bar aggregate"))
    };

    let mut bars: Vec<Bar> = (0..out.height())
        .map(|i| -> Result<Bar> {
            Ok(Bar {
                bucket_ns: bkt.get(i).ok_or_else(|| anyhow!("null bucket"))?,
                open: r8(get(open, i)?),
                high: r8(get(high, i)?),
                low: r8(get(low, i)?),
                close: r8(get(close, i)?),
                volume: r8(get(volume, i)?),
                vwap: r8(get(vwap, i)?),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    bars.sort_by_key(|b| b.bucket_ns);
    Ok(bars)
}

/// Size-weighted session VWAP over all ticks, reduced with the Polars engine.
pub fn session_vwap(ticks: &[Tick]) -> Result<f64> {
    if ticks.is_empty() {
        return Err(anyhow!("no ticks"));
    }
    let price: Vec<f64> = ticks.iter().map(|t| t.price).collect();
    let size: Vec<f64> = ticks.iter().map(|t| t.size).collect();
    let out = df!("price" => price, "size" => size)?
        .lazy()
        .select([
            (col("price") * col("size")).sum().alias("pv"),
            col("size").sum().alias("v"),
        ])
        .collect()?;
    let pv = out
        .column("pv")?
        .f64()?
        .get(0)
        .ok_or_else(|| anyhow!("pv"))?;
    let v = out.column("v")?.f64()?.get(0).ok_or_else(|| anyhow!("v"))?;
    if v == 0.0 {
        return Err(anyhow!("zero total size"));
    }
    Ok(r8(pv / v))
}

/// Sample-and-hold session TWAP: each trade's price is weighted by the time
/// until the next trade. The final trade has no following interval and is
/// therefore not weighted.
pub fn session_twap(ticks: &[Tick]) -> Result<f64> {
    let sorted = sorted_by_ts(ticks);
    if sorted.len() < 2 {
        return Err(anyhow!("need >= 2 ticks for twap"));
    }
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    for w in sorted.windows(2) {
        let dt = (w[1].ts_ns - w[0].ts_ns) as f64;
        num += w[0].price * dt;
        den += dt;
    }
    if den == 0.0 {
        return Err(anyhow!("zero elapsed time"));
    }
    Ok(r8(num / den))
}

/// Full summary: session VWAP + TWAP + per-bucket bars.
pub fn summary(ticks: &[Tick], product: &str, bucket_ns: i64) -> Result<Summary> {
    Ok(Summary {
        product: product.to_string(),
        bucket_ns,
        ticks: ticks.len(),
        vwap: session_vwap(ticks)?,
        twap: session_twap(ticks)?,
        bars: bars(ticks, bucket_ns)?,
    })
}
