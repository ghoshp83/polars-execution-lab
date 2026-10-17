"""Market-data ingest: a live Coinbase collector and a deterministic generator.

The live path is the real happy path -- it connects to Coinbase's public
WebSocket (no API key needed for market data) and normalizes each trade into
the canonical tick schema written to an NDJSON replay file. The synthetic
generator produces a deterministic replay for demos and offline runs.
"""

from __future__ import annotations

import json
import math
import random
from datetime import UTC, datetime, timedelta
from pathlib import Path

from .events import EventLog

COINBASE_WS = "wss://ws-feed.exchange.coinbase.com"
_EPOCH = datetime(1970, 1, 1, tzinfo=UTC)


def _iso_to_ns(iso: str) -> int:
    # Whole microseconds, in integers. Going through `timestamp()` puts the
    # value in a float, whose spacing at today's epoch is 256 ns, so the
    # exchange's microsecond came back up to 128 ns off.
    dt = datetime.fromisoformat(iso.replace("Z", "+00:00")).astimezone(UTC)
    return (dt - _EPOCH) // timedelta(microseconds=1) * 1_000


def match_to_tick(msg: dict) -> dict:
    """Normalize a Coinbase `match` message into a canonical tick."""
    return {
        "ts_ns": _iso_to_ns(msg["time"]),
        "product": msg["product_id"],
        "price": float(msg["price"]),
        "size": float(msg["size"]),
        "side": msg["side"],
        "trade_id": int(msg["trade_id"]),
    }


def ticker_to_quote(msg: dict) -> dict:
    """Normalize a Coinbase `ticker` message into a canonical top-of-book quote."""
    return {
        "ts_ns": _iso_to_ns(msg["time"]),
        "product": msg["product_id"],
        "bid": float(msg["best_bid"]),
        "bid_size": float(msg["best_bid_size"]),
        "ask": float(msg["best_ask"]),
        "ask_size": float(msg["best_ask_size"]),
    }


def snapshot_to_books(msg: dict) -> tuple[dict[float, float], dict[float, float]]:
    """Seed bid/ask books (price -> resting size) from a Coinbase `level2` snapshot."""
    bids = {float(p): float(s) for p, s in msg.get("bids", [])}
    asks = {float(p): float(s) for p, s in msg.get("asks", [])}
    return bids, asks


def apply_l2_update(
    bid_book: dict[float, float], ask_book: dict[float, float], changes: list
) -> None:
    """Apply one Coinbase `l2update` to the books in place.

    Each change is `[side, price, size]`; a size of 0 means the level was fully
    consumed and is removed. This is the stateful part of L2: the wire only sends
    deltas, so the book is whatever the accumulated updates say it is.
    """
    for side, price, size in changes:
        book = bid_book if side == "buy" else ask_book
        px, sz = float(price), float(size)
        if sz == 0.0:
            book.pop(px, None)
        else:
            book[px] = sz


def book_levels(
    bid_book: dict[float, float],
    ask_book: dict[float, float],
    *,
    product: str,
    ts_ns: int,
    levels: int,
) -> list[dict]:
    """Flatten the top `levels` of each side into canonical BookLevel rows.

    Bids are ranked highest price first, asks lowest first, so `level` 0 is the
    top of book on each side -- exactly the schema `depth_metrics` consumes.
    """
    best_bids = sorted(bid_book.items(), key=lambda kv: -kv[0])[:levels]
    best_asks = sorted(ask_book.items(), key=lambda kv: kv[0])[:levels]
    rows: list[dict] = []
    for side, book in (("bid", best_bids), ("ask", best_asks)):
        for i, (price, size) in enumerate(book):
            rows.append(
                {
                    "ts_ns": ts_ns,
                    "product": product,
                    "side": side,
                    "level": i,
                    "price": price,
                    "size": size,
                }
            )
    return rows


class _Bound:
    """When a live capture stops: after `count` records, at the first record
    stamped `seconds` or more after the first one written, or whichever comes
    first. A capture with neither would never return."""

    def __init__(self, count: int | None, seconds: float | None):
        if count is None and seconds is None:
            raise ValueError("a live capture needs a count, a duration, or both")
        self.count = count
        self.span_ns = None if seconds is None else round(seconds * 1e9)
        self.start_ns: int | None = None
        self.over = False

    def done(self, received: int) -> bool:
        return self.over or (self.count is not None and received >= self.count)

    def expired(self, ts_ns: int) -> bool:
        if self.span_ns is None:
            return False
        if self.start_ns is None:
            self.start_ns = ts_ns
        self.over = ts_ns - self.start_ns >= self.span_ns
        return self.over


