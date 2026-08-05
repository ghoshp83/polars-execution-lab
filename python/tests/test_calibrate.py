import polars as pl
import pytest

from xexeclab.engine import calibrate_impact, read_calibration

SAMPLE = "data/sample_calibration.ndjson"

_SCHEMA = {
    "ts_ns": pl.Int64,
    "product": pl.String,
    "participation": pl.Float64,
    "realised_bps": pl.Float64,
}


def _samples(rows: list[tuple]) -> pl.DataFrame:
    return pl.DataFrame(rows, schema=_SCHEMA, orient="row")


def test_recovers_the_coefficients_that_generated_the_samples():
    # The sample realised costs are exactly 10*sqrt(p) + 20*p, so the two-term
    # fit recovers coef_bps ~ 10 (temporary) and perm_coef_bps ~ 20 (permanent)
    # with r^2 ~ 1 and ~ 0 residual (hand-checked).
    m = calibrate_impact(read_calibration(SAMPLE), "BTC-USD")
    assert m["samples"] == 5
    assert m["coef_bps"] == pytest.approx(10.0, abs=1e-6)
    assert m["perm_coef_bps"] == pytest.approx(20.0, abs=1e-6)
    assert m["r_squared"] == pytest.approx(1.0, abs=1e-9)
    assert m["rmse_bps"] == pytest.approx(0.0, abs=1e-6)


def test_pure_temporary_cost_fits_a_zero_permanent_term():
    # realised = 10*sqrt(p) with no permanent component -> perm_coef_bps ~ 0.
    rows = [(i, "BTC-USD", p, 10.0 * p**0.5) for i, p in enumerate((0.04, 0.09, 0.16, 0.25))]
    m = calibrate_impact(_samples(rows), "BTC-USD")
    assert m["coef_bps"] == pytest.approx(10.0, abs=1e-6)
    assert m["perm_coef_bps"] == pytest.approx(0.0, abs=1e-6)


def test_single_participation_level_is_singular():
    # One distinct participation cannot separate the two basis functions.
    m = _samples([(0, "BTC-USD", 0.1, 1.0), (1, "BTC-USD", 0.1, 1.1)])
    with pytest.raises(ValueError):
        calibrate_impact(m, "BTC-USD")


def test_empty_samples_raise():
    with pytest.raises(ValueError):
        calibrate_impact(_samples([]), "BTC-USD")
