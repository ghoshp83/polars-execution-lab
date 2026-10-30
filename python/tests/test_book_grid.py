import polars as pl
import pytest

from xexeclab.engine import book_grid_across, book_grid_phase, book_grid_sweep

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


def _steady_book() -> pl.DataFrame:
    """Five bid-heavy snapshots: the queue rests on the bid at every spacing."""
    rows = []
    for i in range(5):
        rows += [(i * S, "BTC-USD", "bid", 0, 100.0, 3.0), (i * S, "BTC-USD", "ask", 0, 101.0, 1.0)]
    return pl.DataFrame(rows, schema=_SCHEMA, orient="row")


def test_a_spacing_holds_across_books_only_when_every_book_keeps_its_queue_side():
    across = book_grid_across([_book(), _steady_book()], "BTC-USD", [1.0, 2.0])
    assert len(across["books"]) == 2
    one = across["spacings"][0]
    assert one["sign_kept_books"] == 2
    assert (one["spread_ratio_min"], one["spread_ratio_max"]) == (1.0, 1.0)
    assert one["imbalance_drift_max"] == 0.0
    # At two seconds the first book flips and the steady one does not.
    two = across["spacings"][1]
    assert two["sign_kept_books"] == 1
    assert two["spread_ratio_min"] == pytest.approx(0.625, abs=1e-8)
    assert two["spread_ratio_max"] == 1.0
    first = across["books"][0]
    drift = abs(first["rows"][1]["avg_depth_imbalance"] - first["full"]["avg_depth_imbalance"])
    assert two["imbalance_drift_max"] == pytest.approx(drift, abs=1e-8)
    assert two["imbalance_drift_max"] > 0.0
    assert across["held_spacings"] == 1
    # Leaving out the flipping book frees its spacing; leaving out the steady
    # one frees nothing.
    assert across["held_without"] == [2, 1]


def test_each_spacing_names_the_books_that_lose_their_queue_side():
    across = book_grid_across([_book(), _steady_book()], "BTC-USD", [1.0, 2.0])
    assert across["spacings"][0]["sign_lost_by"] == []
    assert across["spacings"][1]["sign_lost_by"] == [0]
    # The names follow input order, not the book: swap them and the index moves.
    swapped = book_grid_across([_steady_book(), _book()], "BTC-USD", [1.0, 2.0])
    assert swapped["spacings"][1]["sign_lost_by"] == [1]
    # Kept and lost partition the books at every spacing.
    for s in across["spacings"] + swapped["spacings"]:
        assert s["sign_kept_books"] + len(s["sign_lost_by"]) == 2


@pytest.mark.parametrize("books", [[], [_book()]])
def test_a_grid_across_books_needs_two_of_them(books):
    with pytest.raises(ValueError):
        book_grid_across(books, "BTC-USD", [1.0])


@pytest.mark.parametrize("grid", [[float("inf")], [float("nan")], [1.0, 2.0, 1.0]])
def test_a_grid_spacing_must_be_finite_and_listed_once(grid):
    # Infinity used to overflow here, while Rust saturated and answered.
    with pytest.raises(ValueError):
        book_grid_sweep(_book(), "BTC-USD", grid)


def test_a_repeated_spacing_is_not_counted_twice_as_held():
    with pytest.raises(ValueError):
        book_grid_across([_book(), _steady_book()], "BTC-USD", [1.0, 1.0])


def test_where_a_grid_starts_can_decide_which_side_the_queue_reports():
    phase = book_grid_phase(_book(), "BTC-USD", [2.0], 2)
    two = phase["spacings"][0]
    # From 0 s it lands on 0, 2, 4 (-1/6); from 1 s on 1, 3 (+0.5). Same book.
    assert two["phases_run"] == 2
    assert (two["snapshots_min"], two["snapshots_max"]) == (2, 3)
    assert two["sign_kept_phases"] == 1
    assert two["first_phase_kept"] is False
    assert two["queue_imbalance_min"] == pytest.approx(-1 / 6, abs=1e-8)
    assert two["queue_imbalance_max"] == pytest.approx(0.5, abs=1e-8)
    assert (phase["held_spacings"], phase["lost_spacings"], phase["split_spacings"]) == (0, 0, 1)


def test_the_first_start_is_the_grid_book_grid_runs():
    grid = [1.0, 2.0, 3.0]
    phase = book_grid_phase(_book(), "BTC-USD", grid, 3)
    sweep = book_grid_sweep(_book(), "BTC-USD", grid)
    for p, r in zip(phase["spacings"], sweep["rows"], strict=True):
        assert p["first_phase_kept"] == r["queue_sign_kept"]
    assert phase["full"] == sweep["full"]


def test_a_start_past_the_end_of_the_book_is_left_out_and_the_counts_partition():
    # At 10 s and five starts, offsets 2 s and 4 s still land on a 4 s book; 6 s
    # and 8 s fall past its end and keep nothing.
    phase = book_grid_phase(_book(), "BTC-USD", [1.0, 10.0], 5)
    assert phase["spacings"][1]["phases_run"] == 3
    assert phase["spacings"][1]["snapshots_max"] == 1
    assert phase["held_spacings"] + phase["lost_spacings"] + phase["split_spacings"] == 2
    steady = book_grid_phase(_steady_book(), "BTC-USD", [1.0, 2.0], 4)
    assert steady["held_spacings"] == 2
    assert (steady["lost_spacings"], steady["split_spacings"]) == (0, 0)


@pytest.mark.parametrize("grid,phases", [([1.0], 0), ([1.0], 1), ([], 2), ([float("inf")], 2)])
def test_a_phase_sweep_needs_two_starts_and_a_clean_grid(grid, phases):
    with pytest.raises(ValueError):
        book_grid_phase(_book(), "BTC-USD", grid, phases)
