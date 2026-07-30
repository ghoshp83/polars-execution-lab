"""Execution-analytics core, computed with Polars.

Every function here has a byte-for-byte counterpart in the Rust crate
(`src/execution.rs`). The two are held identical by `test_equivalence.py`,
which runs the compiled Rust binary and this module on the same replay and
asserts the summaries match exactly.
"""

from __future__ import annotations

import math
from pathlib import Path

import polars as pl

TICK_COLUMNS = ("ts_ns", "product", "price", "size", "side", "trade_id")


def _r8(x: float) -> float:
    """Round to 8 dp, half away from zero -- matches the Rust `r8` exactly."""
    x = float(x)
    if x >= 0:
        return math.floor(x * 1e8 + 0.5) / 1e8
    return -(math.floor(-x * 1e8 + 0.5) / 1e8)


def read_ticks(path: str | Path) -> pl.DataFrame:
    """Read canonical ticks from an NDJSON replay file, sorted by time."""
    return pl.read_ndjson(str(path)).sort("ts_ns")


def bars(df: pl.DataFrame, bucket_ns: int) -> pl.DataFrame:
    """OHLCV + VWAP bars per fixed time bucket (full precision)."""
    bucketed = df.sort("ts_ns").with_columns(
        ((pl.col("ts_ns") // bucket_ns) * bucket_ns).alias("bucket_ns")
    )
    return (
        bucketed.group_by("bucket_ns")
        .agg(
            pl.col("price").first().alias("open"),
            pl.col("price").max().alias("high"),
            pl.col("price").min().alias("low"),
            pl.col("price").last().alias("close"),
            pl.col("size").sum().alias("volume"),
            (pl.col("price") * pl.col("size")).sum().alias("pv"),
        )
        .with_columns((pl.col("pv") / pl.col("volume")).alias("vwap"))
        .select("bucket_ns", "open", "high", "low", "close", "volume", "vwap")
        .sort("bucket_ns")
    )


def session_vwap(df: pl.DataFrame) -> float:
    """Size-weighted VWAP over all ticks."""
    row = df.select(
        (pl.col("price") * pl.col("size")).sum().alias("pv"),
        pl.col("size").sum().alias("v"),
    )
    v = row["v"][0]
    if v == 0:
        raise ValueError("zero total size")
    return _r8(row["pv"][0] / v)


def session_twap(df: pl.DataFrame) -> float:
    """Sample-and-hold TWAP: each price weighted by the gap to the next trade.

    The final trade has no following interval and is not weighted.
    """
    d = df.sort("ts_ns")
    ts = d["ts_ns"].to_list()
    px = d["price"].to_list()
    if len(ts) < 2:
        raise ValueError("need >= 2 ticks for twap")
    num = 0.0
    den = 0.0
    for i in range(len(ts) - 1):
        dt = float(ts[i + 1] - ts[i])
        num += px[i] * dt
        den += dt
    if den == 0:
        raise ValueError("zero elapsed time")
    return _r8(num / den)


def summary(df: pl.DataFrame, product: str, bucket_ns: int) -> dict:
    """Full summary: session VWAP + TWAP + per-bucket bars, rounded to match Rust."""
    raw = bars(df, bucket_ns).to_dicts()
    bar_rows = [
        {
            "bucket_ns": r["bucket_ns"],
            "open": _r8(r["open"]),
            "high": _r8(r["high"]),
            "low": _r8(r["low"]),
            "close": _r8(r["close"]),
            "volume": _r8(r["volume"]),
            "vwap": _r8(r["vwap"]),
        }
        for r in raw
    ]
    return {
        "product": product,
        "bucket_ns": bucket_ns,
        "ticks": df.height,
        "vwap": session_vwap(df),
        "twap": session_twap(df),
        "bars": bar_rows,
    }
