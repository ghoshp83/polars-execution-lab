import polars as pl
import pytest

from xexeclab.engine import quote_metrics, read_quotes

SAMPLE = "data/sample_quotes.ndjson"


def test_spread_positive_and_mid_between_bid_and_ask():
    df = read_quotes(SAMPLE)
    m = quote_metrics(df, "BTC-USD")
    assert m["quotes"] == df.height
    assert m["avg_spread"] > 0.0
    assert df["bid"].min() <= m["avg_mid"] <= df["ask"].max()


def test_book_imbalance_bounded_and_tracks_resting_size():
    df = read_quotes(SAMPLE)
    m = quote_metrics(df, "BTC-USD")
    assert -1.0 <= m["avg_book_imbalance"] <= 1.0
    bid_sz = df["bid_size"].sum()
    ask_sz = df["ask_size"].sum()
    if bid_sz != ask_sz:
        # Sign must agree with which side rests more size -- the metric's intent.
        assert (m["avg_book_imbalance"] > 0.0) == (bid_sz > ask_sz)


def test_microprice_leans_toward_the_thinner_side():
    # Heavier ask than bid -> microprice weights toward the bid, below the mid.
    df = pl.DataFrame(
        {
            "ts_ns": [0],
            "product": ["BTC-USD"],
            "bid": [100.0],
            "bid_size": [1.0],
            "ask": [102.0],
            "ask_size": [3.0],
        }
    )
    m = quote_metrics(df, "BTC-USD")
    assert m["avg_mid"] == 101.0
    assert m["avg_microprice"] < m["avg_mid"]


def test_empty_quotes_raise():
    empty = read_quotes(SAMPLE).clear()
    with pytest.raises(ValueError):
        quote_metrics(empty, "BTC-USD")
