"""The held-out rotation -- mirrors `tests/holdout.rs` case for case."""

from __future__ import annotations

import json

import polars as pl
import pytest

from xexeclab.engine import holdout, stability

BUCKET = 1_000_000_000
# An hour between sessions, so captures only line up by time into the session.
HOUR = 3_600 * BUCKET
HALF_LIVES = [0.25, 0.5, 1.0, 2.0, 4.0]
CAPS = [0.01, 0.3, 0.5]


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


def test_the_last_fold_is_the_stability_grid():
    """The rotation is a re-run, not a new calculation: the fold that holds out
    the last session must be the grid ``pov-stability`` prints for the same
    history and input, or the two commands would disagree about one question."""
    sessions = [_blended(), _lumpy(), _pinched()]
    report = holdout(sessions, BUCKET, 1.0, CAPS, HALF_LIVES, 25.0, 5.0)
    grid = stability(sessions[:2], sessions[2], BUCKET, 1.0, CAPS, HALF_LIVES, 25.0, 5.0)
    last = report["folds"][-1]
    assert last["held_out"] == 2
    assert last["flat_improvement_bps"] == [r["flat_improvement_bps"] for r in grid["rows"]]
    assert last["inert_caps"] == grid["inert_caps"]
    assert last["stable_caps"] == grid["stable_caps"]
    assert last["unstable_caps"] == grid["unstable_caps"]
    # This grid is the one that is unstable at 0.3, and the fold must say so.
    assert last["verdicts"] == ["inert", "unstable", "gain"]
    assert report["caps"] == [r["cap"] for r in grid["rows"]]
    assert report["half_lives"] == grid["half_lives"]
    assert report["sessions"] == 3


def test_every_session_is_held_out_exactly_once():
    """Every session takes exactly one turn, in the order given, and each fold
    pools the rest -- so the folds' counts each cover the whole cap grid."""
    sessions = [_blended(), _lumpy(), _thin_tail(), _pinched()]
    report = holdout(sessions, BUCKET, 0.6, CAPS, HALF_LIVES, 25.0, 5.0)
    assert report["sessions"] == 4
    assert [f["held_out"] for f in report["folds"]] == [0, 1, 2, 3]
    for f in report["folds"]:
        assert len(f["verdicts"]) == len(CAPS)
        assert len(f["flat_improvement_bps"]) == len(CAPS)
        assert f["inert_caps"] + f["stable_caps"] + f["unstable_caps"] == len(CAPS)


def test_a_verdict_every_fold_shares_is_the_consensus():
    """When every fold reads a cap the same way the consensus is that reading --
    including ``inert``, which is agreement that nothing was measured and must
    stay visible as such instead of hiding behind a bare "agrees"."""
    report = holdout([_blended(), _lumpy(), _thin_tail()], BUCKET, 1.0, CAPS, HALF_LIVES, 25.0, 5.0)
    for f in report["folds"]:
        assert f["verdicts"] == ["inert", "gain", "gain"]
    assert report["consensus"] == ["inert", "gain", "gain"]
    assert report["agreeing_caps"] == 3
    assert report["all_agree"] is True


def test_folds_that_are_each_stable_can_still_disagree_on_the_sign():
    """The reason a fold carries a direction and not just ``sign_stable``: here
    no fold has an unstable cap, so every fold's own grid calls 0.3 and 0.5
    stable -- yet two folds say pooling costs and one says it pays. Agreement on
    stability is not agreement on the answer."""
    report = holdout([_lumpy(), _pinched(), _blended()], BUCKET, 0.6, CAPS, HALF_LIVES, 25.0, 5.0)
    for f in report["folds"]:
        assert f["unstable_caps"] == 0
        assert f["stable_caps"] == 2
    assert [f["verdicts"] for f in report["folds"]] == [
        ["inert", "loss", "loss"],
        ["inert", "loss", "loss"],
        ["inert", "gain", "gain"],
    ]
    assert report["consensus"] == ["inert", "mixed", "mixed"]
    assert report["agreeing_caps"] == 1
    assert report["all_agree"] is False
    # The directions are read off figures the fold itself reports.
    assert report["folds"][0]["flat_improvement_bps"][1] < 0.0
    assert report["folds"][2]["flat_improvement_bps"][1] > 0.0


def test_the_order_the_sessions_are_given_in_matters():
    """The history order is part of the question -- the last history session is
    the naive forecast and the half-life weights by recency -- so a rotation of
    the same sessions in a different order is a different report."""
    a = holdout([_blended(), _lumpy(), _pinched()], BUCKET, 1.0, CAPS, HALF_LIVES, 25.0, 5.0)
    b = holdout([_lumpy(), _pinched(), _blended()], BUCKET, 1.0, CAPS, HALF_LIVES, 25.0, 5.0)
    assert a["consensus"] == ["inert", "mixed", "gain"]
    assert b["consensus"] == ["inert", "mixed", "mixed"]


def test_fewer_than_three_sessions_is_refused():
    """Each fold pools the other sessions; with two sessions that pool is a
    single capture and there is nothing to rotate."""
    with pytest.raises(ValueError, match="at least 3 sessions"):
        holdout([_blended(), _lumpy()], BUCKET, 1.0, CAPS, HALF_LIVES, 25.0, 5.0)


def test_the_rotation_names_no_pooled_figure_and_no_best_fold():
    """A rotation over a handful of sessions must not be boiled down to one
    figure or one preferred fold: a mean would read as an estimate and a best
    fold as a recommendation. The check is on the serialised report."""
    report = holdout([_blended(), _lumpy(), _thin_tail()], BUCKET, 1.0, CAPS, HALF_LIVES, 25.0, 5.0)
    text = json.dumps(report)
    for word in ("best", "worst", "optimal", "argmax", "recommend", "mean", "average", "pooled"):
        assert word not in text, f"report mentions {word!r}: {text}"