async def stream_coinbase(
    product: str,
    out_path: str | Path,
    max_trades: int | None,
    log: EventLog | None = None,
    max_reconnects: int = 5,
    seconds: float | None = None,
) -> int:
    """Capture up to `max_trades` live trades for `product` into an NDJSON file.

    `seconds` bounds the capture by exchange time instead, or as well: the
    window opens at the first trade written, and the first trade stamped at or
    past its end stops the capture without being written. A session of a fixed
    length cannot be sized by count, because the trade rate varies tenfold over
    a day.

    A live feed drops connections; rather than losing the capture, this
    reconnects and resumes appending until the target count is met or the
    reconnect budget is exhausted. On resubscribe the exchange sends one
    `last_match`, which is the newest trade and may be one already written, so
    a trade is skipped when its `trade_id` is not above the last one captured.
    Each reconnect is logged so a gap in the capture is visible.
    """
    import websockets

    sub = {"type": "subscribe", "product_ids": [product], "channels": ["matches"]}
    bound = _Bound(max_trades, seconds)
    received = 0
    reconnects = 0
    last_trade_id = None
    if log:
        log.emit("ingest_start", product=product, max_trades=max_trades, seconds=seconds)
    Path(out_path).parent.mkdir(parents=True, exist_ok=True)
    with open(out_path, "w") as f:
        while not bound.done(received):
            try:
                async with websockets.connect(COINBASE_WS, ping_interval=20) as ws:
                    await ws.send(json.dumps(sub))
                    while not bound.done(received):
                        msg = json.loads(await ws.recv())
                        if msg.get("type") not in ("match", "last_match"):
                            continue
                        tick = match_to_tick(msg)
                        if last_trade_id is not None and tick["trade_id"] <= last_trade_id:
                            continue
                        if bound.expired(tick["ts_ns"]):
                            break
                        last_trade_id = tick["trade_id"]
                        f.write(json.dumps(tick) + "\n")
                        f.flush()
                        received += 1
                        if log and received % 10 == 0:
                            log.emit("ingest_progress", received=received)
            except websockets.ConnectionClosed:
                reconnects += 1
                if reconnects > max_reconnects:
                    raise
                if log:
                    log.emit("ingest_reconnect", attempt=reconnects, received=received)
    if log:
        log.emit("ingest_complete", received=received, reconnects=reconnects, out=str(out_path))
    return received


async def stream_coinbase_quotes(
    product: str,
    out_path: str | Path,
    max_quotes: int | None,
    log: EventLog | None = None,
    max_reconnects: int = 5,
    seconds: float | None = None,
) -> int:
    """Capture up to `max_quotes` live top-of-book quotes into an NDJSON file.

    `seconds` bounds it by exchange time, as in `stream_coinbase`.

    Subscribes to Coinbase's `ticker` channel (best bid/ask on every trade). A
    live feed drops connections; rather than losing the capture, this reconnects
    and resumes appending until the target count is met or the reconnect budget
    is exhausted. Each reconnect is logged so a gap in the capture is visible.
    """
    import websockets

    sub = {"type": "subscribe", "product_ids": [product], "channels": ["ticker"]}
    bound = _Bound(max_quotes, seconds)
    received = 0
    reconnects = 0
    if log:
        log.emit("quote_ingest_start", product=product, max_quotes=max_quotes, seconds=seconds)
    Path(out_path).parent.mkdir(parents=True, exist_ok=True)
    with open(out_path, "w") as f:
        while not bound.done(received):
            try:
                async with websockets.connect(COINBASE_WS, ping_interval=20) as ws:
                    await ws.send(json.dumps(sub))
                    while not bound.done(received):
                        msg = json.loads(await ws.recv())
                        if msg.get("type") != "ticker" or "best_bid" not in msg:
                            continue
                        quote = ticker_to_quote(msg)
                        if bound.expired(quote["ts_ns"]):
                            break
                        f.write(json.dumps(quote) + "\n")
                        f.flush()
                        received += 1
                        if log and received % 10 == 0:
                            log.emit("quote_ingest_progress", received=received)
            except websockets.ConnectionClosed:
                reconnects += 1
                if reconnects > max_reconnects:
                    raise
                if log:
                    log.emit("quote_ingest_reconnect", attempt=reconnects, received=received)
    if log:
        log.emit(
            "quote_ingest_complete", received=received, reconnects=reconnects, out=str(out_path)
        )
    return received


