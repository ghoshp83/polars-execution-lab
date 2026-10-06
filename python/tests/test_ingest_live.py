"""The live collectors, driven offline.

`stream_coinbase*` own the parts of a capture that the pure normalisers do not:
subscribing, skipping what is not data, stopping at the target, and surviving a
dropped connection. None of that needs the exchange -- only something that
speaks `websockets`' small surface -- so a scripted stand-in replaces the module
and each test states the feed it wants as a list of connections.
"""

from __future__ import annotations

import asyncio
import json
import sys
import types

import pytest

from xexeclab.events import EventLog
from xexeclab.ingest import (
    COINBASE_WS,
    stream_coinbase,
    stream_coinbase_book,
    stream_coinbase_quotes,
)


class _Closed(Exception):
    """Stands in for `websockets.ConnectionClosed`."""


class _Feed:
    """A fake `websockets`: each `connect` serves the next scripted connection,
    then drops it. Running out of connections is a test bug, not a drop."""

    ConnectionClosed = _Closed

    def __init__(self, connections: list[list[dict]]):
        self._connections = list(connections)
        self.urls: list[str] = []
        self.subscriptions: list[dict] = []

    def connect(self, url: str, **_kwargs):
        assert self._connections, "the collector connected more often than the feed scripted"
        self.urls.append(url)
        return _Socket(self, self._connections.pop(0))


class _Socket:
    def __init__(self, feed: _Feed, messages: list[dict]):
        self._feed = feed
        self._messages = list(messages)

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_exc):
        return False

    async def send(self, raw: str) -> None:
        self._feed.subscriptions.append(json.loads(raw))

    async def recv(self) -> str:
        if not self._messages:
            raise _Closed
        return json.dumps(self._messages.pop(0))


@pytest.fixture
def feed(monkeypatch):
    def install(connections: list[list[dict]]) -> _Feed:
        fake = _Feed(connections)
        module = types.ModuleType("websockets")
        module.connect = fake.connect
        module.ConnectionClosed = _Closed
        monkeypatch.setitem(sys.modules, "websockets", module)
        return fake

    return install


def _match(trade_id: int, kind: str = "match") -> dict:
    return {
        "type": kind,
        "trade_id": trade_id,
        "product_id": "BTC-USD",
        "time": f"2024-07-01T00:00:{trade_id:02d}.000000Z",
        "size": "0.01",
        "price": "60000.00",
        "side": "buy",
    }


def _ticker(seq: int) -> dict:
    return {
        "type": "ticker",
        "product_id": "BTC-USD",
        "time": f"2024-07-01T00:00:{seq:02d}.000000Z",
        "best_bid": "59999.50",
        "best_bid_size": "1.0",
        "best_ask": "60000.50",
        "best_ask_size": "1.0",
    }


def _lines(path) -> list[dict]:
    return [json.loads(line) for line in path.read_text().splitlines()]


def test_trades_are_captured_up_to_the_target_and_no_further(feed, tmp_path):
    """The capture is bounded by `max_trades`, not by the feed: a collector that
    drained whatever arrived would never return on a live exchange. Messages
    that are not trades (the subscription ack) must not count toward it."""
    fake = feed([[{"type": "subscriptions"}, _match(1), _match(2), _match(3), _match(4)]])
    out = tmp_path / "trades.ndjson"
    n = asyncio.run(stream_coinbase("BTC-USD", out, 3))
    assert n == 3
    assert [t["trade_id"] for t in _lines(out)] == [1, 2, 3]
    assert fake.urls == [COINBASE_WS]
    assert fake.subscriptions == [
        {"type": "subscribe", "product_ids": ["BTC-USD"], "channels": ["matches"]}
    ]


def test_a_dropped_trade_feed_is_resumed_without_repeating_a_trade(feed, tmp_path):
    """A drop must not end the capture, and the resume must not corrupt it. On
    resubscribe the exchange replays its newest trade as `last_match`; if that
    is one already on disk, writing it again would double its volume in every
    bucket downstream. The reconnect is logged so the gap is visible."""
    feed([[_match(1), _match(2)], [_match(2, "last_match"), _match(3), _match(4)]])
    out = tmp_path / "trades.ndjson"
    log = EventLog()
    n = asyncio.run(stream_coinbase("BTC-USD", out, 4, log))
    assert n == 4
    assert [t["trade_id"] for t in _lines(out)] == [1, 2, 3, 4]
    events = {e["event"]: e for e in log.events}
    assert events["ingest_reconnect"]["attempt"] == 1
    assert events["ingest_reconnect"]["received"] == 2
    assert events["ingest_complete"]["reconnects"] == 1
    assert events["ingest_complete"]["received"] == 4


def test_a_trade_feed_that_keeps_dropping_fails_loudly(feed, tmp_path):
    """The reconnect budget exists so a dead feed is an error and not a short
    file reported as a capture. What was written before the failure stays."""
    feed([[_match(1)], [], []])
    out = tmp_path / "trades.ndjson"
    with pytest.raises(_Closed):
        asyncio.run(stream_coinbase("BTC-USD", out, 5, max_reconnects=1))
    assert [t["trade_id"] for t in _lines(out)] == [1]


def test_quotes_skip_tickers_without_a_book_and_resume_after_a_drop(feed, tmp_path):
    """The first `ticker` after subscribing can arrive without `best_bid`; it is
    not a quote and must be skipped, not crash the normaliser. A drop resumes
    into the same file."""
    no_book = {"type": "ticker", "product_id": "BTC-USD", "time": "2024-07-01T00:00:00Z"}
    fake = feed([[no_book, _ticker(1)], [_ticker(2), _ticker(3)]])
    out = tmp_path / "quotes.ndjson"
    log = EventLog()
    n = asyncio.run(stream_coinbase_quotes("BTC-USD", out, 3, log))
    assert n == 3
    assert len(_lines(out)) == 3
    assert fake.subscriptions[0]["channels"] == ["ticker"]
    assert [e["event"] for e in log.events].count("quote_ingest_reconnect") == 1


def test_a_reconnected_book_is_rebuilt_from_the_fresh_snapshot(feed, tmp_path):
    """After a drop the old book is stale -- levels may have gone while nobody
    was listening. The collector must reseed from the snapshot the exchange
    re-sends, so a level present only before the drop never reaches the file."""
    first = [
        {"type": "snapshot", "bids": [["100.0", "1.0"]], "asks": [["101.0", "1.0"]]},
        {
            "type": "l2update",
            "time": "2024-07-01T00:00:01.000000Z",
            "changes": [["buy", "99.0", "2.0"]],
        },
    ]
    second = [
        {"type": "snapshot", "bids": [["90.0", "1.0"]], "asks": [["91.0", "1.0"]]},
        {
            "type": "l2update",
            "time": "2024-07-01T00:00:02.000000Z",
            "changes": [["sell", "92.0", "3.0"]],
        },
    ]
    feed([first, second])
    out = tmp_path / "book.ndjson"
    n = asyncio.run(stream_coinbase_book("BTC-USD", out, 2, levels=5))
    assert n == 2
    rows = _lines(out)
    before = [r for r in rows if r["ts_ns"] == rows[0]["ts_ns"]]
    after = [r for r in rows if r["ts_ns"] != rows[0]["ts_ns"]]
    assert {r["price"] for r in before} == {100.0, 99.0, 101.0}
    assert {r["price"] for r in after} == {90.0, 91.0, 92.0}
