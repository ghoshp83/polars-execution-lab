"""Execution-analytics core, computed with Polars.

Every function here has a byte-for-byte counterpart in the Rust crate
(`src/execution.rs`). The two are held identical by `test_equivalence.py`,
which runs the compiled Rust binary and this module on the same replay and
asserts the summaries match exactly.
"""

from __future__ import annotations

import json
import math
from pathlib import Path

import polars as pl

TICK_COLUMNS = ("ts_ns", "product", "price", "size", "side", "trade_id")
QUOTE_COLUMNS = ("ts_ns", "product", "bid", "bid_size", "ask", "ask_size")
BOOK_LEVEL_COLUMNS = ("ts_ns", "product", "side", "level", "price", "size")
IMPACT_SLICE_COLUMNS = ("ts_ns", "product", "participation")
CALIBRATION_SAMPLE_COLUMNS = ("ts_ns", "product", "participation", "realised_bps")


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


def read_impact(path: str | Path) -> pl.DataFrame:
    """Read canonical execution-schedule slices from an NDJSON impact replay."""
    p = str(path)
    df = pl.read_parquet(p) if p.endswith(".parquet") else pl.read_ndjson(p)
    return df.sort("ts_ns")


def read_calibration(path: str | Path) -> pl.DataFrame:
    """Read canonical realised-fill calibration samples from an NDJSON replay."""
    p = str(path)
    df = pl.read_parquet(p) if p.endswith(".parquet") else pl.read_ndjson(p)
    return df.sort("ts_ns")


def read_fills(path: str | Path) -> pl.DataFrame:
    """Read canonical realised child fills from an NDJSON fill replay."""
    p = str(path)
    df = pl.read_parquet(p) if p.endswith(".parquet") else pl.read_ndjson(p)
    return df.sort("ts_ns")


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


def _fold_chunk(rows: list[dict], acc: dict) -> None:
    """Reduce one chunk of ticks into the accumulator with the Polars engine.

    The price/size/side sums are the same expressions ``session_vwap`` and
    ``order_flow`` use over the whole frame; summing chunk sums is what makes
    the fold associative. Mirrors the Rust ``fold_chunk``.
    """
    chunk = pl.DataFrame(
        {
            "price": [float(r["price"]) for r in rows],
            "size": [float(r["size"]) for r in rows],
            "side": [str(r["side"]) for r in rows],
        }
    )
    out = chunk.lazy().select(
        (pl.col("price") * pl.col("size")).sum().alias("pv"),
        pl.col("size").sum().alias("v"),
        pl.col("size").filter(pl.col("side") == "buy").sum().alias("buy"),
        pl.col("size").filter(pl.col("side") == "sell").sum().alias("sell"),
    )
    row = out.collect()
    acc["pv"] += row["pv"][0] or 0.0
    acc["volume"] += row["v"][0] or 0.0
    acc["buy"] += row["buy"][0] or 0.0
    acc["sell"] += row["sell"][0] or 0.0


def stream_session(path: str | Path, chunk_rows: int) -> dict:
    """Fold a session's benchmarks out of an NDJSON capture in bounded memory.

    Every other reader in this module materialises the whole file. That is fine
    for a fixture and wrong for a real capture: a day of trades on a liquid
    product does not fit the machine that wants the VWAP of it. So the file is
    folded in chunks of ``chunk_rows`` -- each aggregated with Polars and
    reduced into a running accumulator -- and only one chunk plus one carried
    tick is ever resident. ``peak_rows_in_memory`` reports that bound.

    The result is the same session VWAP, TWAP and order flow the in-memory pass
    computes: the fold is a different route to the answer, not a different
    answer. A streamed pass cannot sort what it has not seen, so a capture that
    is not already in time order is refused rather than silently mis-priced.

    Mirrors the Rust ``stream_session`` operation for operation.
    """
    if chunk_rows < 1:
        raise ValueError("chunk_rows must be >= 1")
    name = str(path)
    if not (name.endswith(".ndjson") or name.endswith(".jsonl")):
        raise ValueError(f"streaming reads NDJSON only, got {name}")

    acc = {"pv": 0.0, "volume": 0.0, "buy": 0.0, "sell": 0.0}
    twap_num = 0.0
    twap_den = 0.0
    buf: list[dict] = []
    rows = 0
    chunks = 0
    product = ""
    first_ts = 0
    # The last tick of the previous chunk: its sample-and-hold interval runs
    # into the next chunk, so it is carried across the boundary rather than
    # dropped. Without it a chunked TWAP would silently lose one interval per
    # boundary and drift away from the in-memory answer.
    carry: dict | None = None

    with open(name, encoding="utf-8") as fh:
        for i, line in enumerate(fh):
            trimmed = line.strip()
            if not trimmed:
                continue
            try:
                tick = json.loads(trimmed)
            except json.JSONDecodeError as exc:
                raise ValueError(f"parsing record on line {i + 1}") from exc
            ts = int(tick["ts_ns"])
            if rows == 0:
                product = str(tick["product"])
                first_ts = ts
            if carry is not None:
                prev_ts = int(carry["ts_ns"])
                if ts < prev_ts:
                    raise ValueError(
                        f"replay is not in time order at row {rows + 1}: {ts} follows {prev_ts}"
                    )
                dt = float(ts - prev_ts)
                twap_num += float(carry["price"]) * dt
                twap_den += dt
            carry = tick
            buf.append(tick)
            rows += 1
            if len(buf) == chunk_rows:
                _fold_chunk(buf, acc)
                chunks += 1
                buf = []
    if buf:
        _fold_chunk(buf, acc)
        chunks += 1

    if rows == 0:
        raise ValueError(f"no ticks in {name}")
    if rows < 2:
        raise ValueError("need >= 2 ticks for twap")
    if acc["volume"] == 0:
        raise ValueError("zero total size")
    if twap_den == 0:
        raise ValueError("zero elapsed time")

    total = acc["buy"] + acc["sell"]
    imbalance = 0.0 if total == 0 else (acc["buy"] - acc["sell"]) / total
    last_ts = int(carry["ts_ns"]) if carry is not None else first_ts
    peak = min(chunk_rows, rows) + (1 if chunks > 1 else 0)
    return {
        "product": product,
        "rows": rows,
        "chunks": chunks,
        "chunk_rows": chunk_rows,
        "peak_rows_in_memory": peak,
        "first_ts_ns": first_ts,
        "last_ts_ns": last_ts,
        "volume": _r8(acc["volume"]),
        "notional": _r8(acc["pv"]),
        "vwap": _r8(acc["pv"] / acc["volume"]),
        "twap": _r8(twap_num / twap_den),
        "buy_volume": _r8(acc["buy"]),
        "sell_volume": _r8(acc["sell"]),
        "imbalance": _r8(imbalance),
    }


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


def queue_metrics(df: pl.DataFrame, product: str) -> dict:
    """Session top-of-book queue-position metrics, rounded to match Rust.

    Reduces each snapshot to the resting size at the touch (``level == 0``) per
    side -- the queue a new passive order joins, which governs its fill priority
    -- then averages over the window. Deliberately distinct from
    :func:`depth_metrics`, which sums across *all* levels; this is the best level
    alone. Mirrors the Rust ``queue_metrics`` expression for expression -- the
    ``.sort("ts_ns")`` fixes the summation order so the means are bit-identical.
    See the Rust ``QueueSummary`` doc for the field meanings.
    """
    if df.height == 0:
        raise ValueError("no book levels")
    per_snap = (
        df.group_by("ts_ns")
        .agg(
            pl.col("size")
            .filter((pl.col("side") == "bid") & (pl.col("level") == 0))
            .first()
            .alias("bid_queue"),
            pl.col("size")
            .filter((pl.col("side") == "ask") & (pl.col("level") == 0))
            .first()
            .alias("ask_queue"),
        )
        .sort("ts_ns")
    )
    bid_queue = pl.col("bid_queue")
    ask_queue = pl.col("ask_queue")
    row = per_snap.select(
        bid_queue.mean().alias("avg_bid_queue"),
        ask_queue.mean().alias("avg_ask_queue"),
        ((bid_queue - ask_queue) / (bid_queue + ask_queue)).mean().alias("avg_queue_imbalance"),
    )
    return {
        "product": product,
        "snapshots": per_snap.height,
        "avg_bid_queue": _r8(row["avg_bid_queue"][0]),
        "avg_ask_queue": _r8(row["avg_ask_queue"][0]),
        "avg_queue_imbalance": _r8(row["avg_queue_imbalance"][0]),
    }


