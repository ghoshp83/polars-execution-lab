import polars as pl
import pytest

from xexeclab.engine import counterfactual, read_fills, shortfall

SAMPLE = "data/sample_fills.ndjson"

_SCHEMA = {
    "ts_ns": pl.Int64,
    "product": pl.String,
    "side": pl.String,
    "qty": pl.Float64,
    "price": pl.Float64,
    "interval_volume": pl.Float64,
}

ARGS = dict(product="BTC-USD", arrival_price=30000.0, coef_bps=25.0, perm_coef_bps=5.0)


def _fills(rows: list[tuple]) -> pl.DataFrame:
    return pl.DataFrame(rows, schema=_SCHEMA, orient="row")


def _strategies(c: dict) -> list[dict]:
    return [c["realised"], *c["alternatives"]]


def test_the_schedule_is_scored_against_benchmarks_not_only_itself():
    # The point of the release. The desk paid 9.35bps all-in over these
    # intervals; a volume-following allocation of the same quantity over the same
    # path would have paid 8.46. The schedule was worth -0.88bps -- it lost.
    # Post-trade attribution alone could not have told you that, because it has
    # nothing to compare the trajectory against.
    c = counterfactual(read_fills(SAMPLE), **ARGS)
    assert c["intervals"] == 4
    assert c["best_alternative"] == "volume"
    assert c["realised"]["cost_bps"] == pytest.approx(9.34506666)
    assert c["alternatives"][0]["cost_bps"] == pytest.approx(9.14263953)
    assert c["alternatives"][1]["cost_bps"] == pytest.approx(8.46120598)
    assert c["edge_bps"] == pytest.approx(-0.88386068)


def test_the_realised_leg_reproduces_the_post_trade_attribution():
    # The comparison is only trustworthy if the realised strategy, priced here,
    # is the same execution ``shortfall`` already attributed. Drift is its
    # realised shortfall and impact is its modelled cost -- if these ever drift
    # apart, the benchmarks are being priced against a different order.
    df = read_fills(SAMPLE)
    c = counterfactual(df, **ARGS)
    s = shortfall(
        df,
        product="BTC-USD",
        parent_qty=3.5,
        arrival_price=30000.0,
        coef_bps=25.0,
        perm_coef_bps=5.0,
    )
    assert c["realised"]["drift_bps"] == pytest.approx(s["realised_bps"])
    assert c["realised"]["impact_bps"] == pytest.approx(s["modelled_bps"])


def test_every_strategy_trades_the_same_quantity():
    # A benchmark that quietly trades less would win on impact for free. Equal
    # quantity is what makes the unfilled remainder cancel and the comparison
    # fair.
    c = counterfactual(read_fills(SAMPLE), **ARGS)
    for s in _strategies(c):
        assert s["qty"] == pytest.approx(c["filled_qty"]), s["name"]


def test_the_edge_is_the_gap_to_the_best_alternative():
    c = counterfactual(read_fills(SAMPLE), **ARGS)
    best = next(a for a in c["alternatives"] if a["name"] == c["best_alternative"])
    assert best["cost_bps"] == min(a["cost_bps"] for a in c["alternatives"])
    assert c["edge_bps"] == pytest.approx(best["cost_bps"] - c["realised"]["cost_bps"])


def test_cost_is_drift_plus_impact():
    # The two terms are reported separately so a desk can see whether it lost on
    # timing or on size; the headline must stay their sum.
    c = counterfactual(read_fills(SAMPLE), **ARGS)
    for s in _strategies(c):
        assert s["cost_bps"] == pytest.approx(s["drift_bps"] + s["impact_bps"])
        for leg in s["legs"]:
            assert leg["cost_bps"] == pytest.approx(leg["drift_bps"] + leg["impact_bps"])


def test_a_model_with_no_coefficients_leaves_only_drift():
    # With no impact law there is nothing to say about size, and the comparison
    # collapses to which strategy was in the market at better prices. The
    # benchmarks must not invent an advantage out of it.
    c = counterfactual(read_fills(SAMPLE), product="BTC-USD", arrival_price=30000.0, coef_bps=0.0)
    for s in _strategies(c):
        assert s["impact_bps"] == 0.0
        assert s["cost_bps"] == pytest.approx(s["drift_bps"])


def test_a_counterfactual_the_market_could_not_have_filled_is_refused():
    # The realised fills are legal -- each took no more than its interval -- but
    # spreading the same quantity evenly would put 2.55 into an interval that
    # only ever traded 0.1. Pricing that through a law undefined above full
    # participation would be fiction, so it errors instead.
    df = _fills(
        [
            (1, "BTC-USD", "buy", 0.1, 30000.0, 0.1),
            (2, "BTC-USD", "buy", 5.0, 30030.0, 100.0),
        ]
    )
    with pytest.raises(ValueError, match="could not have filled"):
        counterfactual(df, **ARGS)


def test_bad_inputs_are_rejected():
    df = read_fills(SAMPLE)
    with pytest.raises(ValueError, match="no fills"):
        counterfactual(_fills([]), **ARGS)
    for bad in (dict(arrival_price=0.0), dict(coef_bps=-1.0), dict(perm_coef_bps=-1.0)):
        with pytest.raises(ValueError):
            counterfactual(df, **{**ARGS, **bad})
    mixed = _fills(
        [
            (1, "BTC-USD", "buy", 0.5, 30000.0, 10.0),
            (2, "BTC-USD", "sell", 0.5, 30010.0, 10.0),
        ]
    )
    with pytest.raises(ValueError, match="mix sides"):
        counterfactual(mixed, **ARGS)
