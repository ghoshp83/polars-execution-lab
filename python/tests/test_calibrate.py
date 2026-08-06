import polars as pl
import pytest

from xexeclab.engine import (
    calibrate_impact,
    calibrate_impact_robust,
    read_calibration,
)

SAMPLE = "data/sample_calibration.ndjson"
NOISY = "data/sample_calibration_noisy.ndjson"

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


def test_robust_with_no_options_reproduces_ordinary_least_squares():
    # huber_delta=None and ridge_lambda=0 leave every weight at 1 and add nothing
    # to the diagonal, so the robust path matches the plain fit exactly -- checked
    # on the noisy design where the coefficients are non-trivial.
    noisy = read_calibration(NOISY)
    ols = calibrate_impact(noisy, "BTC-USD")
    same = calibrate_impact_robust(noisy, "BTC-USD")
    assert same == ols


def test_huber_down_weights_an_outlier_toward_the_clean_truth():
    # The noisy replay is the clean 10*sqrt(p)+20*p design plus one gross outlier.
    # Plain least squares is wrecked by it -- the coefficients blow out to
    # ~80 / -24, and a *negative* permanent impact is physically nonsense.
    # Huber bounds the bad print's pull (it caps influence, it does not reject),
    # so both terms stay near the generating values and the permanent term keeps
    # its sign.
    noisy = read_calibration(NOISY)
    ols = calibrate_impact(noisy, "BTC-USD")
    robust = calibrate_impact_robust(noisy, "BTC-USD", huber_delta=3.0)
    assert ols["perm_coef_bps"] < 0.0  # outlier flips OLS permanent impact negative
    assert robust["perm_coef_bps"] > 0.0  # robust keeps it positive
    assert abs(robust["coef_bps"] - 10.0) < abs(ols["coef_bps"] - 10.0)
    assert abs(robust["perm_coef_bps"] - 20.0) < abs(ols["perm_coef_bps"] - 20.0)
    assert robust["coef_bps"] == pytest.approx(10.0, abs=4.0)
    assert robust["perm_coef_bps"] == pytest.approx(20.0, abs=3.0)


def test_ridge_makes_a_single_participation_level_solvable():
    # One participation level is singular for the plain fit; a non-zero ridge
    # penalty regularises the matrix so the fit returns finite coefficients.
    one_level = _samples([(0, "BTC-USD", 0.1, 1.0), (1, "BTC-USD", 0.1, 1.1)])
    with pytest.raises(ValueError):
        calibrate_impact(one_level, "BTC-USD")
    m = calibrate_impact_robust(one_level, "BTC-USD", ridge_lambda=0.5)
    assert m["coef_bps"] == m["coef_bps"]  # finite (not NaN)
    assert m["perm_coef_bps"] == m["perm_coef_bps"]


def test_robust_empty_samples_raise():
    with pytest.raises(ValueError):
        calibrate_impact_robust(_samples([]), "BTC-USD", huber_delta=3.0)
