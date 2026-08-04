import polars as pl
import pytest

from xexeclab.engine import impact_curve, read_impact

SAMPLE = "data/sample_impact.ndjson"

_SCHEMA = {
    "ts_ns": pl.Int64,
    "product": pl.String,
    "participation": pl.Float64,
}


def _schedule(rows: list[tuple]) -> pl.DataFrame:
    return pl.DataFrame(rows, schema=_SCHEMA, orient="row")


def test_square_root_curve_summarises_the_sample_schedule():
    m = impact_curve(read_impact(SAMPLE), "BTC-USD", 10.0)
    # participations 0.01/0.04/0.09/0.16/0.25 -> sqrt 0.1..0.5; coef_bps=10 gives
    # per-slice impact 1/2/3/4/5 bps: avg 3, max 5, total 15 (hand-checked).
    assert m["slices"] == 5
    assert m["avg_impact_bps"] == pytest.approx(3.0)
    assert m["max_impact_bps"] == pytest.approx(5.0)
    assert m["total_impact_bps"] == pytest.approx(15.0)


def test_impact_is_concave_in_participation():
    # The square-root law is concave: doubling participation less than doubles
    # the cost, which is exactly the behaviour a linear model misses.
    small = impact_curve(_schedule([(0, "BTC-USD", 0.25)]), "BTC-USD", 10.0)
    big = impact_curve(_schedule([(0, "BTC-USD", 0.5)]), "BTC-USD", 10.0)
    assert big["avg_impact_bps"] > small["avg_impact_bps"]
    assert big["avg_impact_bps"] < 2.0 * small["avg_impact_bps"]


def test_coef_bps_scales_the_curve_linearly():
    one = impact_curve(_schedule([(0, "BTC-USD", 0.25)]), "BTC-USD", 10.0)
    two = impact_curve(_schedule([(0, "BTC-USD", 0.25)]), "BTC-USD", 20.0)
    assert two["avg_impact_bps"] == pytest.approx(2.0 * one["avg_impact_bps"])


def test_permanent_term_is_linear_and_adds_to_total_cost():
    # Permanent impact is linear in participation: perm_coef_bps * participation.
    # On the sample (participations sum 0.55, mean 0.11) with perm_coef_bps=20 the
    # permanent cost is avg 2.2, total 11.0; total_cost = temporary 15 + perm 11.
    m = impact_curve(read_impact(SAMPLE), "BTC-USD", 10.0, 20.0)
    assert m["avg_perm_impact_bps"] == pytest.approx(2.2)
    assert m["total_perm_impact_bps"] == pytest.approx(11.0)
    assert m["total_impact_bps"] == pytest.approx(15.0)
    assert m["total_cost_bps"] == pytest.approx(26.0)


def test_zero_perm_coef_is_the_pure_square_root_curve():
    # No permanent coefficient -> permanent fields vanish and the full round-trip
    # cost collapses to the temporary square-root total (back-compat default).
    m = impact_curve(read_impact(SAMPLE), "BTC-USD", 10.0, 0.0)
    assert m["perm_coef_bps"] == pytest.approx(0.0)
    assert m["total_perm_impact_bps"] == pytest.approx(0.0)
    assert m["total_cost_bps"] == pytest.approx(m["total_impact_bps"])


def test_empty_schedule_raises():
    with pytest.raises(ValueError):
        impact_curve(_schedule([]), "BTC-USD", 10.0)
