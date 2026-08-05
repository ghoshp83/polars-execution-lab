import polars as pl
import pytest

from xexeclab.engine import calibrate_impact
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
