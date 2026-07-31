"""Unit tests for the pure normalization in ingest -- the part of the live
collector that does not need a network. `match_to_tick` is what turns a raw
Coinbase `match` message into the canonical schema the whole engine relies on,
so a mistake here silently corrupts every downstream benchmark.
"""

from xexeclab.engine import TICK_COLUMNS
from xexeclab.ingest import _iso_to_ns, match_to_tick

RAW_MATCH = {
    "type": "match",
    "trade_id": 424242,
    "product_id": "BTC-USD",
    "time": "2024-07-01T00:00:00.500000Z",
    "size": "0.01500000",
    "price": "60000.12",
    "side": "sell",
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
