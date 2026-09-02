import pytest

from xexeclab.engine import optimal_schedule

# One parent order of 3.0 against 2.0 of volume per interval, priced with a
# 25 bps temporary and 5 bps permanent coefficient. Hand-checkable: the TWAP
# trades 0.5 per slice, i.e. 25% participation.
ARGS = dict(
    product="BTC-USD",
    slices=6,
    total_size=3.0,
    per_slice_volume=2.0,
    coef_bps=25.0,
    perm_coef_bps=5.0,
)


def test_the_schedule_beats_the_twap_it_is_measured_against():
    # The whole point of the search: with inventory risk priced, some
    # front-load is cheaper than trading uniformly. Values hand-checked.
    m = optimal_schedule(**ARGS, sigma_bps=8.0)
    assert m["urgency"] == pytest.approx(0.7)
    assert m["impact_bps"] == pytest.approx(13.98295273, abs=1e-8)
    assert m["risk_bps"] == pytest.approx(3.58880415, abs=1e-8)
    assert m["total_bps"] == pytest.approx(17.57175688, abs=1e-8)
    assert m["twap_total_bps"] == pytest.approx(17.78686714, abs=1e-8)
    assert m["saving_bps"] == pytest.approx(0.21511026, abs=1e-8)
    # It buys the saving by paying *more* impact to carry *less* risk.
    assert m["impact_bps"] > m["twap_impact_bps"]
    assert m["risk_bps"] < m["twap_risk_bps"]


def test_with_no_volatility_the_twap_is_optimal():
    # Front-loading only pays for itself against timing risk. Remove the risk
    # and the concave impact law makes uniform trading strictly cheapest, so
    # the optimiser must return the TWAP, not a spuriously urgent schedule.
    m = optimal_schedule(**ARGS, sigma_bps=0.0)
    assert m["urgency"] == 0.0
    assert m["risk_bps"] == 0.0
    assert m["saving_bps"] == 0.0
    assert m["total_bps"] == pytest.approx(m["twap_total_bps"])
    assert m["impact_bps"] == pytest.approx(13.75)


def test_urgency_rises_with_volatility():
    # The economic claim of the module: the more the mid can move against you,
    # the earlier you trade. If this stopped holding, the trade-off would be
    # mispriced.
    urgencies = [optimal_schedule(**ARGS, sigma_bps=s)["urgency"] for s in (0.0, 4.0, 8.0, 60.0)]
    assert urgencies == sorted(urgencies)
    assert urgencies[-1] > urgencies[0] == 0.0


def test_the_schedule_trades_the_whole_order_and_front_loads_it():
    # The weights must be a partition of the parent: every slice smaller than
    # the one before, summing to one, with nothing left outstanding at the end.
    m = optimal_schedule(**ARGS, sigma_bps=8.0)
    weights = [s["weight"] for s in m["schedule"]]
    assert weights == sorted(weights, reverse=True)
    assert sum(weights) == pytest.approx(1.0)
    assert sum(s["size"] for s in m["schedule"]) == pytest.approx(ARGS["total_size"])
    assert m["schedule"][-1]["remaining"] == pytest.approx(0.0)
    assert [s["slice"] for s in m["schedule"]] == list(range(6))


def test_slice_costs_are_the_repo_impact_model_weighted_by_size():
    # Each slice's contribution must be the same two-term law impact_curve
    # prices, scaled by the fraction of the parent it trades -- so the totals
    # are in basis points of the parent, not an average of per-slice bps.
    m = optimal_schedule(**ARGS, sigma_bps=8.0)
    for s in m["schedule"]:
        assert s["temp_bps"] == pytest.approx(
            ARGS["coef_bps"] * s["participation"] ** 0.5 * s["weight"], abs=1e-7
        )
        assert s["perm_bps"] == pytest.approx(
            ARGS["perm_coef_bps"] * s["participation"] * s["weight"], abs=1e-7
        )
    total = sum(s["temp_bps"] + s["perm_bps"] for s in m["schedule"])
    assert m["impact_bps"] == pytest.approx(total, abs=1e-7)


def test_an_order_too_big_for_the_interval_is_refused_not_clipped():
    # 3.0 against 1.0 of volume over 2 slices needs 150% participation even
    # when spread evenly. Silently clipping would report a cheap schedule that
    # cannot be traded.
    with pytest.raises(ValueError, match="uniform schedule"):
        optimal_schedule(
            product="BTC-USD",
            slices=2,
            total_size=3.0,
            per_slice_volume=1.0,
            coef_bps=25.0,
            perm_coef_bps=5.0,
            sigma_bps=8.0,
        )


def test_bad_inputs_raise():
    with pytest.raises(ValueError):
        optimal_schedule(**{**ARGS, "slices": 0}, sigma_bps=8.0)
    with pytest.raises(ValueError):
        optimal_schedule(**{**ARGS, "total_size": 0.0}, sigma_bps=8.0)
    with pytest.raises(ValueError):
        optimal_schedule(**{**ARGS, "per_slice_volume": float("nan")}, sigma_bps=8.0)
    # A negative coefficient would make impact a reward and the search would
    # run to the most urgent schedule on the grid.
    with pytest.raises(ValueError):
        optimal_schedule(**{**ARGS, "coef_bps": -1.0}, sigma_bps=8.0)
    with pytest.raises(ValueError):
        optimal_schedule(**ARGS, sigma_bps=-1.0)