async def stream_coinbase_book(
    product: str,
    out_path: str | Path,
    max_snapshots: int | None,
    levels: int = 10,
    log: EventLog | None = None,
    max_reconnects: int = 5,
    seconds: float | None = None,
) -> int:
    """Reconstruct the L2 book live and write its top `levels` as a depth replay.

    `seconds` bounds it by exchange time, as in `stream_coinbase`, so a book
    taken beside a trade capture with the same `seconds` covers the same stretch.

    Subscribes to Coinbase's public `level2_batch` channel: one `snapshot` seeds
    the book, then each `l2update` mutates it in place (`apply_l2_update`). After
    every update the top `levels` of each side are flattened (`book_levels`) and
    appended to the NDJSON depth replay, one canonical BookLevel row per line.

    A dropped connection is handled by reconnecting; the exchange re-sends a fresh
    `snapshot` on resubscribe, so the book is re-seeded from scratch -- a clean
    backfill with no stale levels carried across the gap. Each reconnect is
    logged so the discontinuity is visible in the event stream.
    """
    import websockets

    sub = {"type": "subscribe", "product_ids": [product], "channels": ["level2_batch"]}
    bound = _Bound(max_snapshots, seconds)
    written = 0
    reconnects = 0
    if log:
        log.emit(
            "book_ingest_start",
            product=product,
            max_snapshots=max_snapshots,
            levels=levels,
            seconds=seconds,
        )
    Path(out_path).parent.mkdir(parents=True, exist_ok=True)
    with open(out_path, "w") as f:
        while not bound.done(written):
            # Reset on (re)connect: the fresh snapshot is the backfill.
            bid_book: dict[float, float] = {}
            ask_book: dict[float, float] = {}
            try:
                # The snapshot is the whole book, past websockets' 1 MiB default.
                async with websockets.connect(COINBASE_WS, ping_interval=20, max_size=None) as ws:
                    await ws.send(json.dumps(sub))
                    while not bound.done(written):
                        msg = json.loads(await ws.recv())
                        mtype = msg.get("type")
                        if mtype == "snapshot":
                            bid_book, ask_book = snapshot_to_books(msg)
                            continue
                        if mtype != "l2update":
                            continue
                        apply_l2_update(bid_book, ask_book, msg.get("changes", []))
                        if not bid_book or not ask_book:
                            continue
                        ts_ns = _iso_to_ns(msg["time"])
                        if bound.expired(ts_ns):
                            break
                        rows = book_levels(
                            bid_book, ask_book, product=product, ts_ns=ts_ns, levels=levels
                        )
                        for row in rows:
                            f.write(json.dumps(row) + "\n")
                        f.flush()
                        written += 1
                        if log and written % 10 == 0:
                            log.emit("book_ingest_progress", snapshots=written)
            except websockets.ConnectionClosed:
                reconnects += 1
                if reconnects > max_reconnects:
                    raise
                if log:
                    log.emit("book_ingest_reconnect", attempt=reconnects, snapshots=written)
    if log:
        log.emit(
            "book_ingest_complete", snapshots=written, reconnects=reconnects, out=str(out_path)
        )
    return written


def synthetic_book(
    out_path: str | Path,
    n_snapshots: int = 100,
    levels: int = 5,
    seed: int = 7,
    product: str = "BTC-USD",
    start_ns: int = 1_719_792_000_000_000_000,
) -> int:
    """Write `n_snapshots` deterministic L2 book snapshots -- an offline depth
    replay. Returns the number of BookLevel rows written."""
    rng = random.Random(seed)
    mid = 60000.0
    ts = start_ns
    written = 0
    Path(out_path).parent.mkdir(parents=True, exist_ok=True)
    with open(out_path, "w") as f:
        for _ in range(n_snapshots):
            mid += rng.uniform(-15, 15)
            ts += int(rng.uniform(0.05, 0.6) * 1e9)
            half_spread = round(rng.uniform(0.25, 1.5), 2)
            for side, sign in (("bid", -1.0), ("ask", 1.0)):
                for i in range(levels):
                    row = {
                        "ts_ns": ts,
                        "product": product,
                        "side": side,
                        "level": i,
                        "price": round(mid + sign * (half_spread + i * 0.5), 2),
                        "size": round(rng.uniform(0.1, 3.0), 4),
                    }
                    f.write(json.dumps(row) + "\n")
                    written += 1
    return written


