"""Market-data ingest: a live Coinbase collector and a deterministic generator.

The live path is the real happy path -- it connects to Coinbase's public
WebSocket (no API key needed for market data) and normalizes each trade into
the canonical tick schema written to an NDJSON replay file. The synthetic
generator produces a deterministic replay for demos and offline runs.
"""

from __future__ import annotations

import json
import random
from datetime import UTC, datetime
from pathlib import Path

from .events import EventLog

COINBASE_WS = "wss://ws-feed.exchange.coinbase.com"


def _iso_to_ns(iso: str) -> int:
    dt = datetime.fromisoformat(iso.replace("Z", "+00:00")).astimezone(UTC)
    return int(dt.timestamp() * 1_000_000_000)


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


async def stream_coinbase(
    product: str,
    out_path: str | Path,
    max_trades: int,
    log: EventLog | None = None,
) -> int:
    """Capture up to `max_trades` live trades for `product` into an NDJSON file."""
    import websockets

    sub = {"type": "subscribe", "product_ids": [product], "channels": ["matches"]}
    received = 0
    if log:
        log.emit("ingest_start", product=product, max_trades=max_trades)
    with open(out_path, "w") as f:
        async with websockets.connect(COINBASE_WS, ping_interval=20) as ws:
            await ws.send(json.dumps(sub))
            while received < max_trades:
                msg = json.loads(await ws.recv())
                if msg.get("type") not in ("match", "last_match"):
                    continue
                f.write(json.dumps(match_to_tick(msg)) + "\n")
                f.flush()
                received += 1
                if log and received % 10 == 0:
                    log.emit("ingest_progress", received=received)
    if log:
        log.emit("ingest_complete", received=received, out=str(out_path))
    return received


async def stream_coinbase_quotes(
    product: str,
    out_path: str | Path,
    max_quotes: int,
    log: EventLog | None = None,
    max_reconnects: int = 5,
) -> int:
    """Capture up to `max_quotes` live top-of-book quotes into an NDJSON file.

    Subscribes to Coinbase's `ticker` channel (best bid/ask on every trade). A
    live feed drops connections; rather than losing the capture, this reconnects
    and resumes appending until the target count is met or the reconnect budget
    is exhausted. Each reconnect is logged so a gap in the capture is visible.
    """
    import websockets

    sub = {"type": "subscribe", "product_ids": [product], "channels": ["ticker"]}
    received = 0
    reconnects = 0
    if log:
        log.emit("quote_ingest_start", product=product, max_quotes=max_quotes)
    with open(out_path, "w") as f:
        while received < max_quotes:
            try:
                async with websockets.connect(COINBASE_WS, ping_interval=20) as ws:
                    await ws.send(json.dumps(sub))
                    while received < max_quotes:
                        msg = json.loads(await ws.recv())
                        if msg.get("type") != "ticker" or "best_bid" not in msg:
                            continue
                        f.write(json.dumps(ticker_to_quote(msg)) + "\n")
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
