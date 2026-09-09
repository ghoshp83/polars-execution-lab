"""Participation-of-volume scheduling -- mirrors `tests/pov.rs` case for case."""

from __future__ import annotations

import polars as pl
import pytest

from xexeclab.engine import pov_schedule, session_vwap

BUCKET = 1_000_000_000


def _ticks(rows: list[tuple[int, float, float]]) -> pl.DataFrame:
    return pl.DataFrame(
        {
            "ts_ns": [r[0] for r in rows],
            "product": ["BTC-USD"] * len(rows),
            "price": [r[1] for r in rows],
            "size": [r[2] for r in rows],
            "side": ["buy"] * len(rows),
            "trade_id": [r[0] for r in rows],
        },
        schema={
            "ts_ns": pl.Int64,
            "product": pl.Utf8,
            "price": pl.Float64,
            "size": pl.Float64,
            "side": pl.Utf8,
            "trade_id": pl.Int64,
        },
    )


def _lumpy() -> pl.DataFrame:
    """Three buckets holding 1.0, 6.0 and 3.0 of volume."""
    return _ticks(
        [
            (0, 100.0, 1.0),
            (BUCKET, 102.0, 4.0),
            (BUCKET + 500_000_000, 104.0, 2.0),
            (2 * BUCKET, 110.0, 3.0),
        ]
    )


def test_the_plan_takes_the_same_share_of_every_bucket():
    plan = pov_schedule(_lumpy(), "BTC-USD", BUCKET, 1.0, 0.5, 10.0)
    assert plan["buckets"] == 3
    assert plan["total_volume"] == 10.0
    # The property the whole allocation rests on: one participation, everywhere.
    assert plan["participation"] == 0.1
    assert [s["participation"] for s in plan["schedule"]] == [0.1, 0.1, 0.1]


def test_the_allocation_follows_the_volume_profile():
    plan = pov_schedule(_lumpy(), "BTC-USD", BUCKET, 1.0, 0.5, 10.0)
    sizes = [s["size"] for s in plan["schedule"]]
    assert sizes == [0.1, 0.6, 0.3]
    assert sum(sizes) == pytest.approx(plan["parent_qty"], abs=1e-9)


def test_following_the_volume_tracks_the_session_vwap_exactly():
    df = _lumpy()
    plan = pov_schedule(df, "BTC-USD", BUCKET, 1.0, 0.5, 10.0)
    # The reason to follow volume rather than the clock -- asserted exactly.
    assert plan["pov_price"] == session_vwap(df)
    assert plan["pov_tracking_bps"] == 0.0


def test_the_clock_uniform_benchmark_misses_the_session_vwap():
    plan = pov_schedule(_lumpy(), "BTC-USD", BUCKET, 1.0, 0.5, 10.0)
    # A zero here would mean the benchmark had been built the same way as the
    # plan, which would make the whole comparison vacuous.
    assert abs(plan["twap_tracking_bps"]) > 1.0


def test_a_flat_profile_makes_the_two_allocations_agree():
    flat = _ticks([(0, 100.0, 2.0), (BUCKET, 101.0, 2.0), (2 * BUCKET, 102.0, 2.0)])
    plan = pov_schedule(flat, "BTC-USD", BUCKET, 0.6, 0.5, 10.0, 2.0)
    assert plan["pov_price"] == plan["twap_price"]
    assert plan["twap_tracking_bps"] == 0.0
    assert plan["edge_bps"] == 0.0


def test_a_lumpy_profile_makes_following_the_volume_cheaper():
    plan = pov_schedule(_lumpy(), "BTC-USD", BUCKET, 1.0, 0.5, 10.0)
    # The clock-uniform allocation pushes a third of the parent into the
    # thinnest bucket and the square-root law charges for it.
    assert plan["twap_impact_bps"] > plan["pov_impact_bps"]
    assert plan["edge_bps"] > 0.0


def test_the_infeasible_benchmark_is_flagged_not_hidden():
    plan = pov_schedule(_lumpy(), "BTC-USD", BUCKET, 1.0, 0.2, 10.0)
    assert plan["twap_max_participation"] > plan["cap"]
    assert plan["twap_feasible"] is False
    # The plan itself is still inside the cap; only the benchmark is not.
    assert plan["participation"] <= plan["cap"]


def test_an_order_above_the_cap_is_refused():
    with pytest.raises(ValueError, match="above the cap"):
        pov_schedule(_lumpy(), "BTC-USD", BUCKET, 5.0, 0.25, 10.0)


@pytest.mark.parametrize("cap", [0.0, -0.1, 1.5])
def test_a_cap_outside_the_unit_interval_is_refused(cap):
    with pytest.raises(ValueError, match=r"cap must be in \(0, 1\]"):
        pov_schedule(_lumpy(), "BTC-USD", BUCKET, 1.0, cap, 10.0)


def test_a_parent_of_no_size_is_refused():
    with pytest.raises(ValueError, match="parent_qty must be a positive"):
        pov_schedule(_lumpy(), "BTC-USD", BUCKET, 0.0, 0.5, 10.0)


def test_a_capture_of_no_volume_is_refused():
    empty = _ticks([(0, 100.0, 0.0), (BUCKET, 101.0, 0.0)])
    with pytest.raises(ValueError, match="zero traded volume"):
        pov_schedule(empty, "BTC-USD", BUCKET, 1.0, 0.5, 10.0)


def test_an_empty_capture_is_refused():
    with pytest.raises(ValueError, match="no ticks"):
        pov_schedule(_ticks([]), "BTC-USD", BUCKET, 1.0, 0.5, 10.0)


def test_the_bucketing_matches_the_bars_the_engine_already_builds():
    from xexeclab.engine import bars

    df = _lumpy()
    plan = pov_schedule(df, "BTC-USD", BUCKET, 1.0, 0.5, 10.0)
    b = bars(df, BUCKET)
    # The profile is not a second, parallel notion of a time bucket -- it is the
    # same one, so a change to `bars` that broke this would fail here.
    assert [s["bucket_ns"] for s in plan["schedule"]] == b["bucket_ns"].to_list()
    assert [s["volume"] for s in plan["schedule"]] == b["volume"].to_list()
