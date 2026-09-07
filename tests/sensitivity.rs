use xexec::counterfactual::counterfactual;
use xexec::model::Fill;
use xexec::replay::read_fills;
use xexec::sensitivity::sensitivity;

fn load() -> Vec<Fill> {
    read_fills("data/sample_fills.ndjson").expect("sample fill replay must load")
}

fn fill(ts_ns: i64, side: &str, qty: f64, price: f64, interval_volume: f64) -> Fill {
    Fill {
        ts_ns,
        product: "BTC-USD".into(),
        side: side.into(),
        qty,
        price,
        interval_volume,
    }
}

const ARRIVAL: f64 = 30000.0;
const GRID: [f64; 5] = [10.0, 15.0, 20.0, 25.0, 30.0];

/// A schedule that wins on timing and loses on concentration: nearly all of it
/// goes into one thin interval at the arrival price, and the rest into a deep
/// one after the market has run 20bps away. Cheap when impact is cheap,
/// expensive when it is not -- so the verdict changes hands inside the grid.
fn crossing() -> Vec<Fill> {
    vec![
        fill(1, "buy", 2.0, 30000.0, 4.0),
        fill(2, "buy", 0.1, 30060.0, 100.0),
    ]
}

#[test]
fn the_verdict_is_reported_against_the_coefficient_it_was_priced_with() {
    // The point of the release: v0.14.0 said "-0.88bps" as though 25 were the
    // coefficient. It is a fitted number, so the whole comparison is re-run
    // across the range it could plausibly have taken.
    let s = sensitivity(&load(), "BTC-USD", ARRIVAL, &GRID, 5.0).unwrap();
    assert_eq!(s.points.len(), 5);
    assert!((s.points[3].coef_bps - 25.0).abs() < 1e-12);
    assert!((s.points[3].edge_bps - -0.88386068).abs() < 1e-8);
    assert!((s.edge_min_bps - -0.93569234).abs() < 1e-8);
    assert!((s.edge_max_bps - -0.7283657).abs() < 1e-8);
}

#[test]
fn a_stable_verdict_does_not_depend_on_the_calibration() {
    // This is what the release is for. The sample's schedule loses to
    // volume-following at every coefficient in the grid, so the 0.88bps in
    // v0.14.0 is a property of the schedule and not of the number fed in.
    let s = sensitivity(&load(), "BTC-USD", ARRIVAL, &GRID, 5.0).unwrap();
    assert!(s.points.iter().all(|p| p.best_alternative == "volume"));
    assert!(s.edge_max_bps < 0.0);
    assert_eq!(s.sign_flips, 0);
    assert!(s.verdict_stable);
    // Nothing crossed zero, so there is no breakeven to report -- and reporting
    // one anyway would be inventing a number.
    assert!(s.breakeven_coef_bps.is_none());
}

#[test]
fn a_verdict_that_flips_is_reported_as_unstable() {
    // The same benchmark wins at every point here, yet the answer still changes:
    // the realised schedule beats it at 10bps and loses to it at 30. A constant
    // winner is not a stable verdict, which is why stability needs both halves.
    let grid = [10.0, 20.0, 30.0, 40.0];
    let s = sensitivity(&crossing(), "BTC-USD", ARRIVAL, &grid, 0.0).unwrap();
    assert!(s.points.iter().all(|p| p.best_alternative == "twap"));
    assert!(s.points[0].edge_bps > 0.0);
    assert!(s.points[3].edge_bps < 0.0);
    assert_eq!(s.sign_flips, 1);
    assert!(!s.verdict_stable);
}

#[test]
fn the_breakeven_coefficient_is_exact_not_interpolated() {
    // The edge is affine in coef_bps -- drift does not depend on it and both
    // impact terms are linear in their coefficients -- so interpolating between
    // the bracketing grid points lands on the true crossing, not near it. Priced
    // at the reported breakeven, the two schedules cost exactly the same.
    let grid = [10.0, 20.0, 30.0, 40.0];
    let s = sensitivity(&crossing(), "BTC-USD", ARRIVAL, &grid, 0.0).unwrap();
    let breakeven = s.breakeven_coef_bps.expect("the edge crosses zero here");
    assert!((breakeven - 24.61720435).abs() < 1e-8);
    assert!(breakeven > 20.0 && breakeven < 30.0);
    let at = counterfactual(&crossing(), "BTC-USD", ARRIVAL, breakeven, 0.0).unwrap();
    assert!(at.edge_bps.abs() < 1e-6);
}

#[test]
fn each_point_is_the_counterfactual_at_that_coefficient() {
    // The sweep must not become a second implementation of the comparison; every
    // point has to be what running counterfactual directly would have said.
    let s = sensitivity(&load(), "BTC-USD", ARRIVAL, &GRID, 5.0).unwrap();
    for p in &s.points {
        let direct = counterfactual(&load(), "BTC-USD", ARRIVAL, p.coef_bps, 5.0).unwrap();
        assert_eq!(p.edge_bps, direct.edge_bps);
        assert_eq!(p.best_alternative, direct.best_alternative);
        assert_eq!(p.realised_cost_bps, direct.realised.cost_bps);
    }
}

#[test]
fn the_reported_range_brackets_every_point() {
    let s = sensitivity(&load(), "BTC-USD", ARRIVAL, &GRID, 5.0).unwrap();
    for p in &s.points {
        assert!(p.edge_bps >= s.edge_min_bps);
        assert!(p.edge_bps <= s.edge_max_bps);
    }
}

#[test]
fn a_grid_that_cannot_be_swept_is_rejected() {
    // One point is not a sensitivity, and an unsorted grid would make the
    // bracketing interpolation meaningless.
    let fills = load();
    for (grid, want) in [
        (vec![25.0], "at least 2 points"),
        (vec![30.0, 10.0], "strictly increasing"),
        (vec![10.0, 10.0], "strictly increasing"),
        (vec![-1.0, 10.0], "non-negative"),
    ] {
        let err = sensitivity(&fills, "BTC-USD", ARRIVAL, &grid, 5.0)
            .expect_err("grid must be rejected")
            .to_string();
        assert!(err.contains(want), "{err} should mention {want}");
    }
}

#[test]
fn bad_fills_are_refused_by_the_comparison_underneath() {
    // The sweep adds no validation of its own for the replay; it must not
    // swallow the refusals counterfactual already makes.
    let mixed = vec![
        fill(1, "buy", 1.0, 30030.0, 10.0),
        fill(2, "sell", 1.0, 30060.0, 10.0),
    ];
    let err = sensitivity(&mixed, "BTC-USD", ARRIVAL, &GRID, 5.0)
        .expect_err("mixed sides must be rejected")
        .to_string();
    assert!(err.contains("mix sides"), "{err}");
    assert!(sensitivity(&load(), "BTC-USD", -1.0, &GRID, 5.0).is_err());
}
