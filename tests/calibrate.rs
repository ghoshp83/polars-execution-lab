use xexec::calibrate::calibrate_impact;
use xexec::model::CalibrationSample;
use xexec::replay::read_calibration;

fn load() -> Vec<CalibrationSample> {
    read_calibration("data/sample_calibration.ndjson").expect("sample calibration replay must load")
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