def synthetic_quotes(
    out_path: str | Path,
    n: int = 200,
    seed: int = 7,
    product: str = "BTC-USD",
    start_ns: int = 1_719_792_000_000_000_000,
) -> int:
    """Write `n` deterministic top-of-book quotes -- an offline quote replay."""
    rng = random.Random(seed)
    mid = 60000.0
    ts = start_ns
    Path(out_path).parent.mkdir(parents=True, exist_ok=True)
    with open(out_path, "w") as f:
        for _ in range(n):
            mid += rng.uniform(-15, 15)
            ts += int(rng.uniform(0.05, 0.6) * 1e9)
            half_spread = round(rng.uniform(0.25, 1.5), 2)
            quote = {
                "ts_ns": ts,
                "product": product,
                "bid": round(mid - half_spread, 2),
                "bid_size": round(rng.uniform(0.1, 3.0), 4),
                "ask": round(mid + half_spread, 2),
                "ask_size": round(rng.uniform(0.1, 3.0), 4),
            }
            f.write(json.dumps(quote) + "\n")
    return n


def synthetic_calibration(
    out_path: str | Path,
    n: int = 200,
    coef_bps: float = 10.0,
    perm_coef_bps: float = 20.0,
    noise_bps: float = 0.0,
    outlier_frac: float = 0.0,
    outlier_bps: float = 0.0,
    seed: int = 7,
    product: str = "BTC-USD",
    start_ns: int = 1_719_792_000_000_000_000,
) -> int:
    """Write `n` deterministic realised-fill calibration samples.

    A desk calibrates the impact model on its *own* fills; this generates a
    stand-in set for demos and offline runs. Each sample draws a participation
    in (0, 1] and prices its realised cost from the two-term Almgren-Chriss model
    `coef_bps * sqrt(p) + perm_coef_bps * p`, plus optional Gaussian `noise_bps`.
    With `noise_bps == 0` the samples lie exactly on the model, so
    `calibrate_impact` recovers `(coef_bps, perm_coef_bps)` to floating-point
    precision; with noise they are a realistic, imperfect fit target.

    Real fill logs also carry the occasional bad print -- a mis-tagged
    participation, a fat-finger, a venue glitch. `outlier_frac` (the share of
    samples corrupted) and `outlier_bps` (the shock added to a corrupted sample's
    cost) inject those, so the replay exercises `calibrate_impact_robust`: plain
    least squares is dragged off by the shocks while the Huber fit shrugs them
    off. With `outlier_frac == 0` no samples are corrupted and the draw sequence
    is unchanged. Returns the number of samples written.
    """
    rng = random.Random(seed)
    ts = start_ns
    Path(out_path).parent.mkdir(parents=True, exist_ok=True)
    with open(out_path, "w") as f:
        for _ in range(n):
            ts += int(rng.uniform(0.05, 0.6) * 1e9)
            participation = round(rng.uniform(0.01, 0.5), 4)
            cost = coef_bps * math.sqrt(participation) + perm_coef_bps * participation
            if noise_bps > 0:
                cost += rng.gauss(0.0, noise_bps)
            if outlier_frac > 0 and rng.random() < outlier_frac:
                cost += outlier_bps
            sample = {
                "ts_ns": ts,
                "product": product,
                "participation": participation,
                "realised_bps": round(cost, 6),
            }
            f.write(json.dumps(sample) + "\n")
    return n


def synthetic_ticks(
    out_path: str | Path,
    n: int = 200,
    seed: int = 7,
    product: str = "BTC-USD",
    start_ns: int = 1_719_792_000_000_000_000,
) -> int:
    """Write `n` deterministic ticks -- a stand-in replay for offline runs."""
    rng = random.Random(seed)
    price = 60000.0
    ts = start_ns
    Path(out_path).parent.mkdir(parents=True, exist_ok=True)
    with open(out_path, "w") as f:
        for i in range(n):
            price += rng.uniform(-15, 15)
            ts += int(rng.uniform(0.05, 0.6) * 1e9)
            tick = {
                "ts_ns": ts,
                "product": product,
                "price": round(price, 2),
                "size": round(rng.uniform(0.01, 0.5), 4),
                "side": "buy" if rng.random() > 0.5 else "sell",
                "trade_id": i + 1,
            }
            f.write(json.dumps(tick) + "\n")
    return n
