from xexeclab.engine import bars, read_ticks, session_twap, session_vwap, summary

SAMPLE = "data/sample_ticks.ndjson"
BUCKET_1S = 1_000_000_000


def test_bars_conserve_volume_and_are_valid_ohlc():
    df = read_ticks(SAMPLE)
    b = bars(df, BUCKET_1S)
    # The sample spans three one-second buckets.
    assert b.height == 3
    assert abs(b["volume"].sum() - df["size"].sum()) < 1e-6
    for row in b.iter_rows(named=True):
        assert row["high"] >= row["low"]
        assert row["low"] <= row["open"] <= row["high"]
        assert row["low"] <= row["close"] <= row["high"]
        assert row["low"] <= row["vwap"] <= row["high"]


def test_vwap_weights_by_size_not_count():
    df = read_ticks(SAMPLE)
    vwap = session_vwap(df)
    mean = df["price"].mean()
    # With non-uniform sizes the size-weighted VWAP must not equal the plain mean.
    assert abs(vwap - mean) > 1e-9
    assert df["price"].min() <= vwap <= df["price"].max()


def test_twap_differs_from_vwap_and_summary_consistent():
    df = read_ticks(SAMPLE)
    assert session_twap(df) != session_vwap(df)
    s = summary(df, "BTC-USD", BUCKET_1S)
    assert s["ticks"] == df.height
    assert len(s["bars"]) == 3
    assert s["vwap"] == session_vwap(df)
    assert s["twap"] == session_twap(df)
