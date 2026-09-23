"""The breakeven rate swept across caps -- mirrors `tests/cap_sweep.rs` case for case."""

from __future__ import annotations

import polars as pl
import pytest

from xexeclab.engine import cap_sweep, pov_forecast

BUCKET = 1_000_000_000
# An hour between sessions, so captures only line up by time into the session.
HOUR = 3_600 * BUCKET
# Pool every session equally.
FLAT = 0.0


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


def _session(start: int, volumes: list[float]) -> pl.DataFrame:
    """Three one-tick buckets starting at ``start`` with the given volumes."""
    return _ticks([(start + i * BUCKET, 100.0 + i, v) for i, v in enumerate(volumes)])


def _lumpy() -> pl.DataFrame:
    """Shares 0.1 / 0.6 / 0.3."""
    return _session(0, [1.0, 6.0, 3.0])


def _blended() -> pl.DataFrame:
    """Half of each: 0.35 / 0.35 / 0.3."""
    return _session(2 * HOUR, [3.5, 3.5, 3.0])


def _thin_tail() -> pl.DataFrame:
    """A session whose volume collapses after the first bucket: only the tail binds."""
    return _session(3 * HOUR, [10.0, 1.0, 0.6])


def _pinched() -> pl.DataFrame:
    """Two even buckets and a thin one, so a tight cap binds in the *even* buckets
    too -- the shape that produces a genuine trade-off rather than a dominance."""
    return _session(4 * HOUR, [1.0, 1.0, 0.02])


def test_every_point_is_the_forecast_at_that_cap():
    """The sweep is a re-run, not a new calculation: every point must equal what
    ``pov_forecast`` reports at that cap on its own. If these ever drift apart
    the sweep has started computing its own answer, which is the one thing it
    must never do."""
    history = [_blended(), _lumpy()]
    grid = [0.1, 0.2, 0.3]
    sweep = cap_sweep(history, _thin_tail(), BUCKET, 1.0, grid, 10.0, 2.0, FLAT)
    assert len(sweep["points"]) == len(grid)
    for point, cap in zip(sweep["points"], grid, strict=True):
        fc = pov_forecast(history, _thin_tail(), BUCKET, 1.0, cap, 10.0, 2.0, FLAT, None)
        gain = fc["capped_improvement"]["capped"]
        assert point["cap"] == cap
        assert point["improvement_bps"] == gain["improvement_bps"]
        assert point["shortfall_qty"] == gain["shortfall_qty"]
        assert point["like_for_like"] == gain["like_for_like"]
        assert point["breakeven_bps"] == gain["breakeven_bps"]
        assert point["forecast_unfilled_qty"] == fc["forecast_capped"]["capped"]["unfilled_qty"]
        assert point["naive_unfilled_qty"] == fc["naive_capped"]["capped"]["unfilled_qty"]


@pytest.mark.parametrize("exec_df", [_thin_tail(), _pinched()])
def test_the_three_counts_partition_the_grid(exec_df):
    """A point cannot be both like-for-like and dominated, and one that has a
    rate is neither -- so the counts must sum to the grid. A reader who adds
    them up and gets more than the grid is being told the same cap twice."""
    sweep = cap_sweep(
        [_blended(), _lumpy()], exec_df, BUCKET, 1.0, [0.1, 0.2, 0.3, 0.4, 0.5], 10.0, 2.0, FLAT
    )
    assert sweep["like_for_like_points"] + sweep["dominated_points"] + sweep[
        "traded_off_points"
    ] == len(sweep["points"])
    for p in sweep["points"]:
        flags = [p["like_for_like"], p["dominated"], p["breakeven_bps"] is not None]
        assert sum(1 for f in flags if f) == 1, f"cap {p['cap']}"


def test_where_only_the_tail_binds_no_cap_needs_a_rate():
    """**The dominance finding, stated as a property rather than a comment.**

    Under the forward carry a plan that gets more away early both misses less
    *and* pays less per unit filled: the extra fill lands in the
    low-participation slots and raises their weight in the average. So in a
    session where only the thin tail binds, one plan dominates outright at every
    cap and there is no rate to price -- ``breakeven_bps`` is ``None`` by
    construction, not by accident."""
    sweep = cap_sweep(
        [_blended(), _lumpy()],
        _thin_tail(),
        BUCKET,
        1.0,
        [0.1, 0.2, 0.3, 0.4, 0.5],
        10.0,
        2.0,
        FLAT,
    )
    # The cap really does bind somewhere, or the test proves nothing.
    assert any(not p["like_for_like"] for p in sweep["points"])
    assert sweep["traded_off_points"] == 0
    assert sweep["breakeven_min_bps"] is None
    assert sweep["breakeven_max_bps"] is None
    for p in sweep["points"]:
        assert p["breakeven_bps"] is None, f"cap {p['cap']} priced a rate"


def test_a_cap_that_bites_in_the_even_buckets_does_need_a_rate():
    """The contrast that makes the previous test a finding and not a bug: bind
    the cap in the *even* buckets as well and a real trade-off appears, with a
    rate attached to it."""
    sweep = cap_sweep(
        [_blended(), _lumpy()], _pinched(), BUCKET, 1.0, [0.2, 0.3, 0.4], 10.0, 2.0, FLAT
    )
    assert sweep["traded_off_points"] > 0, "no cap in the grid produced a trade-off"
    lo = sweep["breakeven_min_bps"]
    hi = sweep["breakeven_max_bps"]
    for p in sweep["points"]:
        if p["breakeven_bps"] is not None:
            assert p["breakeven_bps"] > 0.0
            assert lo <= p["breakeven_bps"] <= hi


def test_a_grid_the_session_can_absorb_is_like_for_like_throughout():
    """``like_for_like_from_cap`` is a threshold, so it must be read from the top
    of the grid down. A cap slack enough that neither plan misses anything gives
    the whole grid back."""
    sweep = cap_sweep(
        [_blended(), _lumpy()], _lumpy(), BUCKET, 0.1, [0.5, 0.75, 1.0], 10.0, 2.0, FLAT
    )
    assert sweep["like_for_like_points"] == len(sweep["points"])
    assert sweep["dominated_points"] == 0
    assert sweep["traded_off_points"] == 0
    assert sweep["like_for_like_from_cap"] == 0.5


def test_a_single_cap_is_refused():
    """One point is not a sweep."""
    with pytest.raises(ValueError, match="at least 2 points"):
        cap_sweep([_blended(), _lumpy()], _thin_tail(), BUCKET, 1.0, [0.25], 10.0, 2.0, FLAT)


def test_a_grid_that_does_not_increase_is_refused():
    """The grid is read as an ordering, so it has to be one."""
    with pytest.raises(ValueError, match="strictly increasing"):
        cap_sweep([_blended(), _lumpy()], _thin_tail(), BUCKET, 1.0, [0.3, 0.2], 10.0, 2.0, FLAT)


@pytest.mark.parametrize("grid", [[0.0, 0.5], [0.5, 1.5]])
def test_a_cap_outside_the_unit_interval_is_refused(grid):
    """A cap is a share of volume; outside ``(0, 1]`` it is not one."""
    with pytest.raises(ValueError, match=r"in \(0, 1\]"):
        cap_sweep([_blended(), _lumpy()], _thin_tail(), BUCKET, 1.0, grid, 10.0, 2.0, FLAT)
