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


# Day two: the same feed, collector and cut, thirty-one hours later.
@pytest.fixture(scope="module")
def day_two():
    return [read_ticks(REAL / f"btc_usd_d2_{i}.ndjson") for i in range(1, 7)]


def test_day_two_is_its_own_unbroken_run_a_day_and_more_later(sessions, day_two):
    # Six more whole sessions, contiguous among themselves; the gap between the
    # two runs is the point of having them.
    ids = [i for s in day_two for i in s["trade_id"].to_list()]
    ts = [t for s in day_two for t in s["ts_ns"].to_list()]
    assert len(ids) == 5248
    assert ids == list(range(ids[0], ids[0] + len(ids)))
    assert ts == sorted(ts) and all(t % 1_000 == 0 for t in ts)
    assert ts[0] - sessions[-1]["ts_ns"].max() > 24 * 3600 * 10**9
    start = ts[0] // BUCKET
    for i, s in enumerate(day_two):
        first = start + i * BUCKETS
        assert sorted({t // BUCKET for t in s["ts_ns"].to_list()}) == list(
            range(first, first + BUCKETS)
        )
    # Over six times the volume of day one: 89 BTC against 14.
    assert 85 < sum(s["size"].sum() for s in day_two) < 95


def test_the_cap_day_one_settled_does_not_settle_on_day_two(day_two):
    # Day one's one settled cap was the first result in this project that was
    # not inert. A second day is the first chance to see whether it repeats,
    # and it does not: the 25% cap is unstable on two of day two's four folds,
    # and no cap settles. Every session reads the same at every window, so the
    # disagreement is between days, not between windows.
    walk = walk_forward(day_two, *ARGS)
    assert [f["verdicts"][4] for f in walk["folds"]] == ["unstable", "gain", "gain", "unstable"]
    sweep = window_sweep(day_two, *ARGS)
    assert sweep["settled_caps"] == 0
    assert all(w["consensus"] == ["mixed"] * 5 for w in sweep["walks"])
    assert sweep["every_input_stable"] == [True] * 5
    assert sweep["newest_only_caps"] == 0


def test_a_parent_small_against_the_day_stops_telling_the_caps_apart(day_two):
    # 0.2 BTC is up to a quarter of a day-one session (0.83 to 5.73 BTC) and
    # under 2.5% of every day-two session (8.2 to 24.2), so on the later folds
    # no cap binds and all five are one plan. The grid is only as wide as the
    # parent is large.
    walk = walk_forward(day_two, *ARGS)
    for fold in walk["folds"][2:]:
        assert len(set(fold["flat_improvement_bps"])) == 1
    assert len(set(walk["folds"][0]["flat_improvement_bps"])) == 5


def test_across_two_days_no_cap_settles(sessions, day_two):
    # Walked as one history the 25% cap gains on nine folds out of ten, and the
    # tenth -- the newest session -- is unstable at every cap. Scored from
    # exactly six sessions, so that day two's first fold sees only day one,
    # it gains on five of six. Neither is a settled cap.
    both = sessions + day_two
    walk = walk_forward(both, *ARGS)
    assert walk["consensus"] == ["mixed"] * 5
    assert [f["verdicts"][4] for f in walk["folds"]] == ["gain"] * 9 + ["unstable"]
    assert walk["folds"][-1]["verdicts"] == ["unstable"] * 5
    window = walk_forward(both, *ARGS, window=6)
    assert [f["verdicts"][4] for f in window["folds"]] == ["gain"] * 5 + ["unstable"]
    assert window["consensus"] == ["mixed"] * 5


def test_recorded_quotes_are_the_book_at_each_trade_of_their_session():
    # Coinbase's ticker fires once per trade with that trade's time, so the
    # quote file and the first day-two session are the same 776 instants: a
    # quote here is the book a trade left behind, not a sample on a clock.
    # It never crosses, and half the time it is one cent wide.
    from xexeclab.engine import quote_metrics, read_quotes

    quotes = read_quotes(REAL / "btc_usd_d2_1_quotes.ndjson")
    trades = read_ticks(REAL / "btc_usd_d2_1.ndjson")
    assert quotes["ts_ns"].to_list() == trades["ts_ns"].to_list()
    spread = (quotes["ask"] - quotes["bid"]).round(2)
    assert spread.min() == 0.01 and spread.max() == 18.0
    assert (spread == 0.01).sum() == 371
    metrics = quote_metrics(quotes, "BTC-USD")
    assert metrics["quotes"] == 776
    assert metrics["avg_spread"] == 2.21742268


def test_sized_to_its_own_history_day_one_settles_no_cap(sessions, sweep):
    """Day one's one settled cap was the fixed parent's doing. Its sessions
    traded from 0.83 to 5.73 BTC, so 0.2 BTC was 3.5% of one and 24% of
    another. A share of 0.085 is the same 0.2 against day one's mean session,
    taken from each fold's history alone, and with it nothing settles."""
    sized = window_sweep(sessions, BUCKET, None, CAPS, HALF_LIVES, 25.0, 5.0, parent_share=0.085)
    assert sweep["settled_caps"] == 1
    assert sized["settled_caps"] == 0
    assert sized["walks"][0]["consensus"] == ["mixed"] * 5
    grown = walk_forward(sessions, BUCKET, None, CAPS, HALF_LIVES, 25.0, 5.0, parent_share=0.085)
    parents = [f["parent_qty"] for f in grown["folds"]]
    assert parents == [0.35185609, 0.25819891, 0.21457672, 0.19137286]
    assert [f["verdicts"][4] for f in grown["folds"]] == ["inert", "unstable", "gain", "gain"]


def test_sized_to_the_day_the_parent_grows_with_it_and_still_settles_nothing(day_two):
    """On day two the same share is six to seven times day one's parent, so
    the caps bind again where a fixed 0.2 BTC let them all give one plan. They
    still disagree along the walk, and the 25% cap now loses on a fold."""
    walk = walk_forward(day_two, BUCKET, None, CAPS, HALF_LIVES, 25.0, 5.0, parent_share=0.085)
    parents = [f["parent_qty"] for f in walk["folds"]]
    assert parents == [1.37807173, 1.41895757, 1.25205563, 1.3017328]
    assert [f["verdicts"][4] for f in walk["folds"]] == ["gain", "loss", "gain", "unstable"]
    assert walk["consensus"] == ["mixed"] * 5
