use xexec::calibrate::{calibrate_impact, calibrate_impact_robust};
use xexec::model::CalibrationSample;
use xexec::replay::read_calibration;

fn load() -> Vec<CalibrationSample> {
    read_calibration("data/sample_calibration.ndjson").expect("sample calibration replay must load")
}

fn load_noisy() -> Vec<CalibrationSample> {
    read_calibration("data/sample_calibration_noisy.ndjson")
        .expect("noisy calibration replay must load")
}

fn sample(ts_ns: i64, participation: f64, realised_bps: f64) -> CalibrationSample {
    CalibrationSample {
        ts_ns,
        product: "BTC-USD".into(),
        participation,
        realised_bps,
    }
}

#[test]
fn recovers_the_coefficients_that_generated_the_samples() {
    // The sample realised costs are exactly 10*sqrt(p) + 20*p, so the two-term
    // fit must recover coef_bps ~ 10 (temporary) and perm_coef_bps ~ 20
    // (permanent) with a near-perfect fit.
    let m = calibrate_impact(&load(), "BTC-USD").unwrap();
    assert_eq!(m.samples, 5);
    assert!((m.coef_bps - 10.0).abs() < 1e-6);
    assert!((m.perm_coef_bps - 20.0).abs() < 1e-6);
    assert!((m.r_squared - 1.0).abs() < 1e-9);
    assert!(m.rmse_bps.abs() < 1e-6);
}

#[test]
fn a_pure_temporary_cost_fits_a_zero_permanent_term() {
    // realised = 10*sqrt(p) with no permanent component: the permanent
    // coefficient should come out ~ 0.
    let s: Vec<CalibrationSample> = [(0i64, 0.04f64), (1, 0.09), (2, 0.16), (3, 0.25)]
        .iter()
        .map(|&(t, p)| sample(t, p, 10.0 * p.sqrt()))
        .collect();
    let m = calibrate_impact(&s, "BTC-USD").unwrap();
    assert!((m.coef_bps - 10.0).abs() < 1e-6);
    assert!(m.perm_coef_bps.abs() < 1e-6);
}

#[test]
fn a_single_participation_level_is_singular() {
    // One distinct participation cannot separate the square-root term from the
    // linear term, so the fit is rejected rather than blowing up.
    let s = vec![sample(0, 0.1, 1.0), sample(1, 0.1, 1.1)];
    assert!(calibrate_impact(&s, "BTC-USD").is_err());
}

#[test]
fn empty_samples_are_rejected() {
    assert!(calibrate_impact(&[], "BTC-USD").is_err());
}

#[test]
fn robust_with_no_options_reproduces_ordinary_least_squares() {
    // huber_delta = None and ridge_lambda = 0 must leave every weight at 1 and
    // add nothing to the diagonal, so the robust path is bit-identical to the
    // plain fit -- checked on the noisy design where the coefficients are
    // non-trivial.
    let noisy = load_noisy();
    let ols = calibrate_impact(&noisy, "BTC-USD").unwrap();
    let same = calibrate_impact_robust(&noisy, "BTC-USD", None, 0.0, 8).unwrap();
    assert_eq!(ols.coef_bps, same.coef_bps);
    assert_eq!(ols.perm_coef_bps, same.perm_coef_bps);
    assert_eq!(ols.rmse_bps, same.rmse_bps);
    assert_eq!(ols.r_squared, same.r_squared);
}

#[test]
fn huber_down_weights_an_outlier_toward_the_clean_truth() {
    // The noisy replay is the clean 10*sqrt(p)+20*p design plus one gross
    // outlier. Plain least squares is wrecked by it (coefficients blow out to
    // ~80 / -24, and a negative permanent impact is nonsense); the Huber fit
    // bounds the bad print's pull, so both terms stay near the generating values
    // and the permanent term keeps its sign.
    let noisy = load_noisy();
    let ols = calibrate_impact(&noisy, "BTC-USD").unwrap();
    let robust = calibrate_impact_robust(&noisy, "BTC-USD", Some(3.0), 0.0, 8).unwrap();
    assert!(ols.perm_coef_bps < 0.0); // outlier flips OLS permanent impact negative
    assert!(robust.perm_coef_bps > 0.0); // robust keeps it positive
    assert!((robust.coef_bps - 10.0).abs() < (ols.coef_bps - 10.0).abs());
    assert!((robust.perm_coef_bps - 20.0).abs() < (ols.perm_coef_bps - 20.0).abs());
    assert!((robust.coef_bps - 10.0).abs() < 4.0);
    assert!((robust.perm_coef_bps - 20.0).abs() < 3.0);
}

#[test]
fn ridge_makes_a_single_participation_level_solvable() {
    // One participation level is singular for the plain fit; a non-zero ridge
    // penalty regularises the normal matrix so the fit returns finite,
    // shrunk-toward-zero coefficients instead of being rejected.
    let s = vec![sample(0, 0.1, 1.0), sample(1, 0.1, 1.1)];
    assert!(calibrate_impact(&s, "BTC-USD").is_err());
    let m = calibrate_impact_robust(&s, "BTC-USD", None, 0.5, 8).unwrap();
    assert!(m.coef_bps.is_finite());
    assert!(m.perm_coef_bps.is_finite());
}

#[test]
fn robust_empty_samples_are_rejected() {
    assert!(calibrate_impact_robust(&[], "BTC-USD", Some(3.0), 0.0, 8).is_err());
}
