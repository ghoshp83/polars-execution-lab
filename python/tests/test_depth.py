import polars as pl
import pytest

from xexeclab.engine import depth_metrics, queue_metrics, read_book

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


def test_depth_summarises_the_sample_book():
    m = depth_metrics(read_book(SAMPLE), "BTC-USD")
    # Five 3-level snapshots; values hand-checked against the fixture.
    assert m["snapshots"] == 5
    assert m["avg_bid_depth"] == pytest.approx(4.12)
    assert m["avg_ask_depth"] == pytest.approx(3.6)
    assert m["avg_spread"] == pytest.approx(1.4)
    assert -1.0 <= m["avg_depth_imbalance"] <= 1.0


def test_depth_imbalance_is_positive_when_the_bid_rests_heavier():
    # One snapshot: bids total 3.0, asks total 1.0 -> (3-1)/(3+1) = 0.5.
    df = _book(
        [
            (0, "BTC-USD", "bid", 0, 100.0, 2.0),
            (0, "BTC-USD", "bid", 1, 99.5, 1.0),
            (0, "BTC-USD", "ask", 0, 101.0, 0.5),
            (0, "BTC-USD", "ask", 1, 101.5, 0.5),
        ]
    )
    m = depth_metrics(df, "BTC-USD")
    assert m["snapshots"] == 1
    assert m["avg_depth_imbalance"] == pytest.approx(0.5)
    assert m["avg_spread"] == pytest.approx(1.0)


def test_depth_imbalance_flips_sign_when_the_ask_rests_heavier():
    df = _book(
        [
            (0, "BTC-USD", "bid", 0, 100.0, 0.5),
            (0, "BTC-USD", "ask", 0, 101.0, 2.0),
        ]
    )
    m = depth_metrics(df, "BTC-USD")
    assert m["avg_depth_imbalance"] < 0


def test_empty_book_raises():
    with pytest.raises(ValueError):
        depth_metrics(_book([]), "BTC-USD")


def test_queue_reports_the_touch_size_averaged_over_the_window():
    m = queue_metrics(read_book(SAMPLE), "BTC-USD")
    # Best-bid sizes 1.20/0.60/2.00/0.90/1.50 (mean 1.24), best-ask
    # 0.80/1.40/0.50/1.10/1.50 (mean 1.06); hand-checked against the fixture.
    assert m["snapshots"] == 5
    assert m["avg_bid_queue"] == pytest.approx(1.24)
    assert m["avg_ask_queue"] == pytest.approx(1.06)
    assert m["avg_queue_imbalance"] == pytest.approx(0.06)


def test_queue_ignores_depth_below_the_touch():
    # A huge level-1 bid that dominates depth must NOT move the queue: queue
    # position is the size at level 0 alone. Best bid 2.0 vs best ask 0.5 ->
    # imbalance (2-0.5)/(2+0.5) = 0.6.
    df = _book(
        [
            (0, "BTC-USD", "bid", 0, 100.0, 2.0),
            (0, "BTC-USD", "bid", 1, 99.5, 50.0),
            (0, "BTC-USD", "ask", 0, 101.0, 0.5),
        ]
    )
    m = queue_metrics(df, "BTC-USD")
    assert m["avg_bid_queue"] == pytest.approx(2.0)
    assert m["avg_ask_queue"] == pytest.approx(0.5)
    assert m["avg_queue_imbalance"] == pytest.approx(0.6)


def test_queue_empty_book_raises():
    with pytest.raises(ValueError):
        queue_metrics(_book([]), "BTC-USD")
