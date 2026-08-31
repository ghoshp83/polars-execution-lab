import math

import polars as pl
import pytest

from xexeclab.engine import read_book, sweep_curve

SAMPLE = "data/sample_book.ndjson"
LADDER = [0.5, 1.0, 2.0, 3.0, 10.0]

_SCHEMA = {
    "ts_ns": pl.Int64,
    "product": pl.String,
    "side": pl.String,
    "level": pl.Int64,
    "price": pl.Float64,
    "size": pl.Float64,
}


def test_curve_fits_the_impact_coefficient_from_the_sample_book():
    c = sweep_curve(read_book(SAMPLE), "BTC-USD", "buy", LADDER)
    # Mean ask depth is (4.8 + 3.0 + 3.0 + 2.7 + 4.5) / 5 = 3.6, so a 2.0 order
    # is 55.6% participation. Values hand-checked against the fixture.
    assert c["snapshots"] == 5
    assert c["avg_depth"] == pytest.approx(3.6)
    assert c["points"] == 5
    assert c["coef_bps"] == pytest.approx(0.03953661, abs=1e-8)
    assert c["curve"][2]["participation"] == pytest.approx(0.55555556, abs=1e-8)


def test_short_fills_are_reported_but_never_regressed():
    # The whole honesty claim of the module: a size the captured book cannot
    # fill paid only for the liquidity that was there, so its cost understates
    # the truth. Such a point must still be *shown* -- with its short fill --
    # but must not drag the fitted coefficient down.
    c = sweep_curve(read_book(SAMPLE), "BTC-USD", "buy", LADDER)
    assert c["fitted_points"] == 3
    short = [p for p in c["curve"] if not p["fitted"]]
    assert [p["order_size"] for p in short] == [3.0, 10.0]
    assert all(p["fill_ratio"] < 1.0 for p in short)

    fitted_only = sweep_curve(read_book(SAMPLE), "BTC-USD", "buy", [0.5, 1.0, 2.0])
    assert fitted_only["coef_bps"] == c["coef_bps"]


def test_the_fitted_law_reprices_every_point_and_the_residual_is_the_gap():
    # modelled_bps must be the model's own prediction at that point and
    # residual_bps the measured-minus-modelled gap -- otherwise the diagnostic
    # that tells you the square-root law fits the book badly is meaningless.
    c = sweep_curve(read_book(SAMPLE), "BTC-USD", "buy", LADDER)
    for p in c["curve"]:
        expected = c["coef_bps"] * math.sqrt(p["participation"])
        assert p["modelled_bps"] == pytest.approx(expected, abs=1e-7)
        assert p["residual_bps"] == pytest.approx(p["measured_bps"] - p["modelled_bps"], abs=1e-8)


def test_the_ladder_is_sorted_and_measured_cost_rises_with_size():
    # The curve is reported cheapest-first whatever order the ladder arrives in,
    # and the book must charge more for more size -- if it did not, the sweep
    # underneath would be wrong and the fitted coefficient meaningless.
    c = sweep_curve(read_book(SAMPLE), "BTC-USD", "buy", [2.0, 0.5, 1.0])
    sizes = [p["order_size"] for p in c["curve"]]
    costs = [p["measured_bps"] for p in c["curve"]]
    assert sizes == [0.5, 1.0, 2.0]
    assert costs == sorted(costs)


def test_a_sell_curve_prices_the_other_side_of_the_book():
    df = read_book(SAMPLE)
    buy = sweep_curve(df, "BTC-USD", "buy", [1.0, 2.0])
    sell = sweep_curve(df, "BTC-USD", "sell", [1.0, 2.0])
    # Each side rests its own depth, so participation has a different
    # denominator and the recovered coefficient must differ too.
    assert sell["avg_depth"] != buy["avg_depth"]
    assert sell["coef_bps"] != buy["coef_bps"]


def test_bad_inputs_raise():
    df = read_book(SAMPLE)
    with pytest.raises(ValueError):
        sweep_curve(pl.DataFrame([], schema=_SCHEMA), "BTC-USD", "buy", [1.0, 2.0])
    # One point cannot separate a curve from a single observation.
    with pytest.raises(ValueError):
        sweep_curve(df, "BTC-USD", "buy", [1.0])
    with pytest.raises(ValueError):
        sweep_curve(df, "BTC-USD", "sideways", [1.0, 2.0])
    # A NaN has no ordering, so it must be rejected before the ladder is sorted.
    with pytest.raises(ValueError):
        sweep_curve(df, "BTC-USD", "buy", [1.0, float("nan")])
    with pytest.raises(ValueError):
        sweep_curve(df, "BTC-USD", "buy", [1.0, 1.0])
    # A ladder the book cannot fill anywhere leaves nothing honest to fit.
    with pytest.raises(ValueError):
        sweep_curve(df, "BTC-USD", "buy", [10.0, 20.0])
