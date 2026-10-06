"""The time-respecting walk -- mirrors `tests/walk_forward.rs` case for case."""

from __future__ import annotations

import json

import polars as pl
import pytest

from xexeclab.engine import holdout, stability, walk_forward

BUCKET = 1_000_000_000
# An hour between sessions, so captures only line up by time into the session.
HOUR = 3_600 * BUCKET
HALF_LIVES = [0.25, 0.5, 1.0, 2.0, 4.0]
CAPS = [0.01, 0.3, 0.5]

# Shares 0.1 / 0.6 / 0.3.
LUMPY = [1.0, 6.0, 3.0]
# Half of each: 0.35 / 0.35 / 0.3.
BLENDED = [3.5, 3.5, 3.0]
# Volume collapses after the first bucket: only the tail binds.
THIN_TAIL = [10.0, 1.0, 0.6]
# Two even buckets and a thin one, so a tight cap binds in the *even* buckets.
PINCHED = [1.0, 1.0, 0.02]

ARGS = (BUCKET, 0.6, CAPS, HALF_LIVES, 25.0, 5.0)


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


def _session(hour: int, volumes: list[float]) -> pl.DataFrame:
    """Three one-tick buckets with the given volumes, starting ``hour`` hours in."""
    return _ticks([(hour * HOUR + i * BUCKET, 100.0 + i, v) for i, v in enumerate(volumes)])


def _in_order(*shapes: list[float]) -> list[pl.DataFrame]:
    """The shapes in the order given, an hour apart, so the order is the time order."""
    return [_session(i, v) for i, v in enumerate(shapes)]


def test_the_last_fold_is_the_stability_grid_and_the_rotations_last_fold():
    """The walk is a re-run, not a new calculation. Its last fold has every
    other session behind it, so it must be the grid ``pov-stability`` prints for
    the same history and input -- and the one fold the rotation gets right too."""
    sessions = _in_order(LUMPY, BLENDED, THIN_TAIL, PINCHED)
    walk = walk_forward(sessions, *ARGS)
    grid = stability(sessions[:3], sessions[3], *ARGS)
    rotation = holdout(sessions, *ARGS)
    last = walk["folds"][-1]
    assert last["input"] == 3
    assert last["flat_improvement_bps"] == [r["flat_improvement_bps"] for r in grid["rows"]]
    assert last["unstable_caps"] == grid["unstable_caps"]
    assert last["verdicts"] == ["inert", "gain", "unstable"]
    rotated = rotation["folds"][-1]
    assert last["verdicts"] == rotated["verdicts"]
    assert last["flat_improvement_bps"] == rotated["flat_improvement_bps"]
    assert walk["sessions"] == 4
    assert walk["half_lives"] == grid["half_lives"]


def test_a_fold_is_untouched_by_the_sessions_after_its_input():
    """The property the command exists for: a fold cannot see a session later
    than its input. Replace the newest session with a different one and the
    earlier fold must not move by a digit."""
    wa = walk_forward(_in_order(LUMPY, BLENDED, THIN_TAIL, PINCHED), *ARGS)
    wb = walk_forward(_in_order(LUMPY, BLENDED, THIN_TAIL, LUMPY), *ARGS)
    assert [f["input"] for f in wa["folds"]] == [2, 3]
    assert wa["folds"][0]["verdicts"] == wb["folds"][0]["verdicts"]
    assert wa["folds"][0]["flat_improvement_bps"] == wb["folds"][0]["flat_improvement_bps"]
    # The newest session did change, so the last fold is free to differ.
    assert wa["folds"][1]["flat_improvement_bps"] != wb["folds"][1]["flat_improvement_bps"]