def sweep_cost(df: pl.DataFrame, product: str, side: str, order_size: float) -> dict:
    """Session book-sweep cost for a marketable order, rounded to match Rust.

    Stage 1 walks each snapshot's book in the order a taker meets it -- asks
    cheapest first for a ``buy``, bids dearest first for a ``sell`` -- allocating
    the order across levels via a running total of the size ahead of each level;
    stage 2 prices each sweep against the touch it started from and averages over
    the window. Mirrors the Rust ``sweep_cost`` expression for expression -- the
    sorts fix the consumption order so the means are bit-identical. See the Rust
    ``SweepSummary`` doc for the field meanings.
    """
    if df.height == 0:
        raise ValueError("no book levels")
    # NaN and infinity are rejected explicitly; the Rust side rejects the same set.
    if not math.isfinite(order_size) or order_size <= 0:
        raise ValueError("order_size must be a positive finite number")
    # The taker crosses the opposite side of the book.
    book_side = {"buy": "ask", "sell": "bid"}.get(side)
    if book_side is None:
        raise ValueError(f"side must be buy or sell, got {side}")

    cum_before = pl.col("size").cum_sum().over("ts_ns") - pl.col("size")
    remaining = order_size - cum_before
    alloc = (
        pl.when(remaining <= 0)
        .then(0.0)
        .otherwise(pl.when(remaining < pl.col("size")).then(remaining).otherwise(pl.col("size")))
        .alias("alloc")
    )
    per_snap = (
        df.filter(pl.col("side") == book_side)
        .sort(["ts_ns", "price"], descending=[False, side == "sell"])
        .with_columns(alloc)
        .group_by("ts_ns")
        .agg(
            (pl.col("alloc") * pl.col("price")).sum().alias("notional"),
            pl.col("alloc").sum().alias("filled"),
            (pl.col("alloc") > 0).sum().cast(pl.Float64).alias("levels_consumed"),
            pl.col("price").first().alias("touch"),
        )
        .sort("ts_ns")
    )
    vwap = pl.col("notional") / pl.col("filled")
    touch = pl.col("touch")
    # Signed so a larger number is always worse for the taker on either side.
    slippage_bps = (
        (vwap - touch) / touch * 10_000.0 if side == "buy" else (touch - vwap) / touch * 10_000.0
    )
    row = per_snap.select(
        vwap.mean().alias("avg_sweep_vwap"),
        slippage_bps.mean().alias("avg_slippage_bps"),
        pl.col("levels_consumed").mean().alias("avg_levels_consumed"),
        (pl.col("filled") / order_size).mean().alias("avg_fill_ratio"),
        # 1e-9 absorbs float-sum noise: a book that exactly covers the order
        # must count as filled, not miss by an ulp.
        (pl.col("filled") >= order_size - 1e-9).sum().alias("filled_snapshots"),
    )
    return {
        "product": product,
        "side": side,
        "order_size": order_size,
        "snapshots": per_snap.height,
        "filled_snapshots": int(row["filled_snapshots"][0]),
        "avg_sweep_vwap": _r8(row["avg_sweep_vwap"][0]),
        "avg_slippage_bps": _r8(row["avg_slippage_bps"][0]),
        "avg_levels_consumed": _r8(row["avg_levels_consumed"][0]),
        "avg_fill_ratio": _r8(row["avg_fill_ratio"][0]),
    }


def sweep_curve(df: pl.DataFrame, product: str, side: str, sizes: list[float]) -> dict:
    """Fit the square-root impact law to the book's own sweep costs.

    ``calibrate_impact`` recovers the impact coefficients from a desk's realised
    fills; this recovers the temporary coefficient from an L2 capture alone, for
    the venue or product where no fills exist yet. Each size in the ladder is
    swept through the book by :func:`sweep_cost`, expressed against the mean
    resting depth as a participation rate, and the concave law
    ``measured_bps = coef_bps * sqrt(participation)`` is fitted through the
    origin over the fully-filled points. A size the book cannot fill has an
    understated cost, so it is reported with its short ``fill_ratio`` but
    excluded from the fit. Mirrors the Rust ``sweep_curve`` operation for
    operation -- the sort fixes the summation order so the recovered
    coefficients are bit-identical. See the Rust ``SweepCurveSummary`` doc for
    the field meanings.
    """
    if df.height == 0:
        raise ValueError("no book levels")
    if len(sizes) < 2:
        raise ValueError("need at least two order sizes to fit a curve")
    # The taker crosses the opposite side of the book; sweep_cost validates the
    # side too, but the depth denominator below needs it up front.
    book_side = {"buy": "ask", "sell": "bid"}.get(side)
    if book_side is None:
        raise ValueError(f"side must be buy or sell, got {side}")
    # Validate before sorting: a NaN has no order, so it must be rejected here
    # rather than corrupting the ladder. sweep_cost rejects the same set.
    for q in sizes:
        if not math.isfinite(q) or q <= 0:
            raise ValueError(f"order sizes must be positive finite numbers, got {q}")
    ladder = sorted(float(q) for q in sizes)
    for a, b in zip(ladder, ladder[1:], strict=False):
        if a == b:
            raise ValueError(f"duplicate order size {a}")

    # Mean resting size per snapshot on the swept side: the denominator that
    # turns an absolute order size into a participation rate.
    depth = (
        df.filter(pl.col("side") == book_side)
        .group_by("ts_ns")
        .agg(pl.col("size").sum().alias("depth"))
        .sort("ts_ns")
        .select(
            pl.col("depth").mean().alias("avg_depth"),
            pl.col("depth").count().alias("snapshots"),
        )
    )
    avg_depth = depth["avg_depth"][0]
    if avg_depth is None:
        raise ValueError(f"no levels on the {book_side} side")
    snapshots = int(depth["snapshots"][0])

    # Sweep the book once per ladder rung. Each rung is a full walk of every
    # snapshot, so the measured cost is the book's own answer, not a model's.
    swept = [sweep_cost(df, product, side, q) for q in ladder]
    participation = [q / avg_depth for q in ladder]
    measured_bps = [m["avg_slippage_bps"] for m in swept]
    fill_ratio = [m["avg_fill_ratio"] for m in swept]

    # Sufficient statistics for the origin-through fit of y on the single
    # regressor x = sqrt(participation), over the fully-filled points only. The
    # 1e-9 matches the fill tolerance sweep_cost uses.
    stats = (
        pl.DataFrame(
            {
                "order_size": ladder,
                "participation": participation,
                "measured_bps": measured_bps,
                "fill_ratio": fill_ratio,
            }
        )
        .sort("order_size")
        .filter(pl.col("fill_ratio") >= 1.0 - 1e-9)
        .with_columns(pl.col("participation").sqrt().alias("x"))
        .select(
            (pl.col("x") * pl.col("x")).sum().alias("sxx"),
            (pl.col("x") * pl.col("measured_bps")).sum().alias("sxy"),
            (pl.col("measured_bps") * pl.col("measured_bps")).sum().alias("syy"),
            pl.col("measured_bps").sum().alias("sy"),
            pl.col("measured_bps").count().alias("n"),
        )
    )
    fitted_points = int(stats["n"][0])
    if fitted_points < 2:
        raise ValueError(
            "need at least two fully-filled order sizes to fit the curve; "
            "the book is too thin for this ladder"
        )
    sxx = stats["sxx"][0]
    sxy = stats["sxy"][0]
    syy = stats["syy"][0]
    sy = stats["sy"][0]
    n = float(fitted_points)
    if sxx <= 0:
        raise ValueError("degenerate ladder: every participation rate is zero")
    coef = sxy / sxx

    ss_res_raw = syy - 2.0 * coef * sxy + coef * coef * sxx
    ss_res = ss_res_raw if ss_res_raw > 0 else 0.0
    ybar = sy / n
    ss_tot = syy - n * ybar * ybar
    rmse = math.sqrt(ss_res / n)
    r_squared = 0.0 if ss_tot == 0 else 1.0 - ss_res / ss_tot

    curve = []
    for q, part, meas, fill in zip(ladder, participation, measured_bps, fill_ratio, strict=True):
        modelled = coef * math.sqrt(part)
        curve.append(
            {
                "order_size": q,
                "participation": _r8(part),
                "measured_bps": _r8(meas),
                "modelled_bps": _r8(modelled),
                "residual_bps": _r8(meas - modelled),
                "fill_ratio": _r8(fill),
                "fitted": fill >= 1.0 - 1e-9,
            }
        )
    return {
        "product": product,
        "side": side,
        "snapshots": snapshots,
        "avg_depth": _r8(avg_depth),
        "points": len(ladder),
        "fitted_points": fitted_points,
        "coef_bps": _r8(coef),
        "rmse_bps": _r8(rmse),
        "r_squared": _r8(r_squared),
        "curve": curve,
    }


