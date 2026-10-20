import polars as pl
import pytest

from xexeclab.engine import book_grid_sweep

S = 1_000_000_000

_SCHEMA = {
    "ts_ns": pl.Int64,
    "product": pl.String,
    "side": pl.String,
    "level": pl.Int64,
    "price": pl.Float64,
    "size": pl.Float64,
}


def _book() -> pl.DataFrame:
    """Five one-level snapshots a second apart; see the Rust twin in
    ``tests/book_grid.rs``. Full rate: queue +0.1 on the bid, spread 0.64."""

    def ask_heavy(t):
        return [(t, "BTC-USD", "bid", 0, 100.9, 1.0), (t, "BTC-USD", "ask", 0, 101.0, 3.0)]

    def bid_heavy(t):
        return [(t, "BTC-USD", "bid", 0, 100.0, 3.0), (t, "BTC-USD", "ask", 0, 101.0, 1.0)]

    rows = ask_heavy(0) + bid_heavy(S) + ask_heavy(2 * S) + bid_heavy(3 * S) + bid_heavy(4 * S)
    return pl.DataFrame(rows, schema=_SCHEMA, orient="row")


def test_a_grid_keeps_the_first_snapshot_and_each_one_a_step_after_the_last_kept():
    sweep = book_grid_sweep(_book(), "BTC-USD", [1.0, 2.0, 10.0])
    assert sweep["full"]["snapshots"] == 5
    assert [r["snapshots"] for r in sweep["rows"]] == [5, 3, 1]
    # A one-second grid over a book recorded a second apart keeps all of it.
    one = sweep["rows"][0]
    assert one["avg_spread"] == sweep["full"]["avg_spread"]
    assert one["avg_queue_imbalance"] == sweep["full"]["avg_queue_imbalance"]
    assert one["spread_ratio"] == 1.0


def test_a_grid_that_lands_on_one_kind_of_touch_moves_the_queue_to_the_other_side():
    sweep = book_grid_sweep(_book(), "BTC-USD", [1.0, 2.0])
    assert sweep["full"]["avg_queue_imbalance"] == pytest.approx(0.1, abs=1e-8)
    assert sweep["full"]["avg_spread"] == pytest.approx(0.64, abs=1e-8)
    # Two seconds lands on 0 s, 2 s and 4 s: two ask-heavy, one bid-heavy.
    two = sweep["rows"][1]
    assert two["avg_queue_imbalance"] == pytest.approx(-1 / 6, abs=1e-8)
    assert two["queue_sign_kept"] is False
    assert two["spread_ratio"] == pytest.approx(0.625, abs=1e-8)
    assert two["avg_bid_depth"] + two["avg_ask_depth"] == pytest.approx(4.0, abs=1e-8)
    assert sweep["sign_flips"] == 1


@pytest.mark.parametrize("grid", [[], [0.0], [1.0, -2.0]])
def test_a_grid_spacing_must_be_positive(grid):
    with pytest.raises(ValueError):
        book_grid_sweep(_book(), "BTC-USD", grid)
