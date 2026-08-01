"""Unit tests for the pure normalization in ingest -- the part of the live
collector that does not need a network. `match_to_tick` is what turns a raw
Coinbase `match` message into the canonical schema the whole engine relies on,
so a mistake here silently corrupts every downstream benchmark.
"""

from xexeclab.engine import QUOTE_COLUMNS, TICK_COLUMNS
from xexeclab.ingest import _iso_to_ns, match_to_tick, ticker_to_quote

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
