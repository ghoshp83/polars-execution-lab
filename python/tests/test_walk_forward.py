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


def _spans(walk: dict) -> list[tuple[int, int]]:
    return [(f["history_from"], f["input"]) for f in walk["folds"]]


def test_without_a_window_the_history_grows_from_the_first_session():
    """Leaving the window off must still be the walk it always was: the history
    grows from session 0, and the report says so instead of leaving it implied.
    A window of two starts on the same fold, since both pool sessions 0 and 1."""
    sessions = _in_order(LUMPY, BLENDED, PINCHED, THIN_TAIL)
    expanding = walk_forward(sessions, *ARGS)
    rolling = walk_forward(sessions, *ARGS, window=2)
    assert expanding["window"] is None
    assert rolling["window"] == 2
    assert _spans(expanding) == [(0, 2), (0, 3)]
    assert _spans(rolling) == [(0, 2), (1, 3)]
    assert expanding["folds"][0]["verdicts"] == rolling["folds"][0]["verdicts"]
    assert (
        expanding["folds"][0]["flat_improvement_bps"] == rolling["folds"][0]["flat_improvement_bps"]
    )


def test_a_windowed_fold_is_the_stability_grid_on_its_window_alone():
    """A windowed fold is a re-run too: the grid ``pov-stability`` prints for
    just the sessions inside the window. Here that changes the reading -- with
    the oldest session pooled in, the last fold is ``unstable`` at 0.3 and 0.5;
    from the two sessions before the input it is a ``gain`` at both."""
    sessions = _in_order(LUMPY, BLENDED, PINCHED, THIN_TAIL)
    rolling = walk_forward(sessions, *ARGS, window=2)
    grid = stability(sessions[1:3], sessions[3], *ARGS)
    last = rolling["folds"][-1]
    assert (last["input"], last["history_from"]) == (3, 1)
    assert last["flat_improvement_bps"] == [r["flat_improvement_bps"] for r in grid["rows"]]
    assert last["verdicts"] == ["inert", "gain", "gain"]
    expanding = walk_forward(sessions, *ARGS)
    assert expanding["folds"][-1]["verdicts"] == ["inert", "unstable", "unstable"]


def test_a_windowed_fold_is_untouched_by_sessions_older_than_its_window():
    """The property the window exists for: a fold cannot see a session older
    than its window. Replace the oldest session and the windowed last fold must
    not move by a digit, while the expanding one, which still pools it, does."""
    a = _in_order(LUMPY, BLENDED, PINCHED, THIN_TAIL)
    b = _in_order(THIN_TAIL, BLENDED, PINCHED, THIN_TAIL)

    def last(sessions: list[pl.DataFrame], window: int | None) -> list[float]:
        return walk_forward(sessions, *ARGS, window=window)["folds"][1]["flat_improvement_bps"]

    assert last(a, 2) == last(b, 2)
    assert last(a, None) != last(b, None)


def test_an_agreement_can_rest_on_the_oldest_session():
    """The expanding walk says pooling pays at 0.3 and 0.5 at every step here;
    held to two sessions of history, the last step reads both caps ``unstable``
    and only the inert cap agrees."""
    sessions = _in_order(LUMPY, THIN_TAIL, PINCHED, BLENDED)
    expanding = walk_forward(sessions, *ARGS)
    rolling = walk_forward(sessions, *ARGS, window=2)
    assert expanding["consensus"] == ["inert", "gain", "gain"]
    assert expanding["all_agree"] is True
    assert rolling["folds"][-1]["verdicts"] == ["inert", "unstable", "unstable"]
    assert rolling["consensus"] == ["inert", "mixed", "mixed"]
    assert rolling["agreeing_caps"] == 1
    assert rolling["all_agree"] is False


def test_three_walks_over_the_same_sessions_give_three_answers():
    """Why nothing here picks a window. The same five sessions agree on two
    caps with a growing history, on none with a window of two and on all three
    with a window of three -- and the widest window is also the one with a fold
    fewer, so the fullest agreement is the one with the least behind it."""
    sessions = _in_order(BLENDED, LUMPY, THIN_TAIL, PINCHED, BLENDED)
    expanding = walk_forward(sessions, *ARGS)
    two = walk_forward(sessions, *ARGS, window=2)
    three = walk_forward(sessions, *ARGS, window=3)
    assert expanding["agreeing_caps"] == 2
    assert two["agreeing_caps"] == 0
    assert three["agreeing_caps"] == 3
    assert len(expanding["folds"]) == 3
    assert _spans(two) == [(0, 2), (1, 3), (2, 4)]
    assert _spans(three) == [(0, 3), (1, 4)]


def test_a_window_too_narrow_or_too_wide_is_refused():
    """A window of one is the pool of one the walk already refuses, and a
    window that leaves a single fold leaves nothing for it to agree with."""
    four = _in_order(LUMPY, BLENDED, THIN_TAIL, PINCHED)
    for w in (0, 1):
        with pytest.raises(ValueError, match="window must be at least 2 sessions"):
            walk_forward(four, *ARGS, window=w)
    with pytest.raises(ValueError, match="window of 3 needs at least 5 sessions, got 4"):
        walk_forward(four, *ARGS, window=3)
    five = _in_order(LUMPY, BLENDED, THIN_TAIL, PINCHED, LUMPY)
    assert len(walk_forward(five, *ARGS, window=3)["folds"]) == 2


def test_a_windowed_walk_names_no_best_window():
    """The window is an input, never an output: the report repeats the one it
    was given and must not grow a field that prefers one."""
    sessions = _in_order(BLENDED, LUMPY, THIN_TAIL, PINCHED, BLENDED)
    walk = walk_forward(sessions, *ARGS, window=3)
    assert walk["window"] == 3
    text = json.dumps(walk)
    for word in ("best", "worst", "optimal", "argmax", "recommend", "mean", "average", "pooled"):
        assert word not in text, f"report mentions {word!r}: {text}"
