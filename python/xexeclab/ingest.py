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