# The urgency grid the optimiser searches: 0.0 to 4.0 in steps of 0.1, so index
# 0 is exactly a TWAP. Fixed rather than adaptive because the chosen point must
# be identical in both engines.
_URGENCY_STEPS = 41


def _plan_schedule(
    slices: int,
    total_size: float,
    per_slice_volume: float,
    coef_bps: float,
    perm_coef_bps: float,
    sigma_bps: float,
    urgency: float,
) -> dict:
    """Build and price the exponential-front-load trajectory for one urgency.

    Mirrors the Rust ``plan`` in ``src/schedule.rs`` operation for operation,
    including the sort that fixes the summation order.
    """
    n = float(slices)
    idx = list(range(slices))
    # Polars gates the Rust-side `exp` behind a feature the crate does not pull
    # in, so the decay itself is evaluated in the host and handed to the engine
    # as a column. Both languages call the platform ``exp``, so the weights stay
    # bit-identical.
    raw = [math.exp(-urgency * i / n) for i in idx]
    frame = (
        pl.DataFrame({"slice": idx, "raw": raw}, schema={"slice": pl.Int64, "raw": pl.Float64})
        .sort("slice")
        .with_columns((pl.col("raw") / pl.col("raw").sum()).alias("weight"))
        .with_columns((pl.col("weight") * total_size).alias("size"))
        .with_columns((pl.col("size") / per_slice_volume).alias("participation"))
        .with_columns(
            (pl.col("participation").sqrt() * coef_bps * pl.col("weight")).alias("temp_bps"),
            (pl.col("participation") * perm_coef_bps * pl.col("weight")).alias("perm_bps"),
            (1.0 - pl.col("weight").cum_sum()).alias("remaining"),
        )
    )
    totals = frame.select(
        pl.col("temp_bps").sum().alias("temp"),
        pl.col("perm_bps").sum().alias("perm"),
        (pl.col("remaining") * pl.col("remaining")).sum().alias("var"),
        pl.col("participation").max().alias("max_participation"),
    )
    impact_bps = totals["temp"][0] + totals["perm"][0]
    risk_bps = sigma_bps * math.sqrt(totals["var"][0] / n)
    return {
        "weight": frame["weight"].to_list(),
        "size": frame["size"].to_list(),
        "participation": frame["participation"].to_list(),
        "temp_bps": frame["temp_bps"].to_list(),
        "perm_bps": frame["perm_bps"].to_list(),
        "remaining": frame["remaining"].to_list(),
        "impact_bps": impact_bps,
        "risk_bps": risk_bps,
        "total_bps": impact_bps + risk_bps,
        "max_participation": totals["max_participation"][0],
    }


def optimal_schedule(
    product: str,
    slices: int,
    total_size: float,
    per_slice_volume: float,
    coef_bps: float,
    perm_coef_bps: float = 0.0,
    sigma_bps: float = 0.0,
) -> dict:
    """Search the urgency grid for the cheapest execution trajectory.

    The rest of the engine measures, calibrates and prices a schedule someone
    else chose; this one chooses it. A candidate is an exponential front-load
    with urgency ``k`` -- slice ``i`` of ``n`` weighted by ``exp(-k * i / n)``,
    normalised to sum to one, so ``k = 0`` is exactly a TWAP. Each is priced
    with the same two-term impact model the rest of the repo uses plus the
    Almgren-Chriss timing-risk term ``sigma_bps * sqrt(mean(remaining^2))``, and
    the urgency minimising the sum wins. Mirrors the Rust ``optimal_schedule``
    operation for operation, including the rounding of the objective and the
    first-wins tie-break. See the Rust ``ScheduleSummary`` doc for the field
    meanings.
    """
    if slices < 1:
        raise ValueError("need at least one slice")
    for name, v in (("total_size", total_size), ("per_slice_volume", per_slice_volume)):
        if not math.isfinite(v) or v <= 0:
            raise ValueError(f"{name} must be a positive finite number, got {v}")
    for name, v in (
        ("coef_bps", coef_bps),
        ("perm_coef_bps", perm_coef_bps),
        ("sigma_bps", sigma_bps),
    ):
        if not math.isfinite(v) or v < 0:
            raise ValueError(f"{name} must be a non-negative finite number, got {v}")

    args = (slices, total_size, per_slice_volume, coef_bps, perm_coef_bps, sigma_bps)
    twap = _plan_schedule(*args, 0.0)
    # A schedule that cannot be traded is not a cheap schedule. The TWAP is the
    # flattest candidate on the grid, so if even it overruns the interval's
    # volume no urgency can fit and the caller must reslice.
    if twap["max_participation"] > 1.0:
        raise ValueError(
            f"a uniform schedule takes {twap['max_participation']:.4f} of the volume "
            "available per slice; use more slices or a smaller order"
        )

    # First-wins on a tie over an ascending grid, so the least urgent schedule
    # is preferred when two are priced the same to 8dp.
    best = twap
    best_urgency = 0.0
    best_score = _r8(best["total_bps"])
    for step in range(1, _URGENCY_STEPS):
        urgency = step / 10.0
        cand = _plan_schedule(*args, urgency)
        # Front-loading past what the interval can absorb is infeasible, not
        # free: such candidates are skipped rather than silently clipped.
        if cand["max_participation"] > 1.0:
            continue
        score = _r8(cand["total_bps"])
        if score < best_score:
            best_score = score
            best_urgency = urgency
            best = cand

    schedule = [
        {
            "slice": i,
            "weight": _r8(best["weight"][i]),
            "size": _r8(best["size"][i]),
            "participation": _r8(best["participation"][i]),
            "temp_bps": _r8(best["temp_bps"][i]),
            "perm_bps": _r8(best["perm_bps"][i]),
            "remaining": _r8(best["remaining"][i]),
        }
        for i in range(slices)
    ]
    return {
        "product": product,
        "slices": slices,
        "total_size": _r8(total_size),
        "per_slice_volume": _r8(per_slice_volume),
        "coef_bps": _r8(coef_bps),
        "perm_coef_bps": _r8(perm_coef_bps),
        "sigma_bps": _r8(sigma_bps),
        "urgency": _r8(best_urgency),
        "impact_bps": _r8(best["impact_bps"]),
        "risk_bps": _r8(best["risk_bps"]),
        "total_bps": _r8(best["total_bps"]),
        "twap_impact_bps": _r8(twap["impact_bps"]),
        "twap_risk_bps": _r8(twap["risk_bps"]),
        "twap_total_bps": _r8(twap["total_bps"]),
        "saving_bps": _r8(twap["total_bps"] - best["total_bps"]),
        "schedule": schedule,
    }


