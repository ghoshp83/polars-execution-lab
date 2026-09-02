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
