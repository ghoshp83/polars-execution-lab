import polars as pl
import pytest

from xexeclab.engine import read_fills, shortfall

SAMPLE = "data/sample_fills.ndjson"

_SCHEMA = {
    "ts_ns": pl.Int64,
    "product": pl.String,
    "side": pl.String,
    "qty": pl.Float64,
    "price": pl.Float64,
    "interval_volume": pl.Float64,
}

# The parent is 3.5 against 3.0 filled, so the sample deliberately leaves a
# remainder: the opportunity term has something to charge.
ARGS = dict(product="BTC-USD", parent_qty=3.5, arrival_price=30000.0, coef_bps=25.0)


def _fills(rows: list[tuple]) -> pl.DataFrame:
    return pl.DataFrame(rows, schema=_SCHEMA, orient="row")


def test_the_execution_is_scored_against_the_model_not_only_the_arrival_price():
    # The point of the release: 3.67bps was paid, the impact model says an order
    # of these sizes was always going to cost 5.68bps, so the desk beat the cost
    # of its own size by 2.01bps. Scoring on realised_bps alone would have called
    # this a 3.67bps cost and told you nothing about the execution.
    s = shortfall(read_fills(SAMPLE), perm_coef_bps=5.0, **ARGS)
    assert s["fills"] == 4
    assert s["realised_bps"] == pytest.approx(3.66666667, abs=1e-8)
    assert s["modelled_bps"] == pytest.approx(5.67839999, abs=1e-8)
    assert s["residual_bps"] == pytest.approx(-2.01173332, abs=1e-8)
    assert s["residual_bps"] == pytest.approx(s["realised_bps"] - s["modelled_bps"], abs=1e-8)


def test_unfilled_quantity_is_charged_not_dropped():
    # An algorithm can always flatter its average price by not finishing. The
    # 0.5 that never filled is charged the drift it walked away from, so the
    # headline on the parent (4.00bps) is worse than the 3.67bps it "achieved".
    s = shortfall(read_fills(SAMPLE), perm_coef_bps=5.0, **ARGS)
    assert s["unfilled_qty"] == pytest.approx(0.5, abs=1e-8)
    assert s["opportunity_bps"] == pytest.approx(0.85714286, abs=1e-8)
    assert s["total_bps"] == pytest.approx(4.0, abs=1e-8)
    assert s["total_bps"] > s["realised_bps"]


def test_a_parent_that_fully_filled_pays_no_opportunity_cost():
    # Same fills, parent sized to what was actually done: nothing was abandoned,
    # so there is no drift to charge and the headline is the realised cost.
    s = shortfall(
        read_fills(SAMPLE),
        product="BTC-USD",
        parent_qty=3.0,
        arrival_price=30000.0,
        coef_bps=25.0,
        perm_coef_bps=5.0,
    )
    assert s["fill_rate"] == pytest.approx(1.0, abs=1e-12)
    assert s["opportunity_bps"] == 0.0
    assert s["total_bps"] == pytest.approx(s["realised_bps"], abs=1e-8)


def test_the_headline_is_the_parent_weighted_sum_of_its_parts():
    s = shortfall(read_fills(SAMPLE), perm_coef_bps=5.0, **ARGS)
    # Tolerance is 1e-7, not 1e-8: the parts are reported rounded to 8dp, so
    # recombining them cannot be exact to the last place.
    assert s["total_bps"] == pytest.approx(
        s["fill_rate"] * s["realised_bps"] + s["opportunity_bps"], abs=1e-7
    )


def test_a_model_with_no_coefficients_explains_nothing():
    # The attribution must not invent explanatory power. With both coefficients
    # zero every basis point paid is residual, which is what makes residual_bps
    # readable as "the part the model does not explain".
    s = shortfall(
        read_fills(SAMPLE),
        product="BTC-USD",
        parent_qty=3.5,
        arrival_price=30000.0,
        coef_bps=0.0,
        perm_coef_bps=0.0,
    )
    assert s["modelled_bps"] == 0.0
    assert s["residual_bps"] == pytest.approx(s["realised_bps"], abs=1e-8)
    assert all(sl["modelled_bps"] == 0.0 for sl in s["slices"])


def test_a_seller_filling_above_the_arrival_price_shows_a_gain():
    # Shortfall is signed against the parent, not against the tape: a sell that
    # printed above its decision price earned money and must read negative.
    rows = [
        (1, "BTC-USD", "sell", 1.0, 30030.0, 10.0),
        (2, "BTC-USD", "sell", 1.0, 30060.0, 10.0),
    ]
    s = shortfall(_fills(rows), "BTC-USD", 2.0, 30000.0, 0.0, 0.0)
    assert s["realised_bps"] == pytest.approx(-15.0, abs=1e-8)
    assert s["total_bps"] == pytest.approx(-15.0, abs=1e-8)


def test_fills_that_mix_sides_are_rejected():
    # Netting a buy against a sell would produce a signed number with no meaning.
    rows = [
        (1, "BTC-USD", "buy", 1.0, 30030.0, 10.0),
        (2, "BTC-USD", "sell", 1.0, 30060.0, 10.0),
    ]
    with pytest.raises(ValueError, match="mix sides"):
        shortfall(_fills(rows), "BTC-USD", 2.0, 30000.0, 10.0)


def test_bad_inputs_are_rejected():
    good = [(1, "BTC-USD", "buy", 1.0, 30030.0, 10.0)]
    with pytest.raises(ValueError, match="no fills"):
        shortfall(_fills([]), "BTC-USD", 1.0, 30000.0, 10.0)
    with pytest.raises(ValueError, match="parent_qty"):
        shortfall(_fills(good), "BTC-USD", 0.0, 30000.0, 10.0)
    with pytest.raises(ValueError, match="arrival_price"):
        shortfall(_fills(good), "BTC-USD", 1.0, -1.0, 10.0)
    with pytest.raises(ValueError, match="coef_bps"):
        shortfall(_fills(good), "BTC-USD", 1.0, 30000.0, -1.0)
    # A child cannot take more than its interval held, and the fills cannot
    # total more than the parent: both are reconciliation errors, and reporting
    # a participation or a fill rate above 1 would bury them.
    with pytest.raises(ValueError, match="available in its interval"):
        shortfall(
            _fills([(1, "BTC-USD", "buy", 11.0, 30030.0, 10.0)]), "BTC-USD", 20.0, 30000.0, 10.0
        )
    with pytest.raises(ValueError, match="against a parent"):
        shortfall(_fills(good), "BTC-USD", 0.5, 30000.0, 10.0)
