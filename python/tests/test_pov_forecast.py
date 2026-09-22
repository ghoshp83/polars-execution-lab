"""Pooled volume forecasts -- mirrors `tests/pov_forecast.rs` case for case."""

from __future__ import annotations

import polars as pl
import pytest

from xexeclab.engine import pov_backtest, pov_forecast, read_ticks

BUCKET = 1_000_000_000
# An hour between sessions, so captures only line up by time into the session.
HOUR = 3_600 * BUCKET


def _ticks(rows: list[tuple[int, float, float]], product: str = "BTC-USD") -> pl.DataFrame:
    return pl.DataFrame(
        {
            "ts_ns": [r[0] for r in rows],
            "product": [product] * len(rows),
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


def _session(start: int, volumes: list[float], product: str = "BTC-USD") -> pl.DataFrame:
    """Three one-tick buckets starting at ``start`` with the given volumes."""
    return _ticks([(start + i * BUCKET, 100.0 + i, v) for i, v in enumerate(volumes)], product)


def _lumpy(product: str = "BTC-USD") -> pl.DataFrame:
    """Shares 0.1 / 0.6 / 0.3."""
    return _session(0, [1.0, 6.0, 3.0], product)


def _reshaped() -> pl.DataFrame:
    """The opposite shape: 0.6 / 0.1 / 0.3."""
    return _session(HOUR, [6.0, 1.0, 3.0])


def _blended() -> pl.DataFrame:
    """Half of each: 0.35 / 0.35 / 0.3."""
    return _session(2 * HOUR, [3.5, 3.5, 3.0])


def _thin_tail() -> pl.DataFrame:
    """A session whose volume collapses after the first bucket, so the forward
    carry has nowhere later to put what the cap deferred and the two plans miss
    different quantities."""
    return _session(3 * HOUR, [10.0, 1.0, 0.6])


def _pinched() -> pl.DataFrame:
    """Two even buckets and a thin one, met with a cap tight enough to bind in
    the even buckets too -- what it takes to get a genuine trade-off out of the
    deferral rather than one plan dominating."""
    return _session(4 * HOUR, [1.0, 1.0, 0.02])


def test_one_history_session_is_the_backtest():
    fc = pov_forecast([_lumpy()], _reshaped(), BUCKET, 1.0, 1.0, 10.0, 2.0)
    bt = pov_backtest(_lumpy(), _reshaped(), BUCKET, 1.0, 1.0, 10.0, 2.0)
    # An average of one session is that session.
    assert fc["forecast"]["impact_bps"] == bt["impact_bps"]
    assert fc["forecast"]["forecast_cost_bps"] == bt["forecast_cost_bps"]
    assert fc["forecast"]["profile_distance"] == bt["profile_distance"]
    assert fc["forecast"]["price"] == bt["price"]
    assert fc["oracle_impact_bps"] == bt["oracle_impact_bps"]
    assert fc["naive"]["impact_bps"] == fc["forecast"]["impact_bps"]
    assert fc["improvement_bps"] == 0.0


def test_averaging_opposite_sessions_recovers_a_blended_session():
    fc = pov_forecast([_lumpy(), _reshaped()], _blended(), BUCKET, 1.0, 1.0, 10.0, 2.0)
    assert fc["sessions"] == 2
    assert fc["forecast"]["profile_distance"] == 0.0
    assert fc["forecast"]["forecast_cost_bps"] == 0.0
    assert fc["naive"]["forecast_cost_bps"] > 0.0
    assert fc["improvement_bps"] > 0.0


def test_a_session_does_not_outvote_the_others_by_trading_more():
    heavy = _session(HOUR, [60.0, 10.0, 30.0])
    fc = pov_forecast([_lumpy(), heavy], _blended(), BUCKET, 1.0, 1.0, 10.0)
    # Shares are averaged, not volumes.
    assert [s["plan_share"] for s in fc["schedule"]] == [0.35, 0.35, 0.3]


def test_the_average_can_lose_to_the_most_recent_session():
    fc = pov_forecast([_lumpy(), _reshaped()], _reshaped(), BUCKET, 1.0, 1.0, 10.0)
    # Reported, not assumed: pooling can only add error when the session repeats.
    assert fc["naive"]["forecast_cost_bps"] == 0.0
    assert fc["forecast"]["forecast_cost_bps"] > 0.0
    assert fc["improvement_bps"] < 0.0


def test_a_half_life_of_one_session_halves_the_weight_each_step_back():
    fc = pov_forecast(
        [_lumpy(), _reshaped(), _blended()], _blended(), BUCKET, 1.0, 1.0, 10.0, 0.0, 1.0
    )
    # Raw weights 0.25 / 0.5 / 1.0 over 1.75: the newest session weighs four
    # times the oldest, and the weights are reported normalised.
    assert fc["half_life"] == 1.0
    assert fc["weights"] == [0.14285714, 0.28571429, 0.57142857]
    assert abs(sum(fc["weights"]) - 1.0) < 1e-7


def test_a_short_half_life_converges_on_the_naive_forecast():
    # A half-life of a hundredth of a session leaves the older capture 2^-100 of
    # the weight, so the pooled plan *is* the last session.
    fc = pov_forecast([_lumpy(), _reshaped()], _blended(), BUCKET, 1.0, 1.0, 10.0, 0.0, 0.01)
    assert fc["weights"] == [0.0, 1.0]
    assert fc["forecast"]["impact_bps"] == fc["naive"]["impact_bps"]
    assert fc["forecast"]["profile_distance"] == fc["naive"]["profile_distance"]
    assert fc["improvement_bps"] == 0.0


def test_recency_weighting_wins_when_the_profile_has_drifted():
    # The old session is the odd one out and the market has settled into the
    # recent shape: leaning on the recent sessions beats pooling all three.
    history = [_lumpy(), _reshaped(), _reshaped()]
    exec_df = _session(3 * HOUR, [6.0, 1.0, 3.0])
    flat = pov_forecast(history, exec_df, BUCKET, 1.0, 1.0, 10.0, 0.0, 0.0)
    decayed = pov_forecast(history, exec_df, BUCKET, 1.0, 1.0, 10.0, 0.0, 0.5)
    assert decayed["forecast"]["forecast_cost_bps"] < flat["forecast"]["forecast_cost_bps"]
    assert decayed["forecast"]["profile_distance"] < flat["forecast"]["profile_distance"]
    assert decayed["improvement_bps"] > flat["improvement_bps"]


def test_a_negative_half_life_is_refused():
    with pytest.raises(
        ValueError, match="^half_life must be a non-negative finite number, got -1.0$"
    ):
        pov_forecast([_lumpy()], _blended(), BUCKET, 1.0, 1.0, 10.0, 0.0, -1.0)


def test_no_history_is_refused():
    with pytest.raises(ValueError, match="^need at least one history capture$"):
        pov_forecast([], _blended(), BUCKET, 1.0, 1.0, 10.0)


def test_a_bad_history_capture_is_named_by_position():
    with pytest.raises(ValueError, match="^history capture 2: no ticks$"):
        pov_forecast([_lumpy(), _ticks([])], _blended(), BUCKET, 1.0, 1.0, 10.0)


def test_history_covering_different_buckets_is_refused():
    gappy = _ticks([(HOUR, 100.0, 6.0), (HOUR + 2 * BUCKET, 102.0, 3.0)])
    with pytest.raises(ValueError, match="^history capture 1 covers different buckets"):
        pov_forecast([gappy], _blended(), BUCKET, 1.0, 1.0, 10.0)


def test_history_of_another_product_is_refused():
    with pytest.raises(
        ValueError, match="^history capture 1 is ETH-USD but the execution capture is BTC-USD$"
    ):
        pov_forecast([_lumpy("ETH-USD")], _blended(), BUCKET, 1.0, 1.0, 10.0)


def test_the_bundled_history_forecasts_the_third_session():
    history = [read_ticks("data/sample_ticks.ndjson"), read_ticks("data/sample_ticks_next.ndjson")]
    fc = pov_forecast(
        history, read_ticks("data/sample_ticks_third.ndjson"), BUCKET, 0.2, 0.25, 25.0, 5.0
    )
    # The README's worked example: the third session sits between its two
    # predecessors, so the pooled profile beats yesterday's alone.
    assert fc["sessions"] == 2
    assert fc["forecast"]["profile_distance"] < fc["naive"]["profile_distance"]
    assert fc["improvement_bps"] > 0.0
    assert fc["forecast"]["forecast_cost_bps"] >= 0.0


def test_a_single_history_session_capped_is_the_backtest_capped():
    fc = pov_forecast([_lumpy()], _reshaped(), BUCKET, 1.0, 0.5, 10.0, 2.0, 0.0)
    bt = pov_backtest(_lumpy(), _reshaped(), BUCKET, 1.0, 0.5, 10.0, 2.0)
    # The collapse has to hold under the cap too, or the two commands would be
    # charging the same allocation differently the moment the cap binds.
    assert fc["forecast_capped"]["capped"]["sizes"] == bt["capped"]["sizes"]
    assert fc["forecast_capped"]["capped"]["impact_bps"] == bt["capped"]["impact_bps"]
    assert fc["forecast_capped"]["spread"]["sizes"] == bt["spread"]["sizes"]
    assert fc["forecast_capped"]["spread"]["impact_bps"] == bt["spread"]["impact_bps"]
    # One session is its own naive baseline, capped executions included.
    assert fc["naive_capped"]["capped"]["sizes"] == fc["forecast_capped"]["capped"]["sizes"]


def test_the_reshape_makes_the_shortfall_a_property_of_the_session():
    # The session traded 10 and the cap admits half of it, so the whole parent
    # fits. Water-filling therefore completes whatever shape the forecast has:
    # two forecasts that disagree about every slot still miss nothing.
    fc = pov_forecast([_blended(), _lumpy()], _reshaped(), BUCKET, 1.0, 0.5, 10.0, 2.0, 0.0)
    assert fc["forecast_capped"]["spread"]["completed"]
    assert fc["naive_capped"]["spread"]["completed"]
    assert fc["forecast_capped"]["spread"]["filled_qty"] == 1.0
    assert fc["naive_capped"]["spread"]["filled_qty"] == 1.0
    assert fc["forecast_capped"]["spread"]["sizes"] != fc["naive_capped"]["spread"]["sizes"]


def test_a_cap_the_session_cannot_fill_misses_the_same_under_either_forecast():
    # cap * exec_volume is 0.5 against a parent of 1.0, so no allocation can
    # fill more than half -- and the reshape misses exactly the other half.
    fc = pov_forecast([_blended(), _lumpy()], _reshaped(), BUCKET, 1.0, 0.05, 10.0, 2.0, 0.0)
    assert fc["forecast_capped"]["spread"]["unfilled_qty"] == 0.5
    assert fc["naive_capped"]["spread"]["unfilled_qty"] == 0.5
    assert fc["forecast_capped"]["spread"]["sizes"] == fc["naive_capped"]["spread"]["sizes"]


def test_deferring_still_charges_the_forecast_for_its_shape():
    # The reshape hides the difference between the two forecasts; the forward
    # carry does not. The naive plan puts 0.6 into the thinnest slot and cannot
    # recover it before the close, while the pooled one clears.
    fc = pov_forecast([_blended(), _lumpy()], _reshaped(), BUCKET, 1.0, 0.2, 10.0, 2.0, 0.0)
    assert fc["naive_capped"]["capped"]["unfilled_qty"] == 0.1
    assert not fc["naive_capped"]["capped"]["completed"]
    assert fc["forecast_capped"]["capped"]["unfilled_qty"] == 0.0
    assert fc["forecast_capped"]["capped"]["completed"]
    # Deferring can only ever miss more than re-shaping, which is the bracket.
    assert (
        fc["naive_capped"]["capped"]["unfilled_qty"] >= fc["naive_capped"]["spread"]["unfilled_qty"]
    )
    assert (
        fc["forecast_capped"]["capped"]["unfilled_qty"]
        >= fc["forecast_capped"]["spread"]["unfilled_qty"]
    )


def test_filled_impact_prices_the_quantity_that_actually_traded():
    # impact_bps is per unit of parent, so an execution the cap left short
    # reports a smaller number for having traded less. filled_impact_bps
    # divides by what filled instead, so it does not fall with the shortfall.
    fc = pov_forecast([_blended(), _lumpy()], _reshaped(), BUCKET, 1.0, 0.2, 10.0, 2.0, 0.0)
    forecast = fc["forecast_capped"]["capped"]
    naive = fc["naive_capped"]["capped"]
    # The pooled plan cleared, so there is nothing to re-base: the two agree.
    assert forecast["completed"]
    assert forecast["filled_impact_bps"] == forecast["impact_bps"]
    # The naive plan missed 0.1 of the parent, and that is exactly what made
    # its impact_bps the smaller of the two numbers.
    assert not naive["completed"]
    assert naive["filled_impact_bps"] > naive["impact_bps"]


def test_the_capped_improvement_reports_what_the_cheaper_plan_did_not_fill():
    # Under the forward carry the naive plan misses 0.1 and the pooled one
    # clears. Netting that into a single number would let a plan look better
    # for trading less, so the shortfall is reported beside the improvement.
    fc = pov_forecast([_blended(), _lumpy()], _reshaped(), BUCKET, 1.0, 0.2, 10.0, 2.0, 0.0)
    gain = fc["capped_improvement"]["capped"]
    # Negative: the pooled plan missed *less* than the naive one.
    assert gain["shortfall_qty"] == -0.1
    assert not gain["like_for_like"]
    # The reshape fills the whole parent for either forecast, so there the same
    # comparison is like for like and the shortfall term vanishes.
    spread = fc["capped_improvement"]["spread"]
    assert spread["shortfall_qty"] == 0.0
    assert spread["like_for_like"]


def test_a_slack_cap_leaves_the_capped_improvement_the_uncapped_one():
    # With the cap slack neither execution binds, both fill the whole parent,
    # and filled_impact_bps is just the plan's own impact. The capped question
    # then has to give back the uncapped answer.
    fc = pov_forecast([_blended(), _lumpy()], _reshaped(), BUCKET, 1.0, 1.0, 10.0, 2.0, 0.0)
    assert fc["forecast_capped"]["capped"]["capped_slots"] == 0
    assert fc["naive_capped"]["capped"]["capped_slots"] == 0
    assert fc["forecast_capped"]["capped"]["filled_impact_bps"] == fc["forecast"]["impact_bps"]
    assert fc["naive_capped"]["capped"]["filled_impact_bps"] == fc["naive"]["impact_bps"]
    assert fc["capped_improvement"]["capped"]["like_for_like"]
    # Equal up to the rounding, which lands differently on a difference of
    # rounded numbers than on a rounded difference.
    assert (
        abs(fc["capped_improvement"]["capped"]["improvement_bps"] - fc["improvement_bps"]) <= 1e-8
    )


def test_a_session_too_thin_for_the_parent_leaves_the_forecasts_nothing_to_win():
    # cap * exec_volume is 0.5 against a parent of 1.0, so the reshape fills
    # exactly half whatever the forecast said. Both plans miss the same and
    # trade the same, so pooling is worth nothing under the spread.
    fc = pov_forecast([_blended(), _lumpy()], _reshaped(), BUCKET, 1.0, 0.05, 10.0, 2.0, 0.0)
    spread = fc["capped_improvement"]["spread"]
    assert spread["shortfall_qty"] == 0.0
    assert spread["like_for_like"]
    assert spread["improvement_bps"] == 0.0


def test_without_a_price_for_the_remainder_nothing_is_netted():
    # The default: improvement is in basis points and the shortfall is in
    # quantity, and with no rate between them neither is folded into the other.
    fc = pov_forecast([_blended(), _lumpy()], _reshaped(), BUCKET, 1.0, 0.2, 10.0, 2.0, 0.0)
    assert fc["shortfall_bps"] is None
    assert fc["capped_improvement"]["capped"]["net_bps"] is None
    assert fc["capped_improvement"]["spread"]["net_bps"] is None


def test_a_price_for_the_remainder_charges_the_shortfall():
    # Under the forward carry the naive plan misses 0.1 of a parent of 1.0 and
    # the pooled one clears, so the pooled plan is credited a tenth of the rate.
    fc = pov_forecast([_blended(), _lumpy()], _reshaped(), BUCKET, 1.0, 0.2, 10.0, 2.0, 0.0, 50.0)
    assert fc["shortfall_bps"] == 50.0
    gain = fc["capped_improvement"]["capped"]
    assert gain["shortfall_qty"] == -0.1
    assert not gain["like_for_like"]
    assert abs(gain["net_bps"] - (gain["improvement_bps"] + 0.1 * 50.0)) <= 1e-8
    # The reshape filled the whole parent both ways, so the rate cannot move it.
    spread = fc["capped_improvement"]["spread"]
    assert spread["like_for_like"]
    assert spread["net_bps"] == spread["improvement_bps"]


def test_a_rate_of_zero_is_not_the_same_as_no_rate():
    # Deciding the remainder is free is a statement, and not the one silence
    # makes: the number is reported, it merely equals the improvement.
    fc = pov_forecast([_blended(), _lumpy()], _reshaped(), BUCKET, 1.0, 0.2, 10.0, 2.0, 0.0, 0.0)
    assert fc["shortfall_bps"] == 0.0
    gain = fc["capped_improvement"]["capped"]
    assert gain["shortfall_qty"] != 0.0
    assert gain["net_bps"] == gain["improvement_bps"]


def test_a_negative_price_for_the_remainder_is_refused():
    # A negative rate would pay a plan for missing the order.
    with pytest.raises(
        ValueError, match="shortfall_bps must be a non-negative finite number, got -1"
    ):
        pov_forecast([_blended(), _lumpy()], _reshaped(), BUCKET, 1.0, 0.2, 10.0, 2.0, 0.0, -1.0)


def _pinched_plan(rate: float | None = None) -> dict:
    return pov_forecast([_blended(), _lumpy()], _pinched(), BUCKET, 1.0, 0.3, 10.0, 2.0, 0.0, rate)


def test_the_breakeven_rate_nets_the_gain_to_zero():
    # A definition, not an estimate: charge the shortfall at this rate and the
    # gain cancels exactly. That is what lets the engine report it without
    # claiming to know what a missed unit is worth.
    gain = _pinched_plan()["capped_improvement"]["capped"]
    assert not gain["like_for_like"]
    rate = gain["breakeven_bps"]
    assert rate is not None
    assert abs(_pinched_plan(rate)["capped_improvement"]["capped"]["net_bps"]) <= 1e-8


def test_either_side_of_the_breakeven_the_verdict_is_opposite():
    # The point of reporting it: a desk that cannot name a price for a missed
    # unit can still say whether its price is above or below this one.
    base = _pinched_plan()["capped_improvement"]["capped"]
    rate = base["breakeven_bps"]
    cheap = _pinched_plan(rate / 2.0)["capped_improvement"]["capped"]["net_bps"]
    dear = _pinched_plan(rate * 2.0)["capped_improvement"]["capped"]["net_bps"]
    assert (cheap > 0.0) == (base["improvement_bps"] > 0.0)
    assert cheap * dear < 0.0


def test_a_like_for_like_gain_has_no_breakeven_rate():
    # Nothing was missed, so no rate can change the answer and there is no
    # threshold to report. The reshape fits the whole parent into this session
    # whatever shape the plan has.
    spread = pov_forecast(
        [_lumpy(), _reshaped()], _thin_tail(), BUCKET, 1.0, 0.2, 10.0, 2.0, 0.0, 50.0
    )["capped_improvement"]["spread"]
    assert spread["like_for_like"]
    assert spread["breakeven_bps"] is None
    assert spread["net_bps"] == spread["improvement_bps"]


def test_a_plan_that_is_cheaper_and_misses_less_has_no_breakeven_rate():
    # The other ``None``: the two figures point the same way, so one plan wins
    # outright and no non-negative rate reverses it. A negative "breakeven"
    # would be a rate that pays for missing the order.
    fc = pov_forecast(
        [_reshaped(), _lumpy()], _thin_tail(), BUCKET, 1.0, 0.2, 10.0, 2.0, 0.0, 500.0
    )
    gain = fc["capped_improvement"]["capped"]
    assert gain["shortfall_qty"] < 0.0
    assert gain["improvement_bps"] > 0.0
    assert gain["breakeven_bps"] is None
    assert gain["net_bps"] > gain["improvement_bps"]
