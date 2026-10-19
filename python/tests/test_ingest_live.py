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
        self.options: list[dict] = []
        self.subscriptions: list[dict] = []

    def connect(self, url: str, **options):
        assert self._connections, "the collector connected more often than the feed scripted"
        self.urls.append(url)
        self.options.append(options)
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


def test_a_timed_capture_holds_exactly_its_window_of_exchange_time(feed, tmp_path):
    """Sessions of a fixed length were cut from captures sized by count, and a
    count is a poor proxy: the same 2,600 trades took ten minutes one morning
    and four the next afternoon. The window is the exchange's clock, opened by
    the first trade written; the trade that reaches its end is not written."""
    feed([[{"type": "subscriptions"}, *[_match(i) for i in range(1, 7)]]])
    out = tmp_path / "trades.ndjson"
    log = EventLog()
    n = asyncio.run(stream_coinbase("BTC-USD", out, None, log, seconds=3))
    assert n == 3
    assert [t["trade_id"] for t in _lines(out)] == [1, 2, 3]
    assert log.events[0]["seconds"] == 3
    assert log.events[-1]["received"] == 3


def test_a_timed_capture_still_stops_at_its_count(feed, tmp_path):
    """Both bounds apply, and the nearer one ends the capture."""
    feed([[_match(i) for i in range(1, 7)]])
    out = tmp_path / "trades.ndjson"
    assert asyncio.run(stream_coinbase("BTC-USD", out, 2, seconds=60)) == 2


def test_a_timed_quote_capture_holds_its_window(feed, tmp_path):
    """Quotes are bounded by the same clock, so a quote file and a trade file
    taken side by side with the same `seconds` cover the same stretch."""
    feed([[_ticker(i) for i in range(10, 16)]])
    out = tmp_path / "quotes.ndjson"
    n = asyncio.run(stream_coinbase_quotes("BTC-USD", out, None, seconds=2.5))
    assert n == 3
    assert len(_lines(out)) == 3


def test_a_capture_with_no_bound_is_refused_before_it_connects(feed, tmp_path):
    """Neither a count nor a duration would stream until the process is
    killed; that is an error at the call, not a hang."""
    fake = feed([])
    with pytest.raises(ValueError, match="count, a duration"):
        asyncio.run(stream_coinbase("BTC-USD", tmp_path / "t.ndjson", None))
    assert fake.urls == []


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


def _l2update(second: int, price: str) -> dict:
    return {
        "type": "l2update",
        "time": f"2024-07-01T00:00:{second:02d}.000000Z",
        "changes": [["buy", price, "1.0"]],
    }


def test_a_timed_book_capture_covers_the_same_window_as_the_trades(feed, tmp_path):
    """A book recorded beside a trade capture is only useful if both cover the
    same stretch, and a snapshot count cannot promise that: updates come in
    bursts. The window opens at the first snapshot written and the update that
    reaches its end is not written."""
    seed = {"type": "snapshot", "bids": [["100.0", "1.0"]], "asks": [["101.0", "1.0"]]}
    feed([[seed, *[_l2update(i, f"{90 + i}.0") for i in range(1, 7)]]])
    out = tmp_path / "book.ndjson"
    log = EventLog()
    n = asyncio.run(stream_coinbase_book("BTC-USD", out, None, levels=10, log=log, seconds=3))
    assert n == 3
    assert sorted({r["ts_ns"] for r in _lines(out)}) == [
        1_719_792_001_000_000_000,
        1_719_792_002_000_000_000,
        1_719_792_003_000_000_000,
    ]
    assert log.events[0]["seconds"] == 3
    assert log.events[-1]["snapshots"] == 3


def test_a_book_capture_with_no_bound_is_refused_before_it_connects(feed, tmp_path):
    fake = feed([])
    with pytest.raises(ValueError, match="count, a duration"):
        asyncio.run(stream_coinbase_book("BTC-USD", tmp_path / "b.ndjson", None))
    assert fake.urls == []


