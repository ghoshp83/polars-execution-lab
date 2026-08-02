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
QUOTE_COLUMNS = ("ts_ns", "product", "bid", "bid_size", "ask", "ask_size")
BOOK_LEVEL_COLUMNS = ("ts_ns", "product", "side", "level", "price", "size")


def _r8(x: float) -> float:
    """Round to 8 dp, half away from zero -- matches the Rust `r8` exactly."""
    x = float(x)
    if x >= 0:
        return math.floor(x * 1e8 + 0.5) / 1e8
    return -(math.floor(-x * 1e8 + 0.5) / 1e8)


def read_ticks(path: str | Path) -> pl.DataFrame:
    """Read canonical ticks from a replay file, sorted by time.

    NDJSON (`.ndjson`/`.jsonl`) is the cross-language contract; Parquet is a
    columnar sink for large captures. The format is chosen by file extension.
    """
    p = str(path)
    df = pl.read_parquet(p) if p.endswith(".parquet") else pl.read_ndjson(p)
    return df.sort("ts_ns")


def read_quotes(path: str | Path) -> pl.DataFrame:
    """Read canonical top-of-book quotes from an NDJSON replay, sorted by time."""
    p = str(path)
    df = pl.read_parquet(p) if p.endswith(".parquet") else pl.read_ndjson(p)
    return df.sort("ts_ns")


def read_book(path: str | Path) -> pl.DataFrame:
    """Read canonical L2 order-book levels from an NDJSON depth replay."""
    p = str(path)
    df = pl.read_parquet(p) if p.endswith(".parquet") else pl.read_ndjson(p)
    return df.sort("ts_ns", "side", "level")


def write_ticks(df: pl.DataFrame, path: str | Path) -> int:
    """Persist canonical ticks as Parquet (`.parquet`) or NDJSON by extension.

    Parquet is a compact columnar sink for large captures; NDJSON stays the
    portable replay the Rust engine also reads. Returns the row count written.
    """
    p = str(path)
    if p.endswith(".parquet"):
        df.write_parquet(p)
    else:
        df.write_ndjson(p)
    return df.height


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


def order_flow(df: pl.DataFrame) -> tuple[float, float, float]:
    """Buy volume, sell volume, and order-flow imbalance over all ticks.

    Imbalance is ``(buy - sell) / (buy + sell)`` in ``[-1, 1]``: positive means
    buy-initiated trades dominated the flow. Mirrors the Rust ``order_flow``.
    """
    row = df.select(
        pl.col("size").filter(pl.col("side") == "buy").sum().alias("buy"),
        pl.col("size").filter(pl.col("side") == "sell").sum().alias("sell"),
    )
    buy = row["buy"][0] or 0.0
    sell = row["sell"][0] or 0.0
    total = buy + sell
    imbalance = 0.0 if total == 0 else (buy - sell) / total
    return _r8(buy), _r8(sell), _r8(imbalance)


def quote_metrics(df: pl.DataFrame, product: str) -> dict:
    """Session top-of-book microstructure metrics, rounded to match Rust.

    Every field is the mean of a per-quote quantity (see the Rust
    ``QuoteSummary`` doc): average spread, mid, size-weighted microprice, and
    book imbalance ``(bid_size - ask_size) / (bid_size + ask_size)`` in
    ``[-1, 1]``. Mirrors the Rust ``quote_metrics`` expression for expression.
    """
    if df.height == 0:
        raise ValueError("no quotes")
    depth = pl.col("bid_size") + pl.col("ask_size")
    row = df.select(
        (pl.col("ask") - pl.col("bid")).mean().alias("avg_spread"),
        ((pl.col("bid") + pl.col("ask")) / 2.0).mean().alias("avg_mid"),
        ((pl.col("bid") * pl.col("ask_size") + pl.col("ask") * pl.col("bid_size")) / depth)
        .mean()
        .alias("avg_microprice"),
        ((pl.col("bid_size") - pl.col("ask_size")) / depth).mean().alias("avg_book_imbalance"),
    )
    return {
        "product": product,
        "quotes": df.height,
        "avg_spread": _r8(row["avg_spread"][0]),
        "avg_mid": _r8(row["avg_mid"][0]),
        "avg_microprice": _r8(row["avg_microprice"][0]),
        "avg_book_imbalance": _r8(row["avg_book_imbalance"][0]),
    }


def depth_metrics(df: pl.DataFrame, product: str) -> dict:
    """Session L2 depth microstructure metrics, rounded to match Rust.

    Stage 1 collapses each snapshot (rows sharing a ``ts_ns``) to its resting
    depth per side and its top-of-book spread; stage 2 averages those over the
    window. Mirrors the Rust ``depth_metrics`` expression for expression -- the
    ``.sort("ts_ns")`` fixes the summation order so the means are bit-identical.
    See the Rust ``DepthSummary`` doc for the field meanings.
    """
    if df.height == 0:
        raise ValueError("no book levels")
    per_snap = (
        df.group_by("ts_ns")
        .agg(
            pl.col("size").filter(pl.col("side") == "bid").sum().alias("bid_depth"),
            pl.col("size").filter(pl.col("side") == "ask").sum().alias("ask_depth"),
            pl.col("price")
            .filter((pl.col("side") == "bid") & (pl.col("level") == 0))
            .first()
            .alias("best_bid"),
            pl.col("price")
            .filter((pl.col("side") == "ask") & (pl.col("level") == 0))
            .first()
            .alias("best_ask"),
        )
        .sort("ts_ns")
    )
    bid_depth = pl.col("bid_depth")
    ask_depth = pl.col("ask_depth")
    row = per_snap.select(
        bid_depth.mean().alias("avg_bid_depth"),
        ask_depth.mean().alias("avg_ask_depth"),
        ((bid_depth - ask_depth) / (bid_depth + ask_depth)).mean().alias("avg_depth_imbalance"),
        (pl.col("best_ask") - pl.col("best_bid")).mean().alias("avg_spread"),
    )
    return {
        "product": product,
        "snapshots": per_snap.height,
        "avg_bid_depth": _r8(row["avg_bid_depth"][0]),
        "avg_ask_depth": _r8(row["avg_ask_depth"][0]),
        "avg_depth_imbalance": _r8(row["avg_depth_imbalance"][0]),
        "avg_spread": _r8(row["avg_spread"][0]),
    }


def summary(df: pl.DataFrame, product: str, bucket_ns: int) -> dict:
    """Full summary: session VWAP + TWAP + order-flow imbalance + per-bucket
    bars, rounded to match Rust."""
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
    buy_volume, sell_volume, imbalance = order_flow(df)
    return {
        "product": product,
        "bucket_ns": bucket_ns,
        "ticks": df.height,
        "vwap": session_vwap(df),
        "twap": session_twap(df),
        "buy_volume": buy_volume,
        "sell_volume": sell_volume,
        "imbalance": imbalance,
        "bars": bar_rows,
    }
