"""The cap x half-life grid -- mirrors `tests/stability.rs` case for case."""

from __future__ import annotations

import json

import polars as pl
import pytest

from xexeclab.engine import hl_sweep, stability

BUCKET = 1_000_000_000
# An hour between sessions, so captures only line up by time into the session.
HOUR = 3_600 * BUCKET
HALF_LIVES = [0.25, 0.5, 1.0, 2.0, 4.0]


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
    """Two even buckets and a thin one, so a tight cap binds in the *even* buckets."""
    return _session(4 * HOUR, [1.0, 1.0, 0.02])


def test_every_row_is_the_half_life_sweep_at_that_cap():
    """The grid is a re-run, not a new calculation: every row must be exactly the
    half-life sweep ``pov-hl-sweep`` would print at that cap."""
    history = [_blended(), _lumpy()]
    caps = [0.2, 0.3, 0.5]
    grid = stability(history, _thin_tail(), BUCKET, 1.0, caps, HALF_LIVES, 10.0, 2.0)
    assert grid["half_lives"] == HALF_LIVES
    assert len(grid["rows"]) == len(caps)
    for row, cap in zip(grid["rows"], caps, strict=True):
        s = hl_sweep(history, _thin_tail(), BUCKET, 1.0, cap, HALF_LIVES, 10.0, 2.0)
        assert row["cap"] == cap
        assert row["improvement_bps"] == [p["improvement_bps"] for p in s["points"]]
        assert row["flat_improvement_bps"] == s["flat_improvement_bps"]
        assert row["improvement_span_bps"] == s["improvement_span_bps"]
        assert row["sign_stable"] == s["sign_stable"]
        assert row["inert"] == s["inert"]


@pytest.mark.parametrize("parent", [1.0, 0.6])
def test_a_cap_that_binds_both_plans_alike_is_inert_and_not_counted_stable(parent):
    """**Stable for want of a sign is not stable.** At a 1% cap both plans are
    bound in every bucket and trade identically, so every cell is exactly zero.
    ``sign_stable`` is true there only because there is no sign at all; counting
    that row in ``stable_caps`` would let a cap at which pooling did nothing vouch
    for pooling. The same cap on a smaller parent is not inert, so the flag is
    read off the run and not off the cap."""
    grid = stability(
        [_blended(), _lumpy()],
        _thin_tail(),
        BUCKET,
        parent,
        [0.01, 0.3, 0.5],
        HALF_LIVES,
        25.0,
        5.0,
    )
    if parent == 0.6:
        assert not grid["rows"][0]["inert"], grid["rows"][0]["improvement_bps"]
        assert grid["inert_caps"] == 0
        return
    assert [r["inert"] for r in grid["rows"]] == [True, False, False]
    row = grid["rows"][0]
    assert all(v == 0.0 for v in row["improvement_bps"])
    assert row["flat_improvement_bps"] == 0.0
    assert row["sign_stable"], "an inert row has no sign to flip"
    assert grid["inert_caps"] == 1
    assert grid["stable_caps"] == 2
    assert grid["unstable_caps"] == 0
    assert grid["all_sign_stable"] is True


def test_inert_stable_and_unstable_partition_the_grid():
    """The three counts partition the grid, like ``cap_sweep``'s: every row lands
    in exactly one. The fixture has one row of each kind, so a count that
    double-books a row fails here."""
    grid = stability(
        [_blended(), _lumpy()], _pinched(), BUCKET, 1.0, [0.01, 0.3, 0.5], HALF_LIVES, 25.0, 5.0
    )
    assert (grid["inert_caps"], grid["stable_caps"], grid["unstable_caps"]) == (1, 1, 1), (
        "the fixture no longer has one row of each kind"
    )
    for r in grid["rows"]:
        kinds = [r["inert"], r["sign_stable"] and not r["inert"], not r["sign_stable"]]
        assert sum(1 for k in kinds if k) == 1, f"cap {r['cap']}"
    assert grid["all_sign_stable"] is False


def test_a_ladder_that_is_stable_at_one_cap_and_not_another_is_reported_mixed():
    """**The finding the grid exists for.** On the same sessions the half-life
    decides the sign at one cap and not at the other, so stability is a property
    of the cap as well as the sessions -- which a single half-life sweep, run at
    one cap, cannot show."""
    grid = stability(
        [_blended(), _thin_tail()], _pinched(), BUCKET, 0.6, [0.3, 0.5], HALF_LIVES, 25.0, 5.0
    )
    flags = [r["sign_stable"] for r in grid["rows"]]
    assert flags == [True, False], (
        "the fixture no longer splits, so it proves nothing: "
        f"{[r['improvement_bps'] for r in grid['rows']]}"
    )
    assert grid["inert_caps"] == 0
    assert grid["stable_caps"] == 1
    assert grid["unstable_caps"] == 1
    assert grid["inert_caps"] + grid["stable_caps"] + grid["unstable_caps"] == len(grid["rows"])
    assert grid["all_sign_stable"] is False


def test_a_ladder_with_no_flip_is_all_sign_stable():
    """``all_sign_stable`` is the one-word answer, so it has to be ``True`` when
    no row flipped."""
    grid = stability(
        [_blended(), _lumpy()], _thin_tail(), BUCKET, 1.0, [0.3, 0.5], HALF_LIVES, 10.0, 2.0
    )
    assert grid["unstable_caps"] == 0
    assert grid["all_sign_stable"] is True


def test_the_grid_names_no_best_cell():
    """**The refusal, pinned.** A two-parameter grid scored on the session it is
    fitted to must not name a best cell: it would read as a recommendation
    however it was documented. Checked on the serialised report, so a field added
    later under any of these names fails here."""
    grid = stability(
        [_blended(), _thin_tail()], _pinched(), BUCKET, 0.6, [0.3, 0.5], HALF_LIVES, 25.0, 5.0
    )
    text = json.dumps(grid)
    for word in ("best", "worst", "optimal", "argmax", "recommend"):
        assert word not in text, f"{word} appeared in {text}"


def test_a_single_cap_is_refused():
    """One cap is not a grid; ``pov-hl-sweep`` already answers that question."""
    with pytest.raises(ValueError, match="at least 2 points"):
        stability([_blended(), _lumpy()], _thin_tail(), BUCKET, 1.0, [0.3], HALF_LIVES, 10.0, 2.0)


@pytest.mark.parametrize(
    ("caps", "want"),
    [([0.5, 0.3], "strictly increasing"), ([0.5, 1.5], r"\(0, 1\]"), ([0.0, 0.5], r"\(0, 1\]")],
)
def test_a_cap_grid_out_of_range_or_out_of_order_is_refused(caps, want):
    """The cap grid follows ``cap_sweep``'s rules, so a grid one command accepts
    the other does too."""
    with pytest.raises(ValueError, match=want):
        stability([_blended(), _lumpy()], _thin_tail(), BUCKET, 1.0, caps, HALF_LIVES, 10.0, 2.0)


def test_a_half_life_grid_containing_zero_is_refused_with_the_sweeps_reason():
    """The half-life grid is validated by ``hl_sweep``, not re-implemented here,
    so its refusal -- including the reason zero is refused -- reaches the caller."""
    with pytest.raises(ValueError, match="flat_improvement_bps"):
        stability(
            [_blended(), _lumpy()], _thin_tail(), BUCKET, 1.0, [0.3, 0.5], [0.0, 1.0], 10.0, 2.0
        )