def test_the_same_input_reads_differently_once_the_future_is_removed():
    """Why the rotation is not a backtest, measured. Both commands score session
    2 here, but the rotation also pools session 3 into its history. With that
    future session the cap at 0.3 reads ``unstable``; from the past alone it is
    a plain ``loss``. The rotation's verdict was partly the future's."""
    sessions = _in_order(LUMPY, BLENDED, THIN_TAIL, PINCHED)
    walk = walk_forward(sessions, *ARGS)
    rotation = holdout(sessions, *ARGS)
    assert walk["folds"][0]["input"] == 2
    assert rotation["folds"][2]["held_out"] == 2
    assert walk["folds"][0]["verdicts"] == ["inert", "loss", "loss"]
    assert rotation["folds"][2]["verdicts"] == ["inert", "unstable", "unstable"]


def test_a_walk_can_agree_where_the_rotation_does_not():
    """The disagreement can run the other way. Here every step of the walk says
    pooling pays at 0.3 and 0.5, while the rotation over the same four sessions
    calls both caps ``mixed`` -- its dissenting folds are ones that forecast an
    early session from later ones."""
    sessions = _in_order(LUMPY, THIN_TAIL, PINCHED, BLENDED)
    walk = walk_forward(sessions, *ARGS)
    rotation = holdout(sessions, *ARGS)
    for f in walk["folds"]:
        assert f["verdicts"] == ["inert", "gain", "gain"]
    assert walk["consensus"] == ["inert", "gain", "gain"]
    assert walk["agreeing_caps"] == 3
    assert walk["all_agree"] is True
    assert rotation["consensus"] == ["inert", "mixed", "mixed"]


def test_steps_that_disagree_make_the_cap_mixed():
    """A cap two steps read differently is ``mixed``, and the counts follow --
    here the tightest cap is a ``loss`` with two sessions behind it and
    ``inert`` with three, which is the history growing as much as the input
    changing."""
    walk = walk_forward(_in_order(BLENDED, LUMPY, THIN_TAIL, PINCHED), *ARGS)
    assert walk["folds"][0]["verdicts"] == ["loss", "gain", "gain"]
    assert walk["folds"][1]["verdicts"] == ["inert", "gain", "gain"]
    assert walk["consensus"] == ["mixed", "gain", "gain"]
    assert walk["agreeing_caps"] == 2
    assert walk["all_agree"] is False
    for f in walk["folds"]:
        assert f["inert_caps"] + f["stable_caps"] + f["unstable_caps"] == len(CAPS)


def test_sessions_out_of_time_order_are_refused():
    """ "Oldest first" is the whole claim, so it is checked and not assumed: the
    same four captures with two swapped would quietly forecast from the future."""
    sessions = _in_order(LUMPY, BLENDED, THIN_TAIL, PINCHED)
    sessions[1], sessions[2] = sessions[2], sessions[1]
    with pytest.raises(ValueError, match="time order: session 2"):
        walk_forward(sessions, *ARGS)

    # Overlapping is out of order too: session 1 starts inside session 0.
    overlapping = _in_order(LUMPY, BLENDED, THIN_TAIL, PINCHED)
    overlapping[1] = overlapping[1].with_columns(pl.col("ts_ns") - HOUR + BUCKET)
    with pytest.raises(ValueError, match="time order: session 1"):
        walk_forward(overlapping, *ARGS)


def test_fewer_than_four_sessions_is_refused():
    """The first fold needs two sessions behind it and one fold has nothing to
    agree with, so three sessions is one fold short of a walk."""
    with pytest.raises(ValueError, match="at least 4 sessions"):
        walk_forward(_in_order(LUMPY, BLENDED, THIN_TAIL), *ARGS)


def test_the_walk_names_no_pooled_figure_and_no_best_fold():
    """Two folds are not a sample. The walk must not be boiled down to one
    figure or one preferred step; the check is on the serialised report."""
    walk = walk_forward(_in_order(LUMPY, THIN_TAIL, PINCHED, BLENDED), *ARGS)
    text = json.dumps(walk)
    for word in ("best", "worst", "optimal", "argmax", "recommend", "mean", "average", "pooled"):
        assert word not in text, f"report mentions {word!r}: {text}"
