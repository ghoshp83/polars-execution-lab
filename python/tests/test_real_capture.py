"""The six sessions under `data/real` are one unbroken ten-minute run of BTC-USD
trades from Coinbase's public feed, cut into consecutive 100-second slices.
Every other capture in `data/` was written by hand to make a point; these were
not, so what the engine reports on them is a measurement.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from xexeclab.engine import (
    book_grid_across,
    book_grid_phase,
    book_grid_phase_across,
    book_grid_sweep,
    read_book,
    read_ticks,
    share_sweep,
    walk_forward,
    window_sweep,
)

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


def test_across_shares_day_one_settles_nothing_and_the_newest_session_flips(sessions):
    """Sizing by share moved the question from the size to the share, and the
    share is a number nobody fits either. Swept from 5% to 60% of each fold's
    history, day one settles no cap at any share, and the newest session's
    verdict changes with the share at every cap: at 25% it gains at the two
    smallest shares and loses at the two largest."""
    report = share_sweep(
        sessions, BUCKET, [0.05, 0.085, 0.15, 0.3, 0.6], CAPS, HALF_LIVES, 25.0, 5.0
    )
    assert report["settled_at"] == [0] * 5
    assert report["held_caps"] == 0
    assert report["last_share_stable"] == [False] * 5
    at_25 = [r["last_verdicts"][4] for r in report["shares"]]
    assert at_25 == ["gain", "gain", "unstable", "loss", "loss"]


@pytest.fixture(scope="module")
def day_three():
    return [read_ticks(REAL / f"btc_usd_d3_{i}.ndjson") for i in range(1, 7)]


def test_day_three_is_its_own_unbroken_run_and_the_quietest(day_two, day_three):
    # A third unbroken run, two and a half days after the second, and the
    # quietest of the three: under 3 BTC against 14 and 89, with a last session
    # that traded less than a tenth of one.
    ids = [i for s in day_three for i in s["trade_id"].to_list()]
    ts = [t for s in day_three for t in s["ts_ns"].to_list()]
    assert len(ids) == 1986
    assert ids == list(range(ids[0], ids[0] + len(ids)))
    assert ts == sorted(ts)
    assert ts[0] - max(s["ts_ns"].max() for s in day_two) > 48 * 3600 * 10**9
    assert 2.5 < sum(s["size"].sum() for s in day_three) < 3.0
    assert day_three[5]["size"].sum() < 0.1


def test_on_the_quietest_day_what_holds_across_shares_is_only_inert(day_three):
    """What holds across every share is the newest session's reading at four
    caps, and it holds because that session is inert there: a 0.067 BTC session
    gives every half-life the same figure. `last_share_stable` is a verdict that
    did not move, not one that says gain or loss."""
    report = share_sweep(
        day_three, BUCKET, [0.05, 0.085, 0.15, 0.3, 0.6], CAPS, HALF_LIVES, 25.0, 5.0
    )
    assert report["settled_at"] == [0] * 5
    assert report["held_caps"] == 0
    assert report["last_share_stable"] == [True, True, True, True, False]
    assert all(r["last_verdicts"][:4] == ["inert"] * 4 for r in report["shares"])
    at_25 = [r["last_verdicts"][4] for r in report["shares"]]
    assert at_25 == ["unstable", "inert", "inert", "inert", "inert"]


def test_the_recorded_book_covers_its_session_and_runs_out_of_levels(day_three):
    """The first recorded book: the top ten levels of each side after every
    `level2_batch` update, over the same 100 seconds as the first day-three
    session. Ten levels of BTC-USD hold well under one bitcoin, so the README's
    0.2 BTC parent swept at once does not always fit, and two never does."""
    from xexeclab.engine import depth_metrics, read_book, sweep_cost, sweep_curve

    book = read_book(REAL / "btc_usd_d3_1_book.ndjson")
    session = day_three[0]
    assert book["ts_ns"].min() // BUCKET == session["ts_ns"].min() // BUCKET
    assert book["ts_ns"].max() // BUCKET == session["ts_ns"].max() // BUCKET

    depth = depth_metrics(book, "BTC-USD")
    assert depth["snapshots"] == 1431
    assert depth["avg_ask_depth"] == 0.66147753
    assert depth["avg_bid_depth"] == 0.73534154
    assert depth["avg_spread"] < 0.015

    parent = sweep_cost(book, "BTC-USD", "buy", 0.2)
    assert parent["filled_snapshots"] == 1279
    assert sweep_cost(book, "BTC-USD", "buy", 2.0)["filled_snapshots"] == 0
    # The bid holds more size and still costs more to hit: it sits further out.
    sell = sweep_cost(book, "BTC-USD", "sell", 0.2)
    assert sell["avg_slippage_bps"] > 7.0 * parent["avg_slippage_bps"]

    curve = sweep_curve(book, "BTC-USD", "buy", [0.01, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0])
    assert curve["fitted_points"] == 3
    assert curve["coef_bps"] == 0.0848413


@pytest.fixture(scope="module")
def day_four():
    return [read_ticks(REAL / f"btc_usd_d4_{i}.ndjson") for i in range(1, 7)]


def test_day_four_is_its_own_unbroken_run_the_morning_after(day_three, day_four):
    # A fourth unbroken run, the morning after the third: 938 trades and about
    # four and a half BTC, with no session so quiet that it goes inert.
    ids = [i for s in day_four for i in s["trade_id"].to_list()]
    ts = [t for s in day_four for t in s["ts_ns"].to_list()]
    assert len(ids) == 938
    assert ids == list(range(ids[0], ids[0] + len(ids)))
    assert ts == sorted(ts)
    assert ts[0] - max(s["ts_ns"].max() for s in day_three) > 20 * 3600 * 10**9
    assert 4.5 < sum(s["size"].sum() for s in day_four) < 4.6
    assert day_four[5]["size"].sum() > 1.0


def test_when_the_newest_session_trades_nothing_holds_across_shares(day_four):
    """Day three's `last_share_stable` held only where its newest session was
    inert. Day four's newest session traded over a bitcoin, and the reading
    moves with the share at every cap: nothing holds once nothing is inert."""
    report = share_sweep(
        day_four, BUCKET, [0.05, 0.085, 0.15, 0.3, 0.6], CAPS, HALF_LIVES, 25.0, 5.0
    )
    assert report["settled_at"] == [0] * 5
    assert report["held_caps"] == 0
    assert report["last_share_stable"] == [False] * 5
    assert all("inert" not in r["last_verdicts"] for r in report["shares"])
    at_25 = [r["last_verdicts"][4] for r in report["shares"]]
    assert at_25 == ["loss", "unstable", "unstable", "gain", "unstable"]


def test_fifty_levels_fill_what_ten_levels_of_the_same_book_could_not(day_four):
    """The deep book: fifty levels a side, one snapshot every five seconds, over
    all six day-four sessions. Every size to 5 BTC fills in every snapshot. Cut
    to its own top ten levels, the same snapshots fill 2 BTC in 32 of 119 and
    5 BTC in none, and the curve fits three points to a coefficient under a
    third of the deep one: a shallow recording sets a floor, not a measurement."""
    import polars as pl

    from xexeclab.engine import depth_metrics, read_book, sweep_cost, sweep_curve

    book = read_book(REAL / "btc_usd_d4_book.ndjson")
    start = day_four[0]["ts_ns"].min() // BUCKET
    assert book["ts_ns"].min() // BUCKET == start
    assert book["ts_ns"].max() < (start + 60) * BUCKET
    assert book["level"].max() == 49

    depth = depth_metrics(book, "BTC-USD")
    assert depth["snapshots"] == 119
    assert depth["avg_ask_depth"] == 9.67633378
    for side in ("buy", "sell"):
        assert sweep_cost(book, "BTC-USD", side, 5.0)["filled_snapshots"] == 119
    top = book.filter(pl.col("level") < 10)
    assert sweep_cost(top, "BTC-USD", "buy", 2.0)["filled_snapshots"] == 32
    assert sweep_cost(top, "BTC-USD", "buy", 5.0)["filled_snapshots"] == 0

    sizes = [0.01, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0]
    deep = sweep_curve(book, "BTC-USD", "buy", sizes)
    assert deep["fitted_points"] == 8
    assert deep["coef_bps"] == 1.74400041
    shallow = sweep_curve(top, "BTC-USD", "buy", sizes)
    assert shallow["fitted_points"] == 3
    assert shallow["coef_bps"] == 0.48342117


@pytest.fixture(scope="module")
def day_five():
    return [read_ticks(REAL / f"btc_usd_d5_{i}.ndjson") for i in range(1, 7)]


def test_day_five_is_its_own_unbroken_run_on_a_weekday(day_four, day_five):
    # A fifth unbroken run, the weekday after the weekend: 2,425 trades and five
    # times day four's volume, the busiest run since day two.
    ids = [i for s in day_five for i in s["trade_id"].to_list()]
    ts = [t for s in day_five for t in s["ts_ns"].to_list()]
    assert len(ids) == 2425
    assert ids == list(range(ids[0], ids[0] + len(ids)))
    assert ts == sorted(ts)
    assert ts[0] - max(s["ts_ns"].max() for s in day_four) > 20 * 3600 * 10**9
    assert 22.7 < sum(s["size"].sum() for s in day_five) < 22.8


def test_on_day_five_only_the_inert_cap_holds_across_shares(day_five):
    """Day five's newest session is inert at the tightest cap at every share, so
    that cap reads the same at every share's newest step; the other four move
    with the share. One share settles the inert cap, and none settles the rest."""
    report = share_sweep(
        day_five, BUCKET, [0.05, 0.085, 0.15, 0.3, 0.6], CAPS, HALF_LIVES, 25.0, 5.0
    )
    assert report["held_caps"] == 0
    assert report["settled_at"] == [1, 0, 0, 0, 0]
    assert report["last_share_stable"] == [True, False, False, False, False]
    assert all(r["last_verdicts"][0] == "inert" for r in report["shares"])


def test_a_five_second_grid_keeps_the_curve_and_loses_the_top_of_book():
    """Thirty seconds of the fifty-level book at the full update rate, with
    copies of the previous snapshot left out, beside the five-second grid
    `--every 5` would have kept from it. The grid keeps the depth and the curve:
    the coefficient moves under 4%. It loses the top of book, which moves faster
    than five seconds: the spread averages a quarter of the full-rate figure,
    and the queue imbalance changes sign."""
    import polars as pl

    from xexeclab.engine import (
        depth_metrics,
        queue_metrics,
        read_book,
        sweep_cost,
        sweep_curve,
    )

    book = read_book(REAL / "btc_usd_d5_book_full.ndjson")
    snapshots = [
        g.select("side", "level", "price", "size").rows()
        for _, g in book.group_by("ts_ns", maintain_order=True)
    ]
    assert len(snapshots) == 330
    assert all(a != b for a, b in zip(snapshots, snapshots[1:], strict=False))

    kept: list[int] = []
    for ts in book["ts_ns"].unique().sort().to_list():
        if not kept or ts - kept[-1] >= 5 * 10**9:
            kept.append(ts)
    grid = book.filter(pl.col("ts_ns").is_in(kept))
    assert len(kept) == 6

    sizes = [0.01, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0]
    full = sweep_curve(book, "BTC-USD", "buy", sizes)
    thin = sweep_curve(grid, "BTC-USD", "buy", sizes)
    assert (full["fitted_points"], thin["fitted_points"]) == (7, 7)
    assert full["coef_bps"] == 1.9721695
    assert thin["coef_bps"] == 2.03941096
    assert abs(thin["coef_bps"] / full["coef_bps"] - 1.0) < 0.04

    full_depth = depth_metrics(book, "BTC-USD")
    assert full_depth["avg_spread"] == 0.0400303
    assert full_depth["avg_spread"] > 4 * depth_metrics(grid, "BTC-USD")["avg_spread"]
    assert queue_metrics(book, "BTC-USD")["avg_queue_imbalance"] == 0.17003025
    assert queue_metrics(grid, "BTC-USD")["avg_queue_imbalance"] < 0

    # Monday's ask is thinner than Sunday's: 5 BTC lifts all fifty levels and
    # does not fill, while the deeper bid takes it in every snapshot.
    assert sweep_cost(book, "BTC-USD", "buy", 5.0)["filled_snapshots"] == 0
    assert sweep_cost(book, "BTC-USD", "sell", 5.0)["filled_snapshots"] == 330


@pytest.fixture(scope="module")
def day_six():
    return [read_ticks(REAL / f"btc_usd_d6_{i}.ndjson") for i in range(1, 7)]


def test_day_six_is_its_own_unbroken_run(day_five, day_six):
    # A sixth unbroken run, the Tuesday after day five: 1,738 trades and under
    # sixteen BTC, quieter than Monday.
    ids = [i for s in day_six for i in s["trade_id"].to_list()]
    ts = [t for s in day_six for t in s["ts_ns"].to_list()]
    assert len(ids) == 1738
    assert ids == list(range(ids[0], ids[0] + len(ids)))
    assert ts == sorted(ts)
    assert ts[0] - max(s["ts_ns"].max() for s in day_five) > 20 * 3600 * 10**9
    assert 15.97 < sum(s["size"].sum() for s in day_six) < 15.98


GRID = [0.25, 0.5, 1.0, 2.0, 5.0, 10.0]


@pytest.mark.parametrize("day", ["d5", "d6"])
def test_no_grid_spacing_keeps_the_touch_on_both_full_rate_books(day):
    """No spacing keeps the touch on both days: the spread a grid sees runs more
    than twice the book's at one spacing and under half of it at another, and it
    does not settle as the grid tightens. The depth imbalance stays within 0.035
    of full rate to five seconds on both days."""
    sweep = book_grid_sweep(read_book(REAL / f"btc_usd_{day}_book_full.ndjson"), "BTC-USD", GRID)
    ratios = [r["spread_ratio"] for r in sweep["rows"]]
    assert any(r > 2.0 for r in ratios), ratios
    assert any(r < 0.5 for r in ratios), ratios
    for r in sweep["rows"]:
        if r["every_s"] <= 5.0:
            drift = r["avg_depth_imbalance"] - sweep["full"]["avg_depth_imbalance"]
            assert abs(drift) < 0.035, (r["every_s"], drift)


def test_the_queue_keeps_its_side_on_one_day_and_loses_it_on_the_other():
    """Monday's bid-heavy queue (+0.170) reads ask-heavy from five seconds out;
    Tuesday's ask-heavy queue (-0.297) keeps its side at every spacing, but ten
    seconds reads it more than twice as heavy."""
    monday = book_grid_sweep(read_book(REAL / "btc_usd_d5_book_full.ndjson"), "BTC-USD", GRID)
    assert [r["snapshots"] for r in monday["rows"]] == [95, 53, 28, 15, 6, 3]
    assert [r["queue_sign_kept"] for r in monday["rows"]] == [True] * 4 + [False] * 2
    assert monday["sign_flips"] == 2
    # The five-second row is v0.47.0's hand-thinned grid, now the engine's.
    assert monday["rows"][4]["spread_ratio"] == pytest.approx(0.24981077, abs=1e-8)

    tuesday = book_grid_sweep(read_book(REAL / "btc_usd_d6_book_full.ndjson"), "BTC-USD", GRID)
    assert tuesday["full"]["snapshots"] == 440
    assert tuesday["sign_flips"] == 0
    assert tuesday["full"]["avg_queue_imbalance"] == pytest.approx(-0.29748286, abs=1e-8)
    assert tuesday["rows"][5]["avg_queue_imbalance"] < 2.0 * tuesday["full"]["avg_queue_imbalance"]
    # Two seconds and wider only ever land on a one-cent touch.
    for r in tuesday["rows"][3:]:
        assert r["avg_spread"] == pytest.approx(0.01, abs=1e-8)


@pytest.fixture(scope="module")
def day_seven():
    return [read_ticks(REAL / f"btc_usd_d7_{i}.ndjson") for i in range(1, 7)]


def test_day_seven_is_its_own_unbroken_run(day_six, day_seven):
    # A seventh unbroken run, the Wednesday after day six: 2,778 trades and over
    # forty-four BTC, most of it in the first three sessions.
    ids = [i for s in day_seven for i in s["trade_id"].to_list()]
    ts = [t for s in day_seven for t in s["ts_ns"].to_list()]
    assert len(ids) == 2778
    assert ids == list(range(ids[0], ids[0] + len(ids)))
    assert ts == sorted(ts)
    assert ts[0] - max(s["ts_ns"].max() for s in day_six) > 20 * 3600 * 10**9
    assert 44.62 < sum(s["size"].sum() for s in day_seven) < 44.63


def _full_rate_books(days):
    return [read_book(REAL / f"btc_usd_{d}_book_full.ndjson") for d in days]


def test_a_near_level_queue_flips_at_a_quarter_second():
    """Wednesday's queue sits almost level (+0.017), so a grid flips it from a
    quarter second out -- and its depth imbalance drifts past 0.035 at five
    seconds, which neither earlier book did."""
    wednesday = book_grid_sweep(read_book(REAL / "btc_usd_d7_book_full.ndjson"), "BTC-USD", GRID)
    assert wednesday["full"]["snapshots"] == 490
    assert wednesday["full"]["avg_queue_imbalance"] == pytest.approx(0.01660519, abs=1e-8)
    sides = [r["queue_sign_kept"] for r in wednesday["rows"]]
    assert sides == [False, True, False, False, True, True]
    assert wednesday["sign_flips"] == 3
    drift = wednesday["rows"][4]["avg_depth_imbalance"] - wednesday["full"]["avg_depth_imbalance"]
    assert abs(drift) > 0.035, drift


def test_a_third_book_leaves_one_spacing_held_of_the_four_two_books_held():
    """Two books held four spacings out of six; the third holds one. A spacing
    that keeps the queue's side on every book recorded so far is not shown to
    keep it on the next."""
    two = book_grid_across(_full_rate_books(["d5", "d6"]), "BTC-USD", GRID)
    assert [s["sign_kept_books"] for s in two["spacings"]] == [2, 2, 2, 2, 1, 1]
    assert two["held_spacings"] == 4

    three = book_grid_across(_full_rate_books(["d5", "d6", "d7"]), "BTC-USD", GRID)
    assert [s["sign_kept_books"] for s in three["spacings"]] == [2, 3, 2, 2, 2, 2]
    assert three["held_spacings"] == 1
    # Even the one held spacing does not keep the touch: its spread runs high
    # on every book.
    assert three["spacings"][1]["spread_ratio_min"] > 1.0


@pytest.fixture(scope="module")
def day_eight():
    return [read_ticks(REAL / f"btc_usd_d8_{i}.ndjson") for i in range(1, 7)]


def test_day_eight_is_its_own_unbroken_run(day_seven, day_eight):
    # An eighth unbroken run, the Thursday after day seven: 2,987 trades and
    # over thirty-three BTC, most of it in the third and fourth sessions.
    ids = [i for s in day_eight for i in s["trade_id"].to_list()]
    ts = [t for s in day_eight for t in s["ts_ns"].to_list()]
    assert len(ids) == 2987
    assert ids == list(range(ids[0], ids[0] + len(ids)))
    assert ts == sorted(ts)
    last_day_seven = max(t for s in day_seven for t in s["ts_ns"].to_list())
    assert ts[0] - last_day_seven > 20 * 3600 * 1_000_000_000
    volume = 0.0
    for s in day_eight:
        for size in s["size"].to_list():
            volume += size
    assert 33.19 < volume < 33.20


def test_a_fourth_book_takes_nothing_away_and_one_book_takes_three():
    """Thursday's queue rests firmly on the bid (+0.114) and keeps its side to
    five seconds, so a fourth book takes no held spacing away. Leaving each
    book out in turn shows where the losses came from: without Wednesday four
    spacings hold, without any other book one or two."""
    thursday = book_grid_sweep(read_book(REAL / "btc_usd_d8_book_full.ndjson"), "BTC-USD", GRID)
    assert thursday["full"]["snapshots"] == 470
    assert [r["queue_sign_kept"] for r in thursday["rows"]] == [True, True, True, True, True, False]
    assert thursday["sign_flips"] == 1
    # Thursday's depth imbalance drifts past 0.035 at five seconds too.
    drift = thursday["rows"][4]["avg_depth_imbalance"] - thursday["full"]["avg_depth_imbalance"]
    assert abs(drift) > 0.035, drift

    three = book_grid_across(_full_rate_books(["d5", "d6", "d7"]), "BTC-USD", GRID)
    assert three["held_without"] == [3, 1, 4]
    four = book_grid_across(_full_rate_books(["d5", "d6", "d7", "d8"]), "BTC-USD", GRID)
    assert [s["sign_kept_books"] for s in four["spacings"]] == [3, 4, 3, 3, 3, 2]
    assert four["held_spacings"] == three["held_spacings"]
    assert four["held_without"] == [2, 1, 4, 1]


@pytest.fixture(scope="module")
def day_nine():
    return [read_ticks(REAL / f"btc_usd_d9_{i}.ndjson") for i in range(1, 7)]


def test_day_nine_is_its_own_unbroken_run(day_eight, day_nine):
    # A ninth unbroken run, a week after day eight: 2,355 trades and over thirty
    # BTC, more than half of it in the last session alone.
    ids = [i for s in day_nine for i in s["trade_id"].to_list()]
    ts = [t for s in day_nine for t in s["ts_ns"].to_list()]
    assert len(ids) == 2355
    assert ids == list(range(ids[0], ids[0] + len(ids)))
    assert ts == sorted(ts)
    last_day_eight = max(t for s in day_eight for t in s["ts_ns"].to_list())
    assert ts[0] - last_day_eight > 6 * 24 * 3600 * 1_000_000_000
    volume = 0.0
    for s in day_nine:
        for size in s["size"].to_list():
            volume += size
    assert 30.73 < volume < 30.74
    last = 0.0
    for size in day_nine[5]["size"].to_list():
        last += size
    assert last > volume / 2


def test_a_book_that_never_flips_still_holds_one_spacing_and_the_losers_are_named():
    """The second Thursday's queue rests deep on the ask (-0.536) and is the
    first book to keep its side at every spacing, so a fifth book takes nothing
    away: one spacing still holds. ``sign_lost_by`` names who breaks the rest --
    the Wednesday book alone at 0.25, 1 and 2 s, the Monday book alone at 5 s,
    and two books at 10 s, which is why no single absence frees that one."""
    book = read_book(REAL / "btc_usd_d9_book_full.ndjson")
    second_thursday = book_grid_sweep(book, "BTC-USD", GRID)
    assert second_thursday["full"]["snapshots"] == 393
    assert second_thursday["full"]["avg_queue_imbalance"] < -0.5
    assert all(r["queue_sign_kept"] for r in second_thursday["rows"])
    assert second_thursday["sign_flips"] == 0
    # A one-tick spread throughout, so every grid keeps the touch here.
    assert all(r["spread_ratio"] == 1.0 for r in second_thursday["rows"])

    five = book_grid_across(_full_rate_books(["d5", "d6", "d7", "d8", "d9"]), "BTC-USD", GRID)
    assert [s["sign_lost_by"] for s in five["spacings"]] == [[2], [], [2], [2], [0], [0, 3]]
    assert five["held_spacings"] == 1
    assert five["held_without"] == [2, 1, 4, 1, 1]


@pytest.fixture(scope="module")
def day_ten():
    return [read_ticks(REAL / f"btc_usd_d10_{i}.ndjson") for i in range(1, 7)]


def test_day_ten_is_its_own_unbroken_run(day_nine, day_ten):
    # A tenth unbroken run, the morning after day nine: 3,042 trades and the same
    # thirty BTC, but spread out -- no session holds a third of it, where day
    # nine's last session held more than half.
    ids = [i for s in day_ten for i in s["trade_id"].to_list()]
    ts = [t for s in day_ten for t in s["ts_ns"].to_list()]
    assert len(ids) == 3042
    assert ids == list(range(ids[0], ids[0] + len(ids)))
    assert ts == sorted(ts)
    last_day_nine = max(t for s in day_nine for t in s["ts_ns"].to_list())
    assert ts[0] - last_day_nine > 23 * 3600 * 1_000_000_000
    volume = 0.0
    for s in day_ten:
        for size in s["size"].to_list():
            volume += size
    assert 30.63 < volume < 30.65
    for s in day_ten:
        v = 0.0
        for size in s["size"].to_list():
            v += size
        assert v < volume / 3


def test_a_sixth_book_keeps_its_side_and_loses_its_touch():
    """The Friday's queue rests on the bid (+0.489) and keeps its side at every
    spacing, the second book in a row to do so, but unlike day nine it loses the
    touch: five seconds lands on one-cent spreads against 0.13 at full rate. A
    sixth book takes nothing away."""
    friday = book_grid_sweep(read_book(REAL / "btc_usd_d10_book_full.ndjson"), "BTC-USD", GRID)
    assert friday["full"]["snapshots"] == 356
    assert friday["full"]["avg_queue_imbalance"] > 0.4
    assert friday["sign_flips"] == 0
    assert friday["rows"][4]["spread_ratio"] < 0.1

    six = book_grid_across(_full_rate_books(["d5", "d6", "d7", "d8", "d9", "d10"]), "BTC-USD", GRID)
    assert six["held_spacings"] == 1
    assert six["held_without"] == [2, 1, 4, 1, 1, 1]


def test_where_a_grid_starts_decides_eleven_of_thirty_six_verdicts():
    """Each book at ten starts per spacing. Eleven of the thirty-six
    book-and-spacing verdicts depend on where the grid began, and three that
    ``book-grid`` reports as flips are kept by most starts. Only the half second
    holds at every start on every book. Day seven's quarter second is the one
    thing the start cannot move."""
    names = ["d5", "d6", "d7", "d8", "d9", "d10"]
    runs = [book_grid_phase(b, "BTC-USD", GRID, 10) for b in _full_rate_books(names)]
    assert [[s["sign_kept_phases"] for s in p["spacings"]] for p in runs] == [
        [10, 10, 10, 10, 7, 6],
        [10, 10, 10, 10, 10, 8],
        [0, 10, 4, 3, 6, 7],
        [10, 10, 9, 9, 6, 7],
        [10] * 6,
        [10] * 6,
    ]
    assert [(p["held_spacings"], p["lost_spacings"], p["split_spacings"]) for p in runs] == [
        (4, 0, 2),
        (5, 0, 1),
        (1, 1, 4),
        (2, 0, 4),
        (6, 0, 0),
        (6, 0, 0),
    ]
    # Flipped at start 0, kept by most starts: day five at 5 s and 10 s, day
    # eight at 10 s.
    misread = [
        (n, s["every_s"])
        for n, p in zip(names, runs, strict=True)
        for s in p["spacings"]
        if not s["first_phase_kept"] and s["sign_kept_phases"] * 2 > s["phases_run"]
    ]
    assert misread == [("d5", 5.0), ("d5", 10.0), ("d8", 10.0)]
    held_everywhere = [
        g
        for i, g in enumerate(GRID)
        if all(p["spacings"][i]["sign_kept_phases"] == 10 for p in runs)
    ]
    assert held_everywhere == [0.5]


@pytest.fixture(scope="module")
def day_eleven():
    return [read_ticks(REAL / f"btc_usd_d11_{i}.ndjson") for i in range(1, 7)]


def test_day_eleven_is_its_own_unbroken_run_after_a_weekend(day_ten, day_eleven):
    # An eleventh unbroken run, the Monday after day ten's Friday: 1,882 trades
    # and under half day ten's volume, with one session holding more than a third.
    ids = [i for s in day_eleven for i in s["trade_id"].to_list()]
    ts = [t for s in day_eleven for t in s["ts_ns"].to_list()]
    assert len(ids) == 1882
    assert ids == list(range(ids[0], ids[0] + len(ids)))
    assert ts == sorted(ts)
    last_day_ten = max(t for s in day_ten for t in s["ts_ns"].to_list())
    assert ts[0] - last_day_ten > 72 * 3600 * 1_000_000_000
    volume = 0.0
    for s in day_eleven:
        for size in s["size"].to_list():
            volume += size
    assert 12.72 < volume < 12.73
    fifth = 0.0
    for size in day_eleven[4]["size"].to_list():
        fifth += size
    assert fifth > volume / 3


def test_a_seventh_book_keeps_its_side_and_sees_a_tighter_touch():
    """The Monday's queue rests on the ask (-0.699) and keeps its side at every
    spacing, the third book in a row to do so. Its grids see a tighter touch than
    the book had: from half a second on, the spread is 0.58 of the full-rate one.
    A seventh book takes nothing away."""
    monday = book_grid_sweep(read_book(REAL / "btc_usd_d11_book_full.ndjson"), "BTC-USD", GRID)
    assert monday["full"]["snapshots"] == 270
    assert monday["full"]["avg_queue_imbalance"] < -0.6
    assert monday["sign_flips"] == 0
    for r in monday["rows"][1:]:
        assert r["spread_ratio"] == pytest.approx(0.58064524, abs=1e-8)

    seven = book_grid_across(
        _full_rate_books(["d5", "d6", "d7", "d8", "d9", "d10", "d11"]), "BTC-USD", GRID
    )
    assert seven["held_spacings"] == 1
    assert seven["held_without"] == [2, 1, 4, 1, 1, 1, 1]


def test_read_at_every_start_one_book_takes_one_spacing_away_not_three():
    """Seven books at ten starts. The half second still holds on all of them, so
    the count ``book-grid-across`` gives survives. What it says about each book
    does not: read at every start, day seven alone takes one spacing away, not
    three, and day five none. Six book-and-spacing verdicts held at start 0
    split elsewhere."""
    books = _full_rate_books(["d5", "d6", "d7", "d8", "d9", "d10", "d11"])
    first = book_grid_across(books, "BTC-USD", GRID)
    every = book_grid_phase_across(books, "BTC-USD", GRID, 10)
    assert every["held_spacings"] == 1
    assert every["first_phase_held_spacings"] == first["held_spacings"]
    assert first["held_without"] == [2, 1, 4, 1, 1, 1, 1]
    assert every["held_without"] == [1, 1, 2, 1, 1, 1, 1]
    assert [s["held_books"] for s in every["spacings"]] == [6, 7, 5, 5, 4, 3]
    assert [s["first_phase_books"] for s in every["spacings"]] == [6, 7, 6, 6, 6, 5]
    assert [s["lost_by"] for s in every["spacings"]] == [[2], [], [], [], [], []]
    assert [s["split_by"] for s in every["spacings"]] == [
        [],
        [],
        [2, 3],
        [2, 3],
        [0, 2, 3],
        [0, 1, 2, 3],
    ]


@pytest.fixture(scope="module")
def day_twelve():
    return [read_ticks(REAL / f"btc_usd_d12_{i}.ndjson") for i in range(1, 7)]


def test_day_twelve_is_its_own_unbroken_run_in_an_evening(day_eleven, day_twelve):
    # A twelfth unbroken run and the first recorded in the evening, the day after
    # day eleven's morning: 3,049 trades, three and a half times day eleven's
    # volume, with the first session holding more than a third.
    ids = [i for s in day_twelve for i in s["trade_id"].to_list()]
    ts = [t for s in day_twelve for t in s["ts_ns"].to_list()]
    assert len(ids) == 3049
    assert ids == list(range(ids[0], ids[0] + len(ids)))
    assert ts == sorted(ts)
    last_day_eleven = max(t for s in day_eleven for t in s["ts_ns"].to_list())
    assert ts[0] - last_day_eleven > 24 * 3600 * 1_000_000_000
    volume = 0.0
    for s in day_twelve:
        for size in s["size"].to_list():
            volume += size
    assert 44.22 < volume < 44.23
    first = 0.0
    for size in day_twelve[0]["size"].to_list():
        first += size
    assert first > volume / 3


def test_an_eighth_book_with_a_flat_queue_flips_once_and_takes_nothing_away():
    """The evening queue is nearly flat (-0.039) and flips once, at 5 s. An
    eighth book still leaves the half second held, and takes nothing away on
    its own."""
    evening = book_grid_sweep(read_book(REAL / "btc_usd_d12_book_full.ndjson"), "BTC-USD", GRID)
    assert evening["full"]["snapshots"] == 445
    assert abs(evening["full"]["avg_queue_imbalance"]) < 0.05
    assert evening["sign_flips"] == 1

    eight = book_grid_across(
        _full_rate_books(["d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12"]), "BTC-USD", GRID
    )
    assert eight["held_spacings"] == 1
    assert eight["held_without"] == [1, 1, 4, 1, 1, 1, 1, 1]


def test_every_split_spacing_is_kept_by_most_book_and_start_pairs():
    """Eight books at ten starts, counted in (book, start) pairs. Only the half
    second keeps the side at all 80, but no split spacing is near lost: the
    least kept, 10 s, still keeps it at 64. A split verdict here reads as mostly
    held, not as a coin toss."""
    books = _full_rate_books(["d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12"])
    every = book_grid_phase_across(books, "BTC-USD", GRID, 10)
    assert every["held_spacings"] == 1
    assert every["held_without"] == [1, 1, 2, 1, 1, 1, 1, 1]
    assert [s["kept_starts"] for s in every["spacings"]] == [70, 80, 68, 67, 65, 64]
    for s in every["spacings"]:
        assert s["starts_run"] == 80
        assert s["kept_starts"] * 5 >= s["starts_run"] * 4
    assert every["spacings"][5]["split_by"] == [0, 1, 2, 3, 7]


@pytest.fixture(scope="module")
def day_thirteen():
    return [read_ticks(REAL / f"btc_usd_d13_{i}.ndjson") for i in range(1, 7)]


NINE = ["d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12", "d13"]


def test_day_thirteen_is_its_own_unbroken_run_with_the_most_volume_yet(day_twelve, day_thirteen):
    # A thirteenth unbroken run, the next late afternoon: 5,228 trades, more than
    # double day twelve's volume and just above day two's, the most so far.
    ids = [i for s in day_thirteen for i in s["trade_id"].to_list()]
    ts = [t for s in day_thirteen for t in s["ts_ns"].to_list()]
    assert len(ids) == 5228
    assert ids == list(range(ids[0], ids[0] + len(ids)))
    assert ts == sorted(ts)
    last_day_twelve = max(t for s in day_twelve for t in s["ts_ns"].to_list())
    assert ts[0] - last_day_twelve > 20 * 3600 * 1_000_000_000
    volume = 0.0
    for s in day_thirteen:
        for size in s["size"].to_list():
            volume += size
    assert 89.84 < volume < 89.85
    twelve = 0.0
    for s in day_twelve:
        for size in s["size"].to_list():
            twelve += size
    assert volume > 2 * twelve


def test_a_ninth_book_keeps_its_side_at_every_spacing_and_every_start():
    """The ninth book leans firmly to the ask side (-0.43) and never flips: every
    spacing at every start keeps its side, so it takes nothing away."""
    book = read_book(REAL / "btc_usd_d13_book_full.ndjson")
    sweep = book_grid_sweep(book, "BTC-USD", GRID)
    assert sweep["full"]["snapshots"] == 475
    assert sweep["full"]["avg_queue_imbalance"] < -0.4
    assert sweep["sign_flips"] == 0
    phase = book_grid_phase(book, "BTC-USD", GRID, 10)
    assert all(s["sign_kept_phases"] == s["phases_run"] == 10 for s in phase["spacings"])

    nine = book_grid_across(_full_rate_books(NINE), "BTC-USD", GRID)
    assert nine["held_spacings"] == 1
    assert nine["held_without"] == [1, 1, 4, 1, 1, 1, 1, 1, 1]


def test_the_misses_behind_a_mostly_kept_spacing_are_mostly_one_book():
    """Nine books at ten starts. The totals read every split as mostly kept, but
    the misses are mostly one book's: day seven (book 2) misses the most starts
    at four of the five split spacings, and at 2 s keeps only 3 of its 10."""
    every = book_grid_phase_across(_full_rate_books(NINE), "BTC-USD", GRID, 10)
    assert every["held_spacings"] == 1
    assert every["held_without"] == [1, 1, 2, 1, 1, 1, 1, 1, 1]
    assert [s["kept_starts"] for s in every["spacings"]] == [80, 90, 78, 77, 75, 74]
    most = [(s["most_missed"], s["most_missed_book"]) for s in every["spacings"]]
    assert most == [(10, 2), (0, 0), (6, 2), (7, 2), (4, 2), (4, 0)]
    # Mostly kept in total, yet one book loses the side at most of its starts.
    two = every["spacings"][3]
    assert two["kept_starts"] * 5 >= two["starts_run"] * 4
    assert two["most_missed"] * 2 > every["books"][2]["spacings"][3]["phases_run"]
