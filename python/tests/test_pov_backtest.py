"""Out-of-sample volume plans -- mirrors `tests/pov_backtest.rs` case for case."""

from __future__ import annotations

import polars as pl
import pytest

from xexeclab.engine import pov_backtest, pov_schedule

BUCKET = 1_000_000_000
# An hour later, so the two captures only line up by time into the session.
LATER = 3_600 * BUCKET


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


def _lumpy() -> pl.DataFrame:
    """The capture the plan is built on: 1.0, 6.0 and 3.0 of volume."""
    return _ticks(
        [
            (0, 100.0, 1.0),
            (BUCKET, 102.0, 4.0),
            (BUCKET + 500_000_000, 104.0, 2.0),
            (2 * BUCKET, 110.0, 3.0),
        ]
    )


_RESHAPED = [(LATER, 200.0, 6.0), (LATER + BUCKET, 190.0, 1.0), (LATER + 2 * BUCKET, 210.0, 3.0)]


def _reshaped(product: str = "BTC-USD") -> pl.DataFrame:
    """The session the plan meets: busy and quiet buckets swapped -- 6.0, 1.0, 3.0."""
    return _ticks(_RESHAPED, product)


def test_replaying_a_capture_against_itself_is_the_in_sample_plan():
    df = _lumpy()
    bt = pov_backtest(df, df, BUCKET, 1.0, 0.5, 10.0, 2.0)
    plan = pov_schedule(df, "BTC-USD", BUCKET, 1.0, 0.5, 10.0, 2.0)
    # With no forecast error there is nothing to pay for.
    assert bt["profile_distance"] == 0.0
    assert bt["tracking_bps"] == 0.0
    assert bt["forecast_cost_bps"] == 0.0
    assert bt["max_participation"] == plan["participation"]
    assert bt["impact_bps"] == pytest.approx(plan["pov_impact_bps"], abs=1e-7)


def test_a_mis_forecast_profile_costs_more_than_the_oracle():
    bt = pov_backtest(_lumpy(), _reshaped(), BUCKET, 1.0, 1.0, 10.0, 2.0)
    assert bt["impact_bps"] > bt["oracle_impact_bps"]
    assert bt["forecast_cost_bps"] > 1.0


def test_participation_is_no_longer_constant_out_of_sample():
    bt = pov_backtest(_lumpy(), _reshaped(), BUCKET, 1.0, 1.0, 10.0)
    # In sample every bucket would read 0.1; the property does not survive.
    assert [s["participation"] for s in bt["schedule"]] == [0.01666667, 0.6, 0.1]
    assert bt["max_participation"] == 0.6
    assert bt["oracle_participation"] == 0.1


def test_the_forecast_can_breach_a_cap_the_oracle_respects():
    bt = pov_backtest(_lumpy(), _reshaped(), BUCKET, 1.0, 0.5, 10.0)
    # Reported, not refused: a breach is what the backtest exists to surface.
    assert bt["feasible"] is False
    assert bt["oracle_feasible"] is True


def test_the_price_is_the_plan_weighted_mean_of_the_execution_vwaps():
    bt = pov_backtest(_lumpy(), _reshaped(), BUCKET, 1.0, 1.0, 10.0)
    # 0.1 * 200 + 0.6 * 190 + 0.3 * 210, against a session VWAP of 202.
    assert bt["price"] == 197.0
    assert bt["session_vwap"] == 202.0
    assert bt["tracking_bps"] == pytest.approx(-247.52475248, abs=1e-6)


def test_the_profile_distance_measures_how_far_the_shape_moved():
    bt = pov_backtest(_lumpy(), _reshaped(), BUCKET, 1.0, 1.0, 10.0)
    assert bt["profile_distance"] == 0.5


def test_captures_are_aligned_by_time_into_the_session():
    bt = pov_backtest(_lumpy(), _reshaped(), BUCKET, 1.0, 1.0, 10.0)
    assert bt["buckets"] == 3
    assert [s["slot"] for s in bt["schedule"]] == [0, 1, 2]


def test_captures_covering_different_buckets_are_refused():
    gappy = _ticks([_RESHAPED[0], _RESHAPED[2]])
    with pytest.raises(ValueError, match="cover different buckets"):
        pov_backtest(_lumpy(), gappy, BUCKET, 1.0, 1.0, 10.0)


def test_captures_of_different_products_are_refused():
    with pytest.raises(ValueError, match="the plan capture is BTC-USD"):
        pov_backtest(_lumpy(), _reshaped("ETH-USD"), BUCKET, 1.0, 1.0, 10.0)


def test_an_empty_or_volumeless_capture_is_refused_by_name():
    with pytest.raises(ValueError, match="^plan capture: no ticks$"):
        pov_backtest(_ticks([]), _reshaped(), BUCKET, 1.0, 1.0, 10.0)
    with pytest.raises(ValueError, match="^execution capture: no ticks$"):
        pov_backtest(_lumpy(), _ticks([]), BUCKET, 1.0, 1.0, 10.0)
    with pytest.raises(ValueError, match="^execution capture: zero traded volume$"):
        pov_backtest(_lumpy(), _ticks([(LATER, 200.0, 0.0)]), BUCKET, 1.0, 1.0, 10.0)


@pytest.mark.parametrize("cap", [0.0, -0.1, 1.5])
def test_a_cap_outside_the_unit_interval_is_refused(cap):
    with pytest.raises(ValueError, match=r"^cap must be in \(0, 1\]"):
        pov_backtest(_lumpy(), _reshaped(), BUCKET, 1.0, cap, 10.0)


def test_the_bundled_next_session_meets_the_plan_from_the_first():
    from xexeclab.engine import read_ticks

    bt = pov_backtest(
        read_ticks("data/sample_ticks.ndjson"),
        read_ticks("data/sample_ticks_next.ndjson"),
        BUCKET,
        0.2,
        0.25,
        25.0,
        5.0,
    )
    # The bundled pair is the README's worked example: a nearly flat profile
    # meeting a session whose middle second is thin.
    assert bt["forecast_cost_bps"] > 0.0
    assert bt["feasible"] is False
    assert bt["oracle_feasible"] is True
