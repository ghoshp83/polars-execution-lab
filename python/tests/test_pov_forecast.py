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
