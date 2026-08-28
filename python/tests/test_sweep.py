import polars as pl
import pytest

from xexeclab.engine import read_book, sweep_cost

SAMPLE = "data/sample_book.ndjson"

_SCHEMA = {
    "ts_ns": pl.Int64,
    "product": pl.String,
    "side": pl.String,
    "level": pl.Int64,
    "price": pl.Float64,
    "size": pl.Float64,
}


def _book(rows: list[tuple]) -> pl.DataFrame:
    return pl.DataFrame(rows, schema=_SCHEMA, orient="row")


def test_sweep_prices_a_buy_that_walks_the_sample_book():
    m = sweep_cost(read_book(SAMPLE), "BTC-USD", "buy", 2.0)
    # A 2.0 buy clears the touch in every snapshot and eats into deeper levels
    # (2, 2, 3, 2, 2 -> mean 2.2); values hand-checked against the fixture.
    assert m["snapshots"] == 5
    assert m["filled_snapshots"] == 5
    assert m["avg_sweep_vwap"] == pytest.approx(60011.96)
    assert m["avg_slippage_bps"] == pytest.approx(0.04332343, abs=1e-8)
    assert m["avg_levels_consumed"] == pytest.approx(2.2)
    assert m["avg_fill_ratio"] == pytest.approx(1.0)


def test_an_order_inside_the_touch_pays_no_slippage():
    # The whole order rests at the best level, so the realised price *is* the
    # touch: the sweep costs nothing beyond crossing the spread. This is the
    # boundary that separates liquidity cost from spread cost.
    m = sweep_cost(read_book(SAMPLE), "BTC-USD", "buy", 0.4)
    assert m["avg_slippage_bps"] == pytest.approx(0.0)
    assert m["avg_levels_consumed"] == pytest.approx(1.0)
    assert m["avg_fill_ratio"] == pytest.approx(1.0)


def test_slippage_grows_with_order_size():
    # The economic claim of the whole module: a bigger order reaches worse
    # prices. If this ever stopped holding the book walk would be wrong.
    df = read_book(SAMPLE)
    costs = [sweep_cost(df, "BTC-USD", "buy", q)["avg_slippage_bps"] for q in (0.4, 1.0, 2.0, 3.0)]
    assert costs == sorted(costs)
    assert costs[-1] > costs[0]


def test_a_thin_book_reports_a_short_fill_not_a_silent_full_one():
    # 10.0 exceeds the captured depth of every snapshot: the sweep must report
    # what it could actually fill rather than pretending the order completed.
    m = sweep_cost(read_book(SAMPLE), "BTC-USD", "buy", 10.0)
    assert m["filled_snapshots"] == 0
    assert m["avg_fill_ratio"] == pytest.approx(0.36)
    assert m["avg_levels_consumed"] == pytest.approx(3.0)


def test_a_sell_walks_the_bids_downward():
    # Bids 2.0 @ 100 then 1.0 @ 99; a 3.0 sell realises
    # (2*100 + 1*99) / 3 = 99.666..., i.e. (100 - 99.666...) / 100 * 1e4 bps
    # below the touch. Signed so a larger number is worse on either side.
    df = _book(
        [
            (0, "BTC-USD", "bid", 0, 100.0, 2.0),
            (0, "BTC-USD", "bid", 1, 99.0, 1.0),
            (0, "BTC-USD", "ask", 0, 101.0, 5.0),
        ]
    )
    m = sweep_cost(df, "BTC-USD", "sell", 3.0)
    assert m["avg_sweep_vwap"] == pytest.approx(299.0 / 3)
    assert m["avg_slippage_bps"] == pytest.approx(33.33333333, abs=1e-8)
    assert m["avg_levels_consumed"] == pytest.approx(2.0)


def test_a_buy_and_a_sell_of_the_same_size_price_different_sides():
    # The same order size must not read the same book: a buy crosses the asks,
    # a sell crosses the bids.
    df = read_book(SAMPLE)
    buy = sweep_cost(df, "BTC-USD", "buy", 2.0)
    sell = sweep_cost(df, "BTC-USD", "sell", 2.0)
    assert buy["avg_sweep_vwap"] > sell["avg_sweep_vwap"]


def test_bad_inputs_raise():
    df = read_book(SAMPLE)
    with pytest.raises(ValueError):
        sweep_cost(_book([]), "BTC-USD", "buy", 1.0)
    with pytest.raises(ValueError):
        sweep_cost(df, "BTC-USD", "buy", 0.0)
    # A NaN or infinite size would otherwise poison every average silently.
    with pytest.raises(ValueError):
        sweep_cost(df, "BTC-USD", "buy", float("nan"))
    with pytest.raises(ValueError):
        sweep_cost(df, "BTC-USD", "buy", float("inf"))
    with pytest.raises(ValueError):
        sweep_cost(df, "BTC-USD", "sideways", 1.0)