def _volume_profile(df: pl.DataFrame, bucket_ns: int) -> pl.DataFrame:
    """Per-bucket volume and VWAP, ascending by bucket.

    The ticks are sorted first, exactly as ``bars`` does, so the within-bucket
    summation order does not depend on the order the capture arrived in.
    Mirrors the Rust ``volume_profile`` in ``src/pov.rs``.
    """
    return (
        df.sort("ts_ns")
        .with_columns(((pl.col("ts_ns") // bucket_ns) * bucket_ns).alias("bucket_ns"))
        .group_by("bucket_ns")
        .agg(
            pl.col("size").sum().alias("volume"),
            (pl.col("price") * pl.col("size")).sum().alias("pv"),
        )
        .sort("bucket_ns")
        .with_columns((pl.col("pv") / pl.col("volume")).alias("vwap"))
    )


def _validate_pov(
    bucket_ns: int, parent_qty: float, cap: float, coef_bps: float, perm_coef_bps: float
) -> None:
    """The argument checks every volume plan shares; mirrors the Rust ``validate``."""
    if bucket_ns < 1:
        raise ValueError(f"bucket_ns must be >= 1, got {bucket_ns}")
    if not math.isfinite(parent_qty) or parent_qty <= 0:
        raise ValueError(f"parent_qty must be a positive finite number, got {parent_qty}")
    if not math.isfinite(cap) or cap <= 0 or cap > 1:
        raise ValueError(f"cap must be in (0, 1], got {cap}")
    for name, v in (("coef_bps", coef_bps), ("perm_coef_bps", perm_coef_bps)):
        if not math.isfinite(v) or v < 0:
            raise ValueError(f"{name} must be a non-negative finite number, got {v}")


def _measured(df: pl.DataFrame, bucket_ns: int) -> tuple[pl.DataFrame, list[float], float]:
    """A capture's profile, per-bucket volumes and total, refusing one no plan
    can be priced on. Mirrors the Rust ``measured``."""
    if df.height == 0:
        raise ValueError("no ticks")
    profile = _volume_profile(df, bucket_ns)
    volume = profile["volume"].to_list()
    total_volume = sum(volume)
    if total_volume <= 0:
        raise ValueError("zero traded volume")
    # A bucket that traded nothing has no VWAP to execute at. Buckets only exist
    # where trades landed, so this means one whose trades all had zero size.
    for i, v in enumerate(volume):
        if v <= 0:
            raise ValueError(
                f"bucket {profile['bucket_ns'][i]} traded no volume and cannot be priced"
            )
    return profile, volume, total_volume


def _slots(profile: pl.DataFrame, bucket_ns: int) -> list[int]:
    """Bucket positions from the capture's first bucket: captures from different
    sessions line up by time into the session, not by wall clock."""
    bucket = profile["bucket_ns"].to_list()
    return [(b - bucket[0]) // bucket_ns for b in bucket]


def pov_schedule(
    df: pl.DataFrame,
    product: str,
    bucket_ns: int,
    parent_qty: float,
    cap: float,
    coef_bps: float,
    perm_coef_bps: float = 0.0,
) -> dict:
    """The schedule the capture's own volume implies.

    ``optimal_schedule`` chooses a trajectory against a single assumed
    ``per_slice_volume``. Real volume is not flat, and a clock-uniform slice
    dropped into a thin interval is a large share of a small market. This plan
    is derived from measured volume instead: the capture is bucketed, and each
    bucket gets the parent order in proportion to the volume it actually traded.

    Two properties follow from that allocation, and both are reported rather
    than assumed -- participation is ``parent_qty / total_volume`` in *every*
    bucket by construction, so the cap is a single scalar check with nothing to
    redistribute; and the achieved price is the volume-weighted mean of the
    bucket VWAPs, which is the session VWAP itself, so ``pov_tracking_bps`` is
    zero up to rounding.

    The clock-uniform allocation of the same quantity over the same buckets is
    priced alongside it as the benchmark, and has neither property. Mirrors the
    Rust ``pov_schedule`` operation for operation; see the Rust ``PovPlan`` doc
    for the field meanings.
    """
    _validate_pov(bucket_ns, parent_qty, cap, coef_bps, perm_coef_bps)
    profile, volume, total_volume = _measured(df, bucket_ns)

    participation = parent_qty / total_volume
    if participation > cap:
        raise ValueError(
            f"a volume-proportional schedule takes {participation:.4f} of the traded "
            f"volume, above the cap {cap:.4f}; use a smaller order or raise the cap"
        )

    n = profile.height
    priced = (
        profile.with_columns((pl.col("volume") / total_volume).alias("share"))
        .with_columns(
            (pl.col("share") * parent_qty).alias("size"),
            (parent_qty / n / pl.col("volume")).alias("twap_participation"),
        )
        .with_columns(
            (pl.col("share") * (coef_bps * math.sqrt(participation))).alias("temp_bps"),
            (pl.col("share") * (perm_coef_bps * participation)).alias("perm_bps"),
            (pl.col("share") * pl.col("vwap")).alias("pov_pv"),
            (pl.col("vwap") / n).alias("twap_pv"),
        )
        .with_columns(
            (pl.col("twap_participation").sqrt() * (coef_bps / n)).alias("twap_temp_bps"),
            (pl.col("twap_participation") * (perm_coef_bps / n)).alias("twap_perm_bps"),
        )
    )
    totals = priced.select(
        pl.col("temp_bps").sum().alias("temp"),
        pl.col("perm_bps").sum().alias("perm"),
        pl.col("pov_pv").sum().alias("pov_price"),
        pl.col("twap_pv").sum().alias("twap_price"),
        pl.col("twap_temp_bps").sum().alias("twap_temp"),
        pl.col("twap_perm_bps").sum().alias("twap_perm"),
        pl.col("twap_participation").max().alias("twap_max"),
    )

    session = session_vwap(df)
    pov_price = totals["pov_price"][0]
    twap_price = totals["twap_price"][0]
    pov_impact = totals["temp"][0] + totals["perm"][0]
    twap_impact = totals["twap_temp"][0] + totals["twap_perm"][0]
    twap_max = totals["twap_max"][0]

    schedule = [
        {
            "bucket_ns": priced["bucket_ns"][i],
            "volume": _r8(volume[i]),
            "volume_share": _r8(priced["share"][i]),
            "vwap": _r8(priced["vwap"][i]),
            "size": _r8(priced["size"][i]),
            "participation": _r8(participation),
            "temp_bps": _r8(priced["temp_bps"][i]),
            "perm_bps": _r8(priced["perm_bps"][i]),
        }
        for i in range(n)
    ]
    return {
        "product": product,
        "bucket_ns": bucket_ns,
        "buckets": n,
        "parent_qty": _r8(parent_qty),
        "cap": _r8(cap),
        "total_volume": _r8(total_volume),
        "participation": _r8(participation),
        "coef_bps": _r8(coef_bps),
        "perm_coef_bps": _r8(perm_coef_bps),
        "session_vwap": session,
        "pov_price": _r8(pov_price),
        "pov_tracking_bps": _r8((pov_price - session) / session * 1e4),
        "pov_impact_bps": _r8(pov_impact),
        "twap_price": _r8(twap_price),
        "twap_tracking_bps": _r8((twap_price - session) / session * 1e4),
        "twap_impact_bps": _r8(twap_impact),
        "twap_max_participation": _r8(twap_max),
        "twap_feasible": _r8(twap_max) <= _r8(cap),
        "edge_bps": _r8(twap_impact - pov_impact),
        "schedule": schedule,
    }


def pov_backtest(
    plan_df: pl.DataFrame,
    exec_df: pl.DataFrame,
    bucket_ns: int,
    parent_qty: float,
    cap: float,
    coef_bps: float,
    perm_coef_bps: float = 0.0,
) -> dict:
    """A volume plan, judged out of sample.

    ``pov_schedule``'s two properties -- constant participation and exact VWAP
    tracking -- hold only in the capture the plan was built from. This replays
    the allocation derived from ``plan_df`` against ``exec_df``, aligned by time
    into the session, and reports what the forecast error did.

    The comparison point is the oracle: the volume-following plan built on the
    execution capture itself. Under the two-term law, equal participation in
    every bucket minimises impact per unit of parent, so ``forecast_cost_bps`` is
    never negative -- reported, not assumed. A cap breach is reported
    (``feasible: False``) rather than refused. Mirrors the Rust ``pov_backtest``
    operation for operation; see the Rust ``PovBacktest`` doc for the fields.
    """
    _validate_pov(bucket_ns, parent_qty, cap, coef_bps, perm_coef_bps)
    try:
        plan_profile, plan_volume, plan_total = _measured(plan_df, bucket_ns)
    except ValueError as e:
        raise ValueError(f"plan capture: {e}") from e
    try:
        exec_profile, _, exec_total = _measured(exec_df, bucket_ns)
    except ValueError as e:
        raise ValueError(f"execution capture: {e}") from e

    product = exec_df["product"][0]
    if plan_df["product"][0] != product:
        raise ValueError(
            f"the plan capture is {plan_df['product'][0]} but the execution capture is {product}"
        )
    plan_slots = _slots(plan_profile, bucket_ns)
    exec_slots = _slots(exec_profile, bucket_ns)
    if plan_slots != exec_slots:
        raise ValueError(
            f"the captures cover different buckets: the plan traded in slots {plan_slots}, "
            f"the execution in {exec_slots}"
        )

    plan_share = [v / plan_total for v in plan_volume]
    session = session_vwap(exec_df)
    oracle_participation, oracle_impact = _oracle(parent_qty, exec_total, coef_bps, perm_coef_bps)
    priced = _price_plan(
        plan_share, exec_slots, exec_profile, exec_total, parent_qty, coef_bps, perm_coef_bps
    )
    score = _score(priced, session, cap, oracle_impact)
    return {
        "product": product,
        "bucket_ns": bucket_ns,
        "buckets": len(exec_slots),
        "parent_qty": _r8(parent_qty),
        "cap": _r8(cap),
        "coef_bps": _r8(coef_bps),
        "perm_coef_bps": _r8(perm_coef_bps),
        "plan_volume": _r8(plan_total),
        "exec_volume": _r8(exec_total),
        "profile_distance": score["profile_distance"],
        "session_vwap": session,
        "price": score["price"],
        "tracking_bps": score["tracking_bps"],
        "impact_bps": score["impact_bps"],
        "max_participation": score["max_participation"],
        "feasible": score["feasible"],
        "oracle_participation": _r8(oracle_participation),
        "oracle_impact_bps": _r8(oracle_impact),
        "oracle_feasible": _r8(oracle_participation) <= _r8(cap),
        "forecast_cost_bps": score["forecast_cost_bps"],
        "schedule": priced["schedule"],
    }


def _oracle(
    parent_qty: float, exec_total: float, coef_bps: float, perm_coef_bps: float
) -> tuple[float, float]:
    """The oracle's participation and impact, in closed form; mirrors the Rust ``oracle``."""
    participation = parent_qty / exec_total
    impact = coef_bps * math.sqrt(participation) + perm_coef_bps * participation
    return participation, impact


def _price_plan(
    plan_share: list[float],
    exec_slots: list[int],
    exec_profile: pl.DataFrame,
    exec_total: float,
    parent_qty: float,
    coef_bps: float,
    perm_coef_bps: float,
) -> dict:
    """Price an allocation -- a share of the parent per slot -- against the session
    it met. Every out-of-sample comparison goes through here, so no two of them
    can charge the same plan differently. Mirrors the Rust ``price_plan``."""
    n = len(exec_slots)
    priced = (
        pl.DataFrame(
            {
                "slot": pl.Series(exec_slots, dtype=pl.Int64),
                "plan_share": pl.Series(plan_share, dtype=pl.Float64),
                "exec_volume": exec_profile["volume"],
                "vwap": exec_profile["vwap"],
            }
        )
        .with_columns((pl.col("exec_volume") / exec_total).alias("exec_share"))
        .with_columns((pl.col("plan_share") * parent_qty).alias("size"))
        .with_columns((pl.col("size") / pl.col("exec_volume")).alias("participation"))
        .with_columns(
            (pl.col("plan_share") * pl.col("participation").sqrt() * coef_bps).alias("temp_bps"),
            (pl.col("plan_share") * pl.col("participation") * perm_coef_bps).alias("perm_bps"),
            (pl.col("plan_share") * pl.col("vwap")).alias("pv"),
            (pl.col("plan_share") - pl.col("exec_share")).alias("share_diff"),
        )
    )
    totals = priced.select(
        pl.col("temp_bps").sum().alias("temp"),
        pl.col("perm_bps").sum().alias("perm"),
        pl.col("pv").sum().alias("price"),
        pl.col("participation").max().alias("max_participation"),
        pl.col("share_diff").abs().sum().alias("share_gap"),
    )

    schedule = [
        {
            "slot": priced["slot"][i],
            "plan_share": _r8(priced["plan_share"][i]),
            "exec_share": _r8(priced["exec_share"][i]),
            "exec_volume": _r8(priced["exec_volume"][i]),
            "vwap": _r8(priced["vwap"][i]),
            "size": _r8(priced["size"][i]),
            "participation": _r8(priced["participation"][i]),
            "temp_bps": _r8(priced["temp_bps"][i]),
            "perm_bps": _r8(priced["perm_bps"][i]),
        }
        for i in range(n)
    ]
    return {
        "price": totals["price"][0],
        "impact": totals["temp"][0] + totals["perm"][0],
        "max_participation": totals["max_participation"][0],
        "share_gap": totals["share_gap"][0],
        "schedule": schedule,
    }


def _score(priced: dict, session: float, cap: float, oracle_impact: float) -> dict:
    """The reported, rounded verdict on one priced allocation; mirrors the Rust ``score``."""
    price = priced["price"]
    return {
        "profile_distance": _r8(priced["share_gap"] / 2.0),
        "price": _r8(price),
        "tracking_bps": _r8((price - session) / session * 1e4),
        "impact_bps": _r8(priced["impact"]),
        "max_participation": _r8(priced["max_participation"]),
        "feasible": _r8(priced["max_participation"]) <= _r8(cap),
        "forecast_cost_bps": _r8(priced["impact"] - oracle_impact),
    }


def shortfall(
    df: pl.DataFrame,
    product: str,
    parent_qty: float,
    arrival_price: float,
    coef_bps: float,
    perm_coef_bps: float = 0.0,
) -> dict:
    """Post-trade attribution: what the execution paid, and what the model expected.

    ``optimal_schedule`` chooses a trajectory before the order goes out; this is
    the other half of that loop, run after the fills come back. The realised
    implementation shortfall of the filled quantity is measured against the
    arrival (decision) price, then each fill is priced *again* through the same
    two-term law the rest of this engine uses --
    ``coef_bps * sqrt(participation)`` temporary plus
    ``perm_coef_bps * participation`` permanent -- and the two are differenced.

    The difference is the number worth reading. ``modelled_bps`` is the cost the
    order was always going to pay for its size; ``residual_bps`` is what is left
    -- venue selection, timing, spread capture, adverse selection, luck. Judging
    an execution on ``realised_bps`` alone rewards whoever was given the small
    orders.

    Quantity that never filled is charged as ``opportunity_bps``, so an
    algorithm that improves its average price by simply not finishing does not
    come out ahead. ``realised_bps`` is quoted on the filled notional;
    ``total_bps`` is quoted on the parent, and is the honest headline.

    Mirrors ``shortfall`` in ``src/shortfall.rs`` operation for operation.
    """
    if df.height == 0:
        raise ValueError("no fills")
    for name, v in (("parent_qty", parent_qty), ("arrival_price", arrival_price)):
        if not math.isfinite(v) or v <= 0.0:
            raise ValueError(f"{name} must be a positive finite number, got {v}")
    for name, v in (("coef_bps", coef_bps), ("perm_coef_bps", perm_coef_bps)):
        if not math.isfinite(v) or v < 0.0:
            raise ValueError(f"{name} must be a non-negative finite number, got {v}")

    # A parent order has one side. Mixed sides in one file is a data error, not
    # a netting instruction -- signing the shortfall would be meaningless.
    side = str(df["side"][0])
    if side not in ("buy", "sell"):
        raise ValueError(f"side must be buy or sell, got {side}")
    for row in df.iter_rows(named=True):
        if row["side"] != side:
            raise ValueError(
                f"fills mix sides ({side} and {row['side']}); one parent order has one side"
            )
        if not math.isfinite(row["qty"]) or row["qty"] <= 0.0:
            raise ValueError(f"fill qty must be positive and finite, got {row['qty']}")
        if not math.isfinite(row["price"]) or row["price"] <= 0.0:
            raise ValueError(f"fill price must be positive and finite, got {row['price']}")
        if not math.isfinite(row["interval_volume"]) or row["interval_volume"] <= 0.0:
            raise ValueError(
                f"interval_volume must be positive and finite, got {row['interval_volume']}"
            )
        if row["qty"] > row["interval_volume"]:
            raise ValueError(
                f"a fill of {row['qty']} took more than the {row['interval_volume']} "
                "available in its interval"
            )

    sign = 1.0 if side == "buy" else -1.0
    frame = df.select("ts_ns", "qty", "price", "interval_volume").sort("ts_ns")
    filled_qty = float(frame.select(pl.col("qty").sum()).item())
    # Filling more than the parent is a reconciliation error upstream; reporting
    # a fill rate above 1 would hide it.
    if filled_qty > parent_qty:
        raise ValueError(f"fills total {filled_qty} against a parent of {parent_qty}")

    priced = (
        frame.with_columns(
            (pl.col("qty") / pl.col("interval_volume")).alias("participation"),
            (pl.col("qty") / pl.lit(filled_qty)).alias("weight"),
            (
                (pl.col("price") - pl.lit(arrival_price))
                / pl.lit(arrival_price)
                * pl.lit(1e4)
                * pl.lit(sign)
            ).alias("realised_bps"),
        )
        .with_columns(
            (
                pl.col("participation").sqrt() * pl.lit(coef_bps)
                + pl.col("participation") * pl.lit(perm_coef_bps)
            ).alias("modelled_bps")
        )
        .with_columns((pl.col("realised_bps") - pl.col("modelled_bps")).alias("residual_bps"))
    )

    totals = priced.select(
        (pl.col("weight") * pl.col("realised_bps")).sum().alias("realised"),
        (pl.col("weight") * pl.col("modelled_bps")).sum().alias("modelled"),
        (pl.col("weight") * pl.col("price")).sum().alias("avg_price"),
        pl.col("price").last().alias("final_price"),
    )
    realised_bps = float(totals["realised"][0])
    modelled_bps = float(totals["modelled"][0])
    avg_price = float(totals["avg_price"][0])
    final_price = float(totals["final_price"][0])

    unfilled_qty = parent_qty - filled_qty
    fill_rate = filled_qty / parent_qty
    # The remainder is charged the drift it walked away from, weighted by how
    # much of the parent it was. A fully-filled parent pays nothing here.
    opportunity_bps = (
        (unfilled_qty / parent_qty) * sign * (final_price - arrival_price) / arrival_price * 1e4
    )
    total_bps = fill_rate * realised_bps + opportunity_bps

    slices = [
        {
            "ts_ns": row["ts_ns"],
            "qty": _r8(row["qty"]),
            "price": _r8(row["price"]),
            "participation": _r8(row["participation"]),
            "weight": _r8(row["weight"]),
            "realised_bps": _r8(row["realised_bps"]),
            "modelled_bps": _r8(row["modelled_bps"]),
            "residual_bps": _r8(row["residual_bps"]),
        }
        for row in priced.iter_rows(named=True)
    ]
    return {
        "product": product,
        "side": side,
        "fills": df.height,
        "parent_qty": _r8(parent_qty),
        "filled_qty": _r8(filled_qty),
        "unfilled_qty": _r8(unfilled_qty),
        "fill_rate": _r8(fill_rate),
        "arrival_price": _r8(arrival_price),
        "avg_price": _r8(avg_price),
        "final_price": _r8(final_price),
        "realised_bps": _r8(realised_bps),
        "modelled_bps": _r8(modelled_bps),
        "residual_bps": _r8(realised_bps - modelled_bps),
        "opportunity_bps": _r8(opportunity_bps),
        "total_bps": _r8(total_bps),
        "slices": slices,
    }


def counterfactual(
    df: pl.DataFrame,
    product: str,
    arrival_price: float,
    coef_bps: float,
    perm_coef_bps: float = 0.0,
) -> dict:
    """Counterfactual scheduling: was the realised schedule itself worth anything?

    ``shortfall`` splits a realised execution into the cost its size always
    implied and the cost it did not. That residual is a bucket, and the obvious
    next question is how much of it the *schedule* earned: would a plain TWAP, or
    a volume-following participation, have paid more or less over the very same
    intervals?

    Each benchmark is handed the quantity that actually filled and spread across
    the same intervals, priced on the same prices and the same traded volumes,
    through the same two-term law. Holding quantity fixed is what makes the
    comparison fair: the unfilled remainder is identical under every strategy, so
    its opportunity cost cancels and is deliberately not reported here.

    ``edge_bps`` is the headline -- ``best_alternative - realised``, so a
    positive number means the realised schedule beat the best simple benchmark.

    Mirrors ``counterfactual`` in ``src/counterfactual.rs`` operation for
    operation.
    """
    if df.height == 0:
        raise ValueError("no fills")
    if not math.isfinite(arrival_price) or arrival_price <= 0.0:
        raise ValueError(f"arrival_price must be a positive finite number, got {arrival_price}")
    for name, v in (("coef_bps", coef_bps), ("perm_coef_bps", perm_coef_bps)):
        if not math.isfinite(v) or v < 0.0:
            raise ValueError(f"{name} must be a non-negative finite number, got {v}")

    # Same rule as post-trade attribution: one parent order has one side.
    side = str(df["side"][0])
    if side not in ("buy", "sell"):
        raise ValueError(f"side must be buy or sell, got {side}")
    for row in df.iter_rows(named=True):
        if row["side"] != side:
            raise ValueError(
                f"fills mix sides ({side} and {row['side']}); one parent order has one side"
            )
        if not math.isfinite(row["qty"]) or row["qty"] <= 0.0:
            raise ValueError(f"fill qty must be positive and finite, got {row['qty']}")
        if not math.isfinite(row["price"]) or row["price"] <= 0.0:
            raise ValueError(f"fill price must be positive and finite, got {row['price']}")
        if not math.isfinite(row["interval_volume"]) or row["interval_volume"] <= 0.0:
            raise ValueError(
                f"interval_volume must be positive and finite, got {row['interval_volume']}"
            )
        if row["qty"] > row["interval_volume"]:
            raise ValueError(
                f"a fill of {row['qty']} took more than the {row['interval_volume']} "
                "available in its interval"
            )

    sign = 1.0 if side == "buy" else -1.0
    base = df.select("ts_ns", "qty", "price", "interval_volume").sort("ts_ns")
    agg = base.select(
        pl.col("qty").sum().alias("filled_qty"),
        pl.col("interval_volume").sum().alias("total_volume"),
    )
    filled_qty = float(agg["filled_qty"][0])
    total_volume = float(agg["total_volume"][0])
    n = float(base.height)

    # Every strategy trades the same quantity over the same intervals. The
    # unfilled remainder is identical under all of them, so it cancels.
    frame = base.with_columns(
        pl.lit(filled_qty / n).alias("twap_qty"),
        (pl.col("interval_volume") / pl.lit(total_volume) * pl.lit(filled_qty)).alias("volume_qty"),
    )

    def price_strategy(qty_col: str, name: str) -> dict:
        priced = (
            frame.select(
                "ts_ns",
                pl.col(qty_col).alias("qty"),
                "price",
                "interval_volume",
            )
            .with_columns(
                (pl.col("qty") / pl.col("interval_volume")).alias("participation"),
                (pl.col("qty") / pl.lit(filled_qty)).alias("weight"),
                (
                    (pl.col("price") - pl.lit(arrival_price))
                    / pl.lit(arrival_price)
                    * pl.lit(1e4)
                    * pl.lit(sign)
                ).alias("drift_bps"),
            )
            .with_columns(
                (
                    pl.col("participation").sqrt() * pl.lit(coef_bps)
                    + pl.col("participation") * pl.lit(perm_coef_bps)
                ).alias("impact_bps")
            )
            .with_columns((pl.col("drift_bps") + pl.col("impact_bps")).alias("cost_bps"))
        )
        # Pricing a counterfactual that takes more than the interval ever traded
        # would be fiction: the impact law is not defined above full participation.
        max_participation = float(priced.select(pl.col("participation").max()).item())
        if max_participation > 1.0:
            raise ValueError(
                f"the {name} allocation would take {max_participation} of an interval's "
                "volume; refusing to price a counterfactual the market could not have filled"
            )
        totals = priced.select(
            (pl.col("weight") * pl.col("drift_bps")).sum().alias("drift"),
            (pl.col("weight") * pl.col("impact_bps")).sum().alias("impact"),
            pl.col("qty").sum().alias("qty"),
        )
        drift_bps = float(totals["drift"][0])
        impact_bps = float(totals["impact"][0])
        return {
            "name": name,
            "qty": _r8(float(totals["qty"][0])),
            "drift_bps": _r8(drift_bps),
            "impact_bps": _r8(impact_bps),
            "cost_bps": _r8(drift_bps + impact_bps),
            "legs": [
                {
                    "ts_ns": row["ts_ns"],
                    "qty": _r8(row["qty"]),
                    "price": _r8(row["price"]),
                    "interval_volume": _r8(row["interval_volume"]),
                    "participation": _r8(row["participation"]),
                    "weight": _r8(row["weight"]),
                    "drift_bps": _r8(row["drift_bps"]),
                    "impact_bps": _r8(row["impact_bps"]),
                    "cost_bps": _r8(row["cost_bps"]),
                }
                for row in priced.iter_rows(named=True)
            ],
        }

    realised = price_strategy("qty", "realised")
    alternatives = [price_strategy("twap_qty", "twap"), price_strategy("volume_qty", "volume")]
    best = min(alternatives, key=lambda a: a["cost_bps"])
    return {
        "product": product,
        "side": side,
        "intervals": df.height,
        "filled_qty": _r8(filled_qty),
        "arrival_price": _r8(arrival_price),
        "realised": realised,
        "alternatives": alternatives,
        "best_alternative": best["name"],
        "edge_bps": _r8(best["cost_bps"] - realised["cost_bps"]),
    }


def sensitivity(
    df: pl.DataFrame,
    product: str,
    arrival_price: float,
    coef_grid: list[float],
    perm_coef_bps: float = 0.0,
) -> dict:
    """Does the counterfactual verdict survive the coefficient it was priced with?

    ``counterfactual`` reports ``edge_bps`` as though the temporary-impact
    coefficient were known. It is not: it is fitted, by ``calibrate_impact`` or
    ``sweep_curve``, from noisy data. A desk told "your schedule lost 0.8bps to
    volume-following" should be able to ask whether that verdict is a property of
    the execution or of the number that was fed in.

    So the same comparison is re-run across a grid of ``coef_bps`` and the
    answers are lined up. ``verdict_stable`` is the headline: it is true only
    when every point in the grid picks the same best alternative *and* agrees on
    the sign of the edge. When the sign does flip, ``breakeven_coef_bps`` is the
    coefficient at which it does -- and because the edge is affine in
    ``coef_bps`` (drift does not depend on it, and both impact terms are linear
    in their coefficients), interpolating between the two bracketing grid points
    is exact rather than approximate.

    ``perm_coef_bps`` is held fixed: this sweeps one axis, not the plane.

    Mirrors ``sensitivity`` in ``src/sensitivity.rs`` operation for operation.
    """
    # One point is not a sensitivity, and an unsorted grid would make the
    # bracketing interpolation meaningless.
    if len(coef_grid) < 2:
        raise ValueError(f"coef_grid needs at least 2 points, got {len(coef_grid)}")
    for v in coef_grid:
        if not math.isfinite(v) or v < 0.0:
            raise ValueError(f"coef_grid values must be non-negative and finite, got {v}")
    for a, b in zip(coef_grid, coef_grid[1:], strict=False):
        if b <= a:
            raise ValueError(f"coef_grid must be strictly increasing, got {a} then {b}")

    reports = [counterfactual(df, product, arrival_price, c, perm_coef_bps) for c in coef_grid]
    points = [
        {
            "coef_bps": _r8(c),
            "realised_cost_bps": r["realised"]["cost_bps"],
            "best_alternative": r["best_alternative"],
            "best_cost_bps": min(a["cost_bps"] for a in r["alternatives"]),
            "edge_bps": r["edge_bps"],
        }
        for c, r in zip(coef_grid, reports, strict=True)
    ]

    grid = pl.DataFrame(
        {
            "coef_bps": [p["coef_bps"] for p in points],
            "edge_bps": [p["edge_bps"] for p in points],
        }
    )
    agg = grid.select(
        pl.col("edge_bps").min().alias("edge_min"),
        pl.col("edge_bps").max().alias("edge_max"),
    )
    edge_min = float(agg["edge_min"][0])
    edge_max = float(agg["edge_max"][0])

    # A sign flip between adjacent points is where the verdict changes hands.
    # An edge of exactly zero is a tie, and a tie is not a stable verdict either.
    sign_flips = 0
    breakeven: float | None = None
    for a, b in zip(points, points[1:], strict=False):
        if a["edge_bps"] * b["edge_bps"] < 0.0:
            sign_flips += 1
            if breakeven is None:
                span = b["coef_bps"] - a["coef_bps"]
                breakeven = a["coef_bps"] + span * (-a["edge_bps"]) / (
                    b["edge_bps"] - a["edge_bps"]
                )
    for p in points:
        if p["edge_bps"] == 0.0 and breakeven is None:
            breakeven = p["coef_bps"]

    names = {p["best_alternative"] for p in points}
    stable = len(names) == 1 and (edge_min > 0.0 or edge_max < 0.0)

    return {
        "product": product,
        "side": str(df["side"][0]),
        "intervals": df.height,
        "arrival_price": _r8(arrival_price),
        "perm_coef_bps": _r8(perm_coef_bps),
        "points": points,
        "edge_min_bps": _r8(edge_min),
        "edge_max_bps": _r8(edge_max),
        "sign_flips": sign_flips,
        "verdict_stable": stable,
        "breakeven_coef_bps": None if breakeven is None else _r8(breakeven),
    }


def impact_curve(
    df: pl.DataFrame, product: str, coef_bps: float, perm_coef_bps: float = 0.0
) -> dict:
    """Session market-impact cost curve, priced under two-term Almgren-Chriss.

    Each slice pays a **temporary** cost following the concave square-root law
    ``temp_bps = coef_bps * sqrt(participation)`` (transient, dissipates after
    the slice) and a **permanent** cost linear in size,
    ``perm_bps = perm_coef_bps * participation`` (a lasting shift of the mid the
    schedule leaves behind). ``participation`` is the fraction of available
    volume the slice consumes; both coefficients are externally-calibrated
    constants. With ``perm_coef_bps == 0`` the permanent fields are zero and
    ``total_cost_bps`` collapses to ``total_impact_bps``. Mirrors the Rust
    ``impact_curve`` expression for expression -- the ``.sort("ts_ns")`` fixes
    the summation order so the means and totals are bit-identical across the two
    engines. See the Rust ``ImpactSummary`` doc for the field meanings.
    """
    if df.height == 0:
        raise ValueError("no impact slices")
    per_slice = df.sort("ts_ns").with_columns(
        (pl.col("participation").sqrt() * coef_bps).alias("impact_bps"),
        (pl.col("participation") * perm_coef_bps).alias("perm_impact_bps"),
    )
    row = per_slice.select(
        pl.col("impact_bps").mean().alias("avg_impact_bps"),
        pl.col("impact_bps").max().alias("max_impact_bps"),
        pl.col("impact_bps").sum().alias("total_impact_bps"),
        pl.col("perm_impact_bps").mean().alias("avg_perm_impact_bps"),
        pl.col("perm_impact_bps").sum().alias("total_perm_impact_bps"),
    )
    total_impact = row["total_impact_bps"][0]
    total_perm = row["total_perm_impact_bps"][0]
    return {
        "product": product,
        "slices": df.height,
        "coef_bps": _r8(coef_bps),
        "perm_coef_bps": _r8(perm_coef_bps),
        "avg_impact_bps": _r8(row["avg_impact_bps"][0]),
        "max_impact_bps": _r8(row["max_impact_bps"][0]),
        "total_impact_bps": _r8(total_impact),
        "avg_perm_impact_bps": _r8(row["avg_perm_impact_bps"][0]),
        "total_perm_impact_bps": _r8(total_perm),
        "total_cost_bps": _r8(total_impact + total_perm),
    }


def calibrate_impact(df: pl.DataFrame, product: str) -> dict:
    """Fit the two-term Almgren-Chriss coefficients from realised fills.

    ``impact_curve`` takes the two coefficients as inputs; this is where they
    come from. Each row is a realised fill: the fraction of volume it took and
    the cost it actually paid (bps vs the pre-trade benchmark). The model says
    that cost is ``coef_bps * sqrt(participation) + perm_coef_bps *
    participation``, so treating ``sqrt(participation)`` and ``participation`` as
    two regressors, the coefficients are the ordinary-least-squares fit through
    the origin (a zero-size fill costs nothing, so there is no intercept). Every
    quantity the normal equations need is a sum, so the fit is a Polars
    aggregation followed by a 2x2 solve. Mirrors the Rust ``calibrate_impact``
    sum for sum and operation for operation -- the ``.sort("ts_ns")`` fixes the
    summation order -- so both engines recover bit-identical coefficients. See
    the Rust ``CalibrationSummary`` doc for the field meanings.
    """
    if df.height == 0:
        raise ValueError("no calibration samples")
    sums = (
        df.sort("ts_ns")
        .with_columns(pl.col("participation").sqrt().alias("x1"))
        .select(
            pl.col("participation").sum().alias("s11"),
            (pl.col("x1") * pl.col("participation")).sum().alias("s12"),
            (pl.col("participation") * pl.col("participation")).sum().alias("s22"),
            (pl.col("x1") * pl.col("realised_bps")).sum().alias("b1"),
            (pl.col("participation") * pl.col("realised_bps")).sum().alias("b2"),
            (pl.col("realised_bps") * pl.col("realised_bps")).sum().alias("syy"),
            pl.col("realised_bps").sum().alias("sy"),
        )
    )
    s11 = sums["s11"][0]
    s12 = sums["s12"][0]
    s22 = sums["s22"][0]
    b1 = sums["b1"][0]
    b2 = sums["b2"][0]
    syy = sums["syy"][0]
    sy = sums["sy"][0]
    n = float(df.height)

    scale = s11 * s22
    det = scale - s12 * s12
    if abs(det) <= 1e-12 * scale:
        raise ValueError(
            "singular design: need at least two distinct participation levels to fit both terms"
        )
    coef = (s22 * b1 - s12 * b2) / det
    perm = (s11 * b2 - s12 * b1) / det

    ss_res_raw = (
        syy
        - 2.0 * coef * b1
        - 2.0 * perm * b2
        + coef * coef * s11
        + 2.0 * coef * perm * s12
        + perm * perm * s22
    )
    ss_res = ss_res_raw if ss_res_raw > 0.0 else 0.0
    ybar = sy / n
    ss_tot = syy - n * ybar * ybar
    rmse = math.sqrt(ss_res / n)
    r_squared = 0.0 if ss_tot == 0.0 else 1.0 - ss_res / ss_tot
    return {
        "product": product,
        "samples": df.height,
        "coef_bps": _r8(coef),
        "perm_coef_bps": _r8(perm),
        "rmse_bps": _r8(rmse),
        "r_squared": _r8(r_squared),
    }


def calibrate_impact_robust(
    df: pl.DataFrame,
    product: str,
    huber_delta: float | None = None,
    ridge_lambda: float = 0.0,
    max_iters: int = 8,
) -> dict:
    """Fit the coefficients robustly, down-weighting outliers and/or shrinking.

    ``calibrate_impact`` is plain least squares: one bad print pulls both
    coefficients toward itself, and a design clustered at one participation level
    barely separates the two basis functions. This variant adds the two standard
    defences, both as the same Polars sums so the fit stays bit-identical to the
    Rust ``calibrate_impact_robust``:

    - ``huber_delta`` -- iteratively reweighted least squares. Each pass solves
      the weighted normal equations, then re-weights every sample by
      ``min(1, delta / |residual|)`` (``delta`` in bps). ``None`` leaves every
      weight at 1 (ordinary least squares).
    - ``ridge_lambda`` -- adds ``ridge_lambda`` to the diagonal of the normal
      matrix, shrinking toward zero and making a single-participation-level
      design solvable. ``0.0`` leaves the fit unregularised.

    With ``huber_delta=None`` and ``ridge_lambda=0.0`` this reproduces
    ``calibrate_impact`` exactly. The reported ``rmse_bps`` / ``r_squared`` are
    always the unweighted fit quality against every sample.
    """
    if df.height == 0:
        raise ValueError("no calibration samples")
    work = df.sort("ts_ns").with_columns(
        pl.col("participation").sqrt().alias("x1"),
        pl.lit(1.0).alias("w"),
    )

    # Unweighted sufficient statistics, computed once (reported fit quality is
    # always against every sample, not the reweighted ones).
    udf = work.select(
        pl.col("participation").sum().alias("s11"),
        (pl.col("x1") * pl.col("participation")).sum().alias("s12"),
        (pl.col("participation") * pl.col("participation")).sum().alias("s22"),
        (pl.col("x1") * pl.col("realised_bps")).sum().alias("b1"),
        (pl.col("participation") * pl.col("realised_bps")).sum().alias("b2"),
        (pl.col("realised_bps") * pl.col("realised_bps")).sum().alias("syy"),
        pl.col("realised_bps").sum().alias("sy"),
    )
    s11u = udf["s11"][0]
    s12u = udf["s12"][0]
    s22u = udf["s22"][0]
    b1u = udf["b1"][0]
    b2u = udf["b2"][0]
    syy = udf["syy"][0]
    sy = udf["sy"][0]
    n = float(df.height)

    passes = max_iters if huber_delta is not None else 1
    coef = 0.0
    perm = 0.0
    for _ in range(passes):
        wsums = work.select(
            (pl.col("w") * pl.col("participation")).sum().alias("s11"),
            (pl.col("w") * pl.col("x1") * pl.col("participation")).sum().alias("s12"),
            (pl.col("w") * pl.col("participation") * pl.col("participation")).sum().alias("s22"),
            (pl.col("w") * pl.col("x1") * pl.col("realised_bps")).sum().alias("b1"),
            (pl.col("w") * pl.col("participation") * pl.col("realised_bps")).sum().alias("b2"),
        )
        s11 = wsums["s11"][0] + ridge_lambda
        s12 = wsums["s12"][0]
        s22 = wsums["s22"][0] + ridge_lambda
        b1 = wsums["b1"][0]
        b2 = wsums["b2"][0]

        scale = s11 * s22
        det = scale - s12 * s12
        if abs(det) <= 1e-12 * scale:
            raise ValueError(
                "singular design: need at least two distinct participation levels, "
                "or a non-zero ridge_lambda, to fit both terms"
            )
        coef = (s22 * b1 - s12 * b2) / det
        perm = (s11 * b2 - s12 * b1) / det

        if huber_delta is None:
            break
        resid = (
            pl.col("realised_bps")
            - (pl.lit(coef) * pl.col("x1") + pl.lit(perm) * pl.col("participation"))
        ).abs()
        work = work.with_columns(
            pl.when(resid <= pl.lit(huber_delta))
            .then(pl.lit(1.0))
            .otherwise(pl.lit(huber_delta) / resid)
            .alias("w")
        )

    ss_res_raw = (
        syy
        - 2.0 * coef * b1u
        - 2.0 * perm * b2u
        + coef * coef * s11u
        + 2.0 * coef * perm * s12u
        + perm * perm * s22u
    )
    ss_res = ss_res_raw if ss_res_raw > 0.0 else 0.0
    ybar = sy / n
    ss_tot = syy - n * ybar * ybar
    rmse = math.sqrt(ss_res / n)
    r_squared = 0.0 if ss_tot == 0.0 else 1.0 - ss_res / ss_tot
    return {
        "product": product,
        "samples": df.height,
        "coef_bps": _r8(coef),
        "perm_coef_bps": _r8(perm),
        "rmse_bps": _r8(rmse),
        "r_squared": _r8(r_squared),
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
