"""Unit tests for the pure normalization in ingest -- the part of the live
collector that does not need a network. `match_to_tick` is what turns a raw
Coinbase `match` message into the canonical schema the whole engine relies on,
so a mistake here silently corrupts every downstream benchmark.
"""

from xexeclab.engine import BOOK_LEVEL_COLUMNS, QUOTE_COLUMNS, TICK_COLUMNS
from xexeclab.ingest import (
    _iso_to_ns,
    apply_l2_update,
    book_levels,
    match_to_tick,
    snapshot_to_books,
    ticker_to_quote,
)

RAW_MATCH = {
    "type": "match",
    "trade_id": 424242,
    "product_id": "BTC-USD",
    "time": "2024-07-01T00:00:00.500000Z",
    "size": "0.01500000",
    "price": "60000.12",
    "side": "sell",
}

RAW_TICKER = {
    "type": "ticker",
    "product_id": "BTC-USD",
    "time": "2024-07-01T00:00:00.500000Z",
    "best_bid": "59999.50",
    "best_bid_size": "1.20000000",
    "best_ask": "60000.50",
    "best_ask_size": "0.80000000",
}


def test_iso_to_ns_parses_utc_to_nanoseconds():
    # 2024-07-01T00:00:00Z is 1719792000 s; the .5s offset adds 5e8 ns.
    assert _iso_to_ns("2024-07-01T00:00:00Z") == 1_719_792_000_000_000_000
    assert _iso_to_ns("2024-07-01T00:00:00.500000Z") == 1_719_792_000_500_000_000


def test_match_to_tick_yields_the_canonical_schema():
    tick = match_to_tick(RAW_MATCH)
    # Exactly the canonical columns, nothing extra, nothing missing.
    assert tuple(tick.keys()) == TICK_COLUMNS
    assert tick["ts_ns"] == 1_719_792_000_500_000_000
    assert tick["product"] == "BTC-USD"
    # Strings from the wire must become numbers of the right type.
    assert isinstance(tick["price"], float) and tick["price"] == 60000.12
    assert isinstance(tick["size"], float) and tick["size"] == 0.015
    assert isinstance(tick["trade_id"], int) and tick["trade_id"] == 424242
    assert tick["side"] == "sell"


def test_ticker_to_quote_yields_the_canonical_quote_schema():
    quote = ticker_to_quote(RAW_TICKER)
    assert tuple(quote.keys()) == QUOTE_COLUMNS
    assert quote["ts_ns"] == 1_719_792_000_500_000_000
    assert quote["product"] == "BTC-USD"
    # Strings from the wire must become floats, and the book must not cross.
    assert isinstance(quote["bid"], float) and quote["bid"] == 59999.5
    assert isinstance(quote["ask"], float) and quote["ask"] == 60000.5
    assert quote["ask"] > quote["bid"]
    assert quote["bid_size"] == 1.2 and quote["ask_size"] == 0.8


def test_l2_update_applies_deltas_and_removes_emptied_levels():
    bids, asks = snapshot_to_books(
        {"bids": [["100.0", "1.0"], ["99.5", "2.0"]], "asks": [["101.0", "1.5"]]}
    )
    # A resize, a new level, and a removal (size 0) -- the stateful core of L2.
    apply_l2_update(
        bids, asks, [["buy", "100.0", "3.0"], ["buy", "99.0", "1.0"], ["sell", "101.0", "0"]]
    )
    assert bids == {100.0: 3.0, 99.5: 2.0, 99.0: 1.0}
    assert asks == {}  # the only ask level was fully consumed


def test_book_levels_flattens_top_of_book_in_canonical_schema():
    bids = {100.0: 1.0, 99.0: 2.0, 98.0: 3.0}
    asks = {101.0: 1.0, 102.0: 2.0, 103.0: 3.0}
    rows = book_levels(bids, asks, product="BTC-USD", ts_ns=42, levels=2)
    assert all(tuple(r.keys()) == BOOK_LEVEL_COLUMNS for r in rows)
    # Two levels a side, ranked best-first: bid level 0 is the highest price,
    # ask level 0 the lowest.
    bid_rows = [r for r in rows if r["side"] == "bid"]
    ask_rows = [r for r in rows if r["side"] == "ask"]
    assert [(r["level"], r["price"]) for r in bid_rows] == [(0, 100.0), (1, 99.0)]
    assert [(r["level"], r["price"]) for r in ask_rows] == [(0, 101.0), (1, 102.0)]
    assert all(r["ts_ns"] == 42 and r["product"] == "BTC-USD" for r in rows)
