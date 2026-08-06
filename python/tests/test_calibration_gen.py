import polars as pl
import pytest

from xexeclab.engine import calibrate_impact, calibrate_impact_robust
from xexeclab.ingest import synthetic_calibration


def test_noiseless_samples_recover_the_generating_coefficients(tmp_path):
    # Round-trip: generate fills exactly on the model, then recover its
    # coefficients. This is the property the whole harness rests on.
    out = tmp_path / "calib.ndjson"
    n = synthetic_calibration(out, n=200, coef_bps=12.0, perm_coef_bps=18.0, noise_bps=0.0, seed=3)
    assert n == 200
    m = calibrate_impact(pl.read_ndjson(out), "BTC-USD")
    assert m["coef_bps"] == pytest.approx(12.0, abs=1e-6)
    assert m["perm_coef_bps"] == pytest.approx(18.0, abs=1e-6)
    assert m["r_squared"] == pytest.approx(1.0, abs=1e-9)


def test_noisy_samples_recover_coefficients_approximately(tmp_path):
    # With measurement noise the fit no longer explains all the variance, but
    # the estimates stay in the right neighbourhood -- exactly what calibrating
    # against real fills looks like.
    out = tmp_path / "calib.ndjson"
    synthetic_calibration(out, n=500, coef_bps=12.0, perm_coef_bps=18.0, noise_bps=0.5, seed=3)
    m = calibrate_impact(pl.read_ndjson(out), "BTC-USD")
    assert m["coef_bps"] == pytest.approx(12.0, abs=1.0)
    assert m["perm_coef_bps"] == pytest.approx(18.0, abs=2.0)
    assert m["rmse_bps"] > 0.0
    assert m["r_squared"] < 1.0


def test_no_outliers_leaves_the_draw_sequence_unchanged(tmp_path):
    # outlier_frac=0 must not consume any extra RNG draws, so the replay is
    # byte-identical to the pre-outlier generator (back-compat).
    a = tmp_path / "a.ndjson"
    b = tmp_path / "b.ndjson"
    synthetic_calibration(a, n=50, noise_bps=0.4, seed=3)
    synthetic_calibration(b, n=50, noise_bps=0.4, outlier_frac=0.0, outlier_bps=99.0, seed=3)
    assert a.read_text() == b.read_text()


def test_outliers_wreck_ols_but_robust_shrugs_them_off(tmp_path):
    # A tenth of the fills carry an 80 bps shock. Plain least squares is dragged
    # materially off the generating coefficients; the Huber fit bounds each bad
    # print's pull and stays strictly closer on both terms. (Under one-sided
    # contamination Huber caps the damage rather than fully removing it, so the
    # honest claim is "much closer than OLS", not "back on the truth".)
    out = tmp_path / "calib.ndjson"
    synthetic_calibration(
        out,
        n=300,
        coef_bps=12.0,
        perm_coef_bps=18.0,
        noise_bps=0.3,
        outlier_frac=0.1,
        outlier_bps=80.0,
        seed=3,
    )
    df = pl.read_ndjson(out)
    ols = calibrate_impact(df, "BTC-USD")
    robust = calibrate_impact_robust(df, "BTC-USD", huber_delta=3.0)
    # the outliers really do damage the plain fit
    assert abs(ols["perm_coef_bps"] - 18.0) > 5.0
    # and the robust fit is strictly closer on both terms
    assert abs(robust["coef_bps"] - 12.0) < abs(ols["coef_bps"] - 12.0)
    assert abs(robust["perm_coef_bps"] - 18.0) < abs(ols["perm_coef_bps"] - 18.0)