def test_the_book_connection_takes_a_snapshot_of_any_size(feed, tmp_path):
    """The live BTC-USD `level2_batch` snapshot is the whole book, over a
    megabyte, and `websockets` closes any connection whose message passes its
    1 MiB default -- before the collector sees it. Every reconnect asks for the
    snapshot again, so a capped connection never records a single row."""
    seed = {"type": "snapshot", "bids": [["100.0", "1.0"]], "asks": [["101.0", "1.0"]]}
    fake = feed([[seed, _l2update(1, "99.0")]])
    asyncio.run(stream_coinbase_book("BTC-USD", tmp_path / "book.ndjson", 1))
    assert fake.options[0]["max_size"] is None


def test_a_thinned_book_still_applies_every_update_it_does_not_write(feed, tmp_path):
    """`every` drops snapshots, not updates. The feed only sends deltas, so an
    update skipped instead of applied would leave the next snapshot written
    wrong for the rest of the run: the levels added at seconds 2 and 3 must
    still be in the book written at second 4."""
    seed = {"type": "snapshot", "bids": [["100.0", "1.0"]], "asks": [["101.0", "1.0"]]}
    feed([[seed, *[_l2update(i, f"{90 + i}.0") for i in range(1, 7)]]])
    out = tmp_path / "book.ndjson"
    log = EventLog()
    n = asyncio.run(
        stream_coinbase_book("BTC-USD", out, None, levels=10, log=log, seconds=5, every=2.5)
    )
    assert n == 2
    rows = _lines(out)
    stamps = sorted({r["ts_ns"] for r in rows})
    assert stamps == [1_719_792_001_000_000_000, 1_719_792_004_000_000_000]
    bids = {r["price"] for r in rows if r["ts_ns"] == stamps[1] and r["side"] == "bid"}
    assert bids == {100.0, 94.0, 93.0, 92.0, 91.0}
    assert log.events[0]["every"] == 2.5


def test_a_full_rate_book_writes_only_the_snapshots_that_moved(feed, tmp_path):
    """The update at second 2 rests a bid below the one level recorded, so the
    top of book raised at second 3 is the only change after the first snapshot.
    A copy written per deeper update would count each second of a busy deep book
    as another observation of a top that did not move."""
    seed = {"type": "snapshot", "bids": [["100.0", "1.0"]], "asks": [["101.0", "1.0"]]}
    updates = [_l2update(1, "91.0"), _l2update(2, "92.0"), _l2update(3, "100.5")]
    feed([[seed, *updates, _l2update(4, "94.0")]])
    out = tmp_path / "book.ndjson"
    log = EventLog()
    n = asyncio.run(stream_coinbase_book("BTC-USD", out, None, levels=1, log=log, seconds=3))
    assert n == 2
    rows = _lines(out)
    assert sorted({r["ts_ns"] for r in rows}) == [
        1_719_792_001_000_000_000,
        1_719_792_003_000_000_000,
    ]
    assert [r["price"] for r in rows if r["side"] == "bid"] == [100.0, 100.5]
    assert log.events[-1]["unchanged"] == 1


def test_a_thinned_book_writes_its_step_even_when_nothing_moved(feed, tmp_path):
    """With `every` the file is a time grid: a step where the recorded levels
    held is still an observation, and skipping it would leave a hole in the grid
    exactly where the book was quiet."""
    seed = {"type": "snapshot", "bids": [["100.0", "1.0"]], "asks": [["101.0", "1.0"]]}
    feed([[seed, *[_l2update(i, f"{90 + i}.0") for i in range(1, 6)]]])
    out = tmp_path / "book.ndjson"
    log = EventLog()
    n = asyncio.run(
        stream_coinbase_book("BTC-USD", out, None, levels=1, log=log, seconds=4, every=1)
    )
    assert n == 4
    assert log.events[-1]["unchanged"] == 0


@pytest.mark.parametrize("every", [0, -1.0])
def test_a_book_thinned_to_no_interval_is_refused_before_it_connects(feed, tmp_path, every):
    fake = feed([])
    with pytest.raises(ValueError, match="positive number of seconds"):
        asyncio.run(stream_coinbase_book("BTC-USD", tmp_path / "b.ndjson", 5, every=every))
    assert fake.urls == []
