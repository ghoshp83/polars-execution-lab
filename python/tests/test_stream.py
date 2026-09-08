"""The streamed session must be the *same* session.

`stream_session` is a different route to the answer the in-memory pass already
gives, computed without ever holding the capture. These tests pin both halves
of that claim: the numbers agree with the in-memory pass, and the residency
stays bounded by the chunk rather than by the file.
"""

import polars as pl
import pytest

from xexeclab.engine import order_flow, read_ticks, session_twap, session_vwap, stream_session

# Six ticks, in time order, with uneven gaps so the sample-and-hold TWAP is not
# the plain mean and a lost interval would show up as a wrong number.
_TICKS = [
    '{"ts_ns":1000,"product":"BTC-USD","price":30000.0,"size":0.5,"side":"buy","trade_id":1}',
    '{"ts_ns":2000,"product":"BTC-USD","price":30010.0,"size":1.5,"side":"sell","trade_id":2}',
    '{"ts_ns":5000,"product":"BTC-USD","price":29990.0,"size":0.25,"side":"buy","trade_id":3}',
    '{"ts_ns":6000,"product":"BTC-USD","price":30020.0,"size":2.0,"side":"buy","trade_id":4}',
    '{"ts_ns":9000,"product":"BTC-USD","price":30005.0,"size":0.75,"side":"sell","trade_id":5}',
    '{"ts_ns":9500,"product":"BTC-USD","price":30015.0,"size":1.0,"side":"buy","trade_id":6}',
]


def _replay(tmp_path, name: str, lines: list[str]) -> str:
    p = tmp_path / f"{name}.ndjson"
    p.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return str(p)


def test_the_streamed_session_equals_the_in_memory_session(tmp_path):
    path = _replay(tmp_path, "same", _TICKS)
    df = read_ticks(path)
    buy, sell, imbalance = order_flow(df)

    s = stream_session(path, 2)

    # The point of the release: folding the file changes the memory profile,
    # not the answer.
    assert s["vwap"] == session_vwap(df)
    assert s["twap"] == session_twap(df)
    assert s["buy_volume"] == buy
    assert s["sell_volume"] == sell
    assert s["imbalance"] == imbalance
    assert s["rows"] == df.height
    assert s["product"] == "BTC-USD"
    assert s["first_ts_ns"] == 1000
    assert s["last_ts_ns"] == 9500


def test_the_chunk_size_does_not_change_the_answer(tmp_path):
    path = _replay(tmp_path, "chunks", _TICKS)
    one = stream_session(path, 1)

    # A chunk size is an operational knob -- how much memory to spend -- and an
    # operational knob that moves the reported VWAP is a bug, not a setting.
    for n in (2, 3, 4, 5, 6, 1000):
        s = stream_session(path, n)
        for field in ("vwap", "twap", "volume", "notional", "imbalance", "rows"):
            assert s[field] == one[field], f"{field} moved at chunk_rows={n}"


def test_the_interval_spanning_a_chunk_boundary_is_still_priced(tmp_path):
    path = _replay(tmp_path, "boundary", _TICKS)
    whole = session_twap(read_ticks(path))

    # With chunk_rows = 1 every interval spans a boundary. If the carried tick
    # were dropped the TWAP would have no intervals left at all; if it were
    # dropped only sometimes the answer would drift. Either way this fails.
    assert stream_session(path, 1)["twap"] == whole
    assert stream_session(path, 5)["twap"] == whole


def test_memory_stays_bounded_by_the_chunk_not_the_file(tmp_path):
    path = _replay(tmp_path, "bounded", _TICKS)

    small = stream_session(path, 2)
    assert small["chunks"] == 3
    # One chunk, plus the tick carried across the boundary.
    assert small["peak_rows_in_memory"] == 3
    assert small["peak_rows_in_memory"] < small["rows"]

    # A chunk wider than the file is one chunk with nothing to carry, so the
    # bound degrades to the file itself -- the honest report, not a flattering
    # one.
    whole = stream_session(path, 1000)
    assert whole["chunks"] == 1
    assert whole["peak_rows_in_memory"] == whole["rows"]


def test_the_fold_agrees_with_the_polars_streaming_engine(tmp_path):
    """Validate the hand-rolled fold against Polars' own out-of-core engine.

    Polars can compute the session sums itself without materialising the file,
    via `scan_ndjson(...).collect(engine="streaming")`. That covers the sums but
    not the sample-and-hold TWAP's cross-chunk carry, which is why the fold
    exists -- so the sums are checked against the engine that does have a
    streaming implementation, and the rest against the in-memory pass.
    """
    path = _replay(tmp_path, "engine", _TICKS)
    ref = (
        pl.scan_ndjson(path)
        .select(
            (pl.col("price") * pl.col("size")).sum().alias("pv"),
            pl.col("size").sum().alias("v"),
        )
        .collect(engine="streaming")
    )

    s = stream_session(path, 2)
    assert s["notional"] == pytest.approx(ref["pv"][0], abs=1e-8)
    assert s["volume"] == pytest.approx(ref["v"][0], abs=1e-8)
    assert s["vwap"] == pytest.approx(ref["pv"][0] / ref["v"][0], abs=1e-8)


def test_a_capture_out_of_time_order_is_refused(tmp_path):
    path = _replay(
        tmp_path,
        "unordered",
        [
            '{"ts_ns":5000,"product":"BTC-USD","price":30000.0,"size":1.0,"side":"buy","trade_id":1}',
            '{"ts_ns":1000,"product":"BTC-USD","price":30010.0,"size":1.0,"side":"buy","trade_id":2}',
        ],
    )

    # The in-memory path sorts; a streamed pass cannot sort what it has not seen
    # yet. Rather than quietly price a shuffled capture as if the gaps were
    # real, it refuses.
    with pytest.raises(ValueError, match="not in time order"):
        stream_session(path, 8)


def test_a_chunk_of_no_rows_is_rejected(tmp_path):
    path = _replay(tmp_path, "zerochunk", _TICKS)
    with pytest.raises(ValueError, match="chunk_rows must be >= 1"):
        stream_session(path, 0)


def test_a_columnar_sink_is_not_a_stream(tmp_path):
    # Parquet holds the same rows, but not in a form a line-oriented fold can
    # walk. Refusing by extension is cheaper and clearer than failing on the
    # first non-UTF-8 byte.
    with pytest.raises(ValueError, match="streaming reads NDJSON only"):
        stream_session(str(tmp_path / "ticks.parquet"), 4)


def test_an_empty_capture_is_refused(tmp_path):
    path = _replay(tmp_path, "empty", ["", ""])
    with pytest.raises(ValueError, match="no ticks in"):
        stream_session(path, 4)


def test_a_single_tick_cannot_make_a_twap(tmp_path):
    path = _replay(tmp_path, "single", [_TICKS[0]])

    # The same refusal `session_twap` gives, worded the same way: one trade has
    # no following interval, so there is no time to weight it by.
    with pytest.raises(ValueError, match="need >= 2 ticks for twap"):
        stream_session(path, 4)
