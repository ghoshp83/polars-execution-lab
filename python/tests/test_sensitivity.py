import polars as pl
import pytest

from xexeclab.engine import counterfactual, read_fills, sensitivity

SAMPLE = "data/sample_fills.ndjson"

_SCHEMA = {
    "ts_ns": pl.Int64,
    "product": pl.String,
    "side": pl.String,
    "qty": pl.Float64,
    "price": pl.Float64,
    "interval_volume": pl.Float64,
}

GRID = [10.0, 15.0, 20.0, 25.0, 30.0]

# A schedule that wins on timing and loses on concentration: nearly all of it
# goes into one thin interval at the arrival price, and the rest into a deep one
# after the market has run 20bps away. Cheap when impact is cheap, expensive when
# it is not -- so the verdict changes hands somewhere inside the grid.
_CROSSING = [
    (1, "BTC-USD", "buy", 2.0, 30000.0, 4.0),
    (2, "BTC-USD", "buy", 0.1, 30060.0, 100.0),
]


def _fills(rows: list[tuple]) -> pl.DataFrame:
    return pl.DataFrame(rows, schema=_SCHEMA, orient="row")


def test_the_verdict_is_reported_against_the_coefficient_it_was_priced_with():
    # The point of the release: v0.14.0 said "-0.88bps" as though 25 were the
    # coefficient. It is a fitted number, so the whole comparison is re-run
    # across the range it could plausibly have taken.
    s = sensitivity(read_fills(SAMPLE), "BTC-USD", 30000.0, GRID, 5.0)
    assert [p["coef_bps"] for p in s["points"]] == GRID
    assert s["points"][3]["coef_bps"] == 25.0
    assert s["points"][3]["edge_bps"] == pytest.approx(-0.88386068, abs=1e-8)
    assert s["edge_min_bps"] == pytest.approx(-0.93569234, abs=1e-8)
    assert s["edge_max_bps"] == pytest.approx(-0.7283657, abs=1e-8)


def test_a_stable_verdict_does_not_depend_on_the_calibration():
    # This is what the release is for. The sample's schedule loses to
    # volume-following at every coefficient in the grid, so the 0.88bps in
    # v0.14.0 is a property of the schedule and not of the number fed in.
    s = sensitivity(read_fills(SAMPLE), "BTC-USD", 30000.0, GRID, 5.0)
    assert {p["best_alternative"] for p in s["points"]} == {"volume"}
    assert s["edge_max_bps"] < 0.0
    assert s["sign_flips"] == 0
    assert s["verdict_stable"] is True
    # Nothing crossed zero, so there is no breakeven to report -- and reporting
    # one anyway would be inventing a number.
    assert s["breakeven_coef_bps"] is None


def test_a_verdict_that_flips_is_reported_as_unstable():
    # The same benchmark wins at every point here, yet the answer still changes:
    # the realised schedule beats it at 10bps and loses to it at 30. A constant
    # winner is not a stable verdict, which is why stability needs both halves.
    s = sensitivity(_fills(_CROSSING), "BTC-USD", 30000.0, [10.0, 20.0, 30.0, 40.0], 0.0)
    assert {p["best_alternative"] for p in s["points"]} == {"twap"}
    assert s["points"][0]["edge_bps"] > 0.0
    assert s["points"][-1]["edge_bps"] < 0.0
    assert s["sign_flips"] == 1
    assert s["verdict_stable"] is False


def test_the_breakeven_coefficient_is_exact_not_interpolated():
    # The edge is affine in coef_bps -- drift does not depend on it and both
    # impact terms are linear in their coefficients -- so interpolating between
    # the bracketing grid points lands on the true crossing, not near it. Priced
    # at the reported breakeven, the two schedules cost exactly the same.
    s = sensitivity(_fills(_CROSSING), "BTC-USD", 30000.0, [10.0, 20.0, 30.0, 40.0], 0.0)
    breakeven = s["breakeven_coef_bps"]
    assert breakeven == pytest.approx(24.61720435, abs=1e-8)
    assert 20.0 < breakeven < 30.0
    at = counterfactual(_fills(_CROSSING), "BTC-USD", 30000.0, breakeven, 0.0)
    assert at["edge_bps"] == pytest.approx(0.0, abs=1e-6)


def test_each_point_is_the_counterfactual_at_that_coefficient():
    # The sweep must not become a second implementation of the comparison; every
    # point has to be what running counterfactual directly would have said.
    df = read_fills(SAMPLE)
    s = sensitivity(df, "BTC-USD", 30000.0, GRID, 5.0)
    for point in s["points"]:
        direct = counterfactual(df, "BTC-USD", 30000.0, point["coef_bps"], 5.0)
        assert point["edge_bps"] == direct["edge_bps"]
        assert point["best_alternative"] == direct["best_alternative"]
        assert point["realised_cost_bps"] == direct["realised"]["cost_bps"]


def test_the_reported_range_brackets_every_point():
    s = sensitivity(read_fills(SAMPLE), "BTC-USD", 30000.0, GRID, 5.0)
    edges = [p["edge_bps"] for p in s["points"]]
    assert s["edge_min_bps"] == min(edges)
    assert s["edge_max_bps"] == max(edges)


def test_a_grid_that_cannot_be_swept_is_rejected():
    df = read_fills(SAMPLE)
    # One point is not a sensitivity, and an unsorted grid would make the
    # bracketing interpolation meaningless.
    with pytest.raises(ValueError, match="at least 2 points"):
        sensitivity(df, "BTC-USD", 30000.0, [25.0], 5.0)
    with pytest.raises(ValueError, match="strictly increasing"):
        sensitivity(df, "BTC-USD", 30000.0, [30.0, 10.0], 5.0)
    with pytest.raises(ValueError, match="strictly increasing"):
        sensitivity(df, "BTC-USD", 30000.0, [10.0, 10.0], 5.0)
    with pytest.raises(ValueError, match="non-negative"):
        sensitivity(df, "BTC-USD", 30000.0, [-1.0, 10.0], 5.0)


def test_bad_fills_are_refused_by_the_comparison_underneath():
    # The sweep adds no validation of its own for the replay; it must not
    # swallow the refusals counterfactual already makes.
    rows = [
        (1, "BTC-USD", "buy", 1.0, 30030.0, 10.0),
        (2, "BTC-USD", "sell", 1.0, 30060.0, 10.0),
    ]
    with pytest.raises(ValueError, match="mix sides"):
        sensitivity(_fills(rows), "BTC-USD", 30000.0, GRID, 5.0)
    with pytest.raises(ValueError, match="arrival_price"):
        sensitivity(read_fills(SAMPLE), "BTC-USD", -1.0, GRID, 5.0)
