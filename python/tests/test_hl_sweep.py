"""The pooling half-life swept at a fixed cap -- mirrors `tests/hl_sweep.rs` case for case."""

from __future__ import annotations

import polars as pl
import pytest

from xexeclab.engine import hl_sweep, pov_forecast

BUCKET = 1_000_000_000
# An hour between sessions, so captures only line up by time into the session.
HOUR = 3_600 * BUCKET


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


def test_every_point_is_the_forecast_at_that_half_life():
    """The sweep is a re-run, not a new calculation: every point must equal what
    ``pov_forecast`` reports at that half-life on its own, including the weight
    vector the pooling produced."""
    history = [_blended(), _lumpy()]
    grid = [0.5, 1.0, 2.0]
    sweep = hl_sweep(history, _thin_tail(), BUCKET, 1.0, 0.3, grid, 10.0, 2.0)
    assert len(sweep["points"]) == len(grid)
    for point, hl in zip(sweep["points"], grid, strict=True):
        fc = pov_forecast(history, _thin_tail(), BUCKET, 1.0, 0.3, 10.0, 2.0, hl, None)
        gain = fc["capped_improvement"]["capped"]
        assert point["half_life"] == hl
        assert point["newest_weight"] == fc["weights"][-1]
        assert point["improvement_bps"] == gain["improvement_bps"]
        assert point["shortfall_qty"] == gain["shortfall_qty"]
        assert point["like_for_like"] == gain["like_for_like"]
        assert point["breakeven_bps"] == gain["breakeven_bps"]


def test_a_short_half_life_is_the_naive_plan():
    """**The left-hand limit, pinned.** A half-life short enough puts essentially
    all the weight on the most recent session, which is precisely the naive plan
    the forecast is scored against -- so the two plans coincide and there is
    nothing left to improve. A sweep that does not bottom out here has not been
    run short enough to see its own floor, and the docstring says so; this is the
    test that keeps that claim true."""
    sweep = hl_sweep(
        [_blended(), _lumpy()], _thin_tail(), BUCKET, 1.0, 0.3, [0.01, 0.02], 10.0, 2.0
    )
    for p in sweep["points"]:
        assert p["newest_weight"] == 1.0, f"half-life {p['half_life']}"
        assert p["improvement_bps"] == 0.0, f"half-life {p['half_life']}"
        assert p["like_for_like"], f"half-life {p['half_life']}"
    assert sweep["improvement_span_bps"] == 0.0
    assert sweep["sign_stable"]


def test_a_long_half_life_converges_on_the_flat_pool():
    """**The right-hand limit, pinned.** As the half-life grows the decay flattens
    and the pooling converges on the equal-weight pool that ``half_life = 0``
    computes directly. ``flat_improvement_bps`` is reported so a reader can see
    which limit the grid is heading for; this asserts it really is the limit, and
    that the grid approaches it rather than wandering."""
    sweep = hl_sweep([_blended(), _lumpy()], _thin_tail(), BUCKET, 1.0, 0.3, [1.0, 1e6], 10.0, 2.0)
    flat = sweep["flat_improvement_bps"]
    near = abs(sweep["points"][0]["improvement_bps"] - flat)
    far = abs(sweep["points"][1]["improvement_bps"] - flat)
    assert far <= near, f"the grid moved away from the flat pool: {far} > {near}"
    assert far < 1e-6, f"1e6 sessions of half-life is still {far} from flat"


def test_the_span_and_the_extremes_describe_the_points():
    """The three summary figures are read off the points, so they have to agree
    with them -- a reader who compares the span against the column of
    ``improvement_bps`` must not find a different range there."""
    sweep = hl_sweep(
        [_blended(), _lumpy()],
        _thin_tail(),
        BUCKET,
        1.0,
        0.3,
        [0.25, 0.5, 1.0, 2.0, 4.0],
        10.0,
        2.0,
    )
    gains = [p["improvement_bps"] for p in sweep["points"]]
    assert sweep["improvement_min_bps"] == min(gains)
    assert sweep["improvement_max_bps"] == max(gains)
    assert sweep["improvement_span_bps"] == max(gains) - min(gains)

    # ``best``/``worst`` name grid points, not values off the end of it.
    def at(hl):
        for p in sweep["points"]:
            if p["half_life"] == hl:
                return p
        raise AssertionError(f"{hl} is not a grid point")

    assert at(sweep["best_half_life"])["improvement_bps"] == max(gains)
    assert at(sweep["worst_half_life"])["improvement_bps"] == min(gains)


def test_a_sign_that_flips_across_the_grid_is_reported_unstable():
    """**The finding the sweep exists for.** Run wide enough and the same sessions
    say pooling cost money at one half-life and paid at another. ``sign_stable``
    is false there, and it must be: a desk that read a single run would have taken
    the sign of an unfitted guess for a property of the market."""
    sweep = hl_sweep(
        [_blended(), _thin_tail()],
        _pinched(),
        BUCKET,
        0.6,
        0.5,
        [0.25, 0.5, 1.0, 2.0, 4.0],
        25.0,
        5.0,
    )
    gains = [p["improvement_bps"] for p in sweep["points"]]
    assert any(g > 0.0 for g in gains) and any(g < 0.0 for g in gains), (
        f"the fixture no longer flips sign, so it proves nothing: {gains}"
    )
    assert not sweep["sign_stable"]
    assert sweep["improvement_span_bps"] > 0.0


def test_a_single_half_life_is_refused():
    """One point is not a sweep."""
    with pytest.raises(ValueError, match="at least 2 points"):
        hl_sweep([_blended(), _lumpy()], _thin_tail(), BUCKET, 1.0, 0.3, [1.0], 10.0, 2.0)


def test_a_grid_that_does_not_increase_is_refused():
    """The grid is read as an ordering -- naive end to flat end -- so it has to be one."""
    with pytest.raises(ValueError, match="strictly increasing"):
        hl_sweep([_blended(), _lumpy()], _thin_tail(), BUCKET, 1.0, 0.3, [2.0, 1.0], 10.0, 2.0)


@pytest.mark.parametrize("grid", [[0.0, 0.5], [-1.0, 0.5]])
def test_a_half_life_of_zero_or_less_is_refused(grid):
    """Zero is the one value that would silently corrupt the sweep rather than
    merely being wrong: ``pov_forecast`` reads it as the equal-weight pool, which
    is the limit the *long* end of the grid approaches, so accepting it at the
    short end would invert the ordering the whole report is written against. It is
    refused, and ``flat_improvement_bps`` is where that case is reported."""
    with pytest.raises(ValueError, match="positive and finite"):
        hl_sweep([_blended(), _lumpy()], _thin_tail(), BUCKET, 1.0, 0.3, grid, 10.0, 2.0)
