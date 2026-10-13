"""The six sessions under `data/real` are one unbroken ten-minute run of BTC-USD
trades from Coinbase's public feed, cut into consecutive 100-second slices.
Every other capture in `data/` was written by hand to make a point; these were
not, so what the engine reports on them is a measurement.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from xexeclab.engine import read_ticks, walk_forward, window_sweep

REAL = Path(__file__).parents[2] / "data" / "real"
# Ten seconds per bucket: the widest five-second gap in the run is empty, and a
# bucket with no trade has no price to execute at.
BUCKET = 10_000_000_000
BUCKETS = 10
CAPS = [0.05, 0.1, 0.15, 0.2, 0.25]
HALF_LIVES = [0.25, 0.5, 1.0, 2.0, 4.0]
ARGS = (BUCKET, 0.2, CAPS, HALF_LIVES, 25.0, 5.0)


@pytest.fixture(scope="module")
def sessions():
    return [read_ticks(REAL / f"btc_usd_{i}.ndjson") for i in range(1, 7)]


@pytest.fixture(scope="module")
def sweep(sessions):
    return window_sweep(sessions, *ARGS)


def test_the_six_sessions_are_one_unbroken_run_of_trades(sessions):
    # What makes the files a recording and not a fixture: the exchange numbers
    # its trades, and no number is missing from the first tick to the last. A
    # session trimmed or reordered to improve a result would show here.
    ids = [i for s in sessions for i in s["trade_id"].to_list()]
    ts = [t for s in sessions for t in s["ts_ns"].to_list()]
    assert len(ids) == 1916
    assert ids == list(range(ids[0], ids[0] + len(ids)))
    assert ts == sorted(ts)
    # The exchange stamps whole microseconds; a tick that is not one was
    # rounded through a float on the way to disk.
    assert all(t % 1_000 == 0 for t in ts)

    start = ts[0] // BUCKET
    for i, s in enumerate(sessions):
        first = start + i * BUCKETS
        assert sorted({t // BUCKET for t in s["ts_ns"].to_list()}) == list(
            range(first, first + BUCKETS)
        )


def test_one_binding_cap_gains_at_every_real_step(sessions, sweep):
    # On the written captures the only cap every walk settles is the inert one.
    # Here a cap that binds gains at every step of every walk, and the other
    # four change sign from one session to the next.
    walk = walk_forward(sessions, *ARGS)
    assert walk["consensus"] == ["mixed", "mixed", "mixed", "mixed", "gain"]
    for fold in walk["folds"]:
        assert fold["verdicts"][4] == "gain"
        assert fold["flat_improvement_bps"][4] > 0.0
    assert walk["folds"][2]["verdicts"][:2] == ["inert", "inert"]
    assert walk["folds"][3]["verdicts"][:2] == ["loss", "loss"]

    assert sweep["settled_caps"] == 1
    assert not sweep["all_settled"]
    assert all(w["consensus"][4] == "gain" for w in sweep["walks"])


def test_the_newest_real_session_holds_where_an_earlier_one_split(sweep):
    # v0.40.0 could only show `every_input_stable` on sessions written for the
    # purpose. On recorded ones the newest session reads the same at every
    # window and every cap, and an earlier session does not.
    assert (sweep["folds_run"], sweep["distinct_folds"], sweep["compared_inputs"]) == (13, 10, 3)
    assert sweep["last_fold_stable"] == [True] * 5
    assert sweep["every_input_stable"] == [True, True, True, False, True]
    assert sweep["newest_only_caps"] == 1
    assert not sweep["inputs"][1]["verdict_stable"][3]


def test_the_widest_real_span_at_one_cap_is_on_an_earlier_session(sweep):
    # Reading the last row alone understates that cap by a fifth.
    assert sweep["widest_span_input"] == [5, 5, 5, 4, 5]
    assert sweep["last_fold_span_bps"] == [
        0.21014988,
        0.39667092,
        0.41470266,
        0.42218023,
        0.76405639,
    ]
    assert sweep["widest_span_bps"] == [
        0.21014988,
        0.39667092,
        0.41470266,
        0.51403291,
        0.76405639,
    ]
