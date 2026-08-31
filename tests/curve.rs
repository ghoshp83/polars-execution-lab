use xexec::curve::sweep_curve;
use xexec::model::BookLevel;
use xexec::replay::read_book;

const LADDER: [f64; 5] = [0.5, 1.0, 2.0, 3.0, 10.0];

fn load() -> Vec<BookLevel> {
    read_book("data/sample_book.ndjson").expect("sample book replay must load")
}

#[test]
fn curve_fits_the_impact_coefficient_from_the_sample_book() {
    let c = sweep_curve(&load(), "BTC-USD", "buy", &LADDER).unwrap();
    // Mean ask depth is (4.8 + 3.0 + 3.0 + 2.7 + 4.5) / 5 = 3.6, so a 2.0 order
    // is 55.6% participation. Values hand-checked against the fixture.
    assert_eq!(c.snapshots, 5);
    assert!((c.avg_depth - 3.6).abs() < 1e-9);
    assert_eq!(c.points, 5);
    assert!((c.coef_bps - 0.03953661).abs() < 1e-8);
    assert!((c.curve[2].participation - 0.55555556).abs() < 1e-8);
}

#[test]
fn short_fills_are_reported_but_never_regressed() {
    // The whole honesty claim of the module: a size the captured book cannot
    // fill paid only for the liquidity that was there, so its cost understates
    // the truth. Such a point must still be *shown* -- with its short fill --
    // but must not drag the fitted coefficient down.
    let c = sweep_curve(&load(), "BTC-USD", "buy", &LADDER).unwrap();
    assert_eq!(c.fitted_points, 3);
    let short: Vec<&_> = c.curve.iter().filter(|p| !p.fitted).collect();
    assert_eq!(short.len(), 2);
    assert!(short.iter().all(|p| p.fill_ratio < 1.0));
    assert!((short[0].order_size - 3.0).abs() < 1e-9);
    assert!((short[1].order_size - 10.0).abs() < 1e-9);

    let fitted_only = sweep_curve(&load(), "BTC-USD", "buy", &[0.5, 1.0, 2.0]).unwrap();
    assert_eq!(fitted_only.coef_bps, c.coef_bps);
}

#[test]
fn the_fitted_law_reprices_every_point_and_the_residual_is_the_gap() {
    // modelled_bps must be the model's own prediction at that point and
    // residual_bps the measured-minus-modelled gap -- otherwise the diagnostic
    // that tells you the square-root law fits the book badly is meaningless.
    let c = sweep_curve(&load(), "BTC-USD", "buy", &LADDER).unwrap();
    for p in &c.curve {
        let expected = c.coef_bps * p.participation.sqrt();
        assert!((p.modelled_bps - expected).abs() < 1e-7, "{p:?}");
        assert!((p.residual_bps - (p.measured_bps - p.modelled_bps)).abs() < 1e-8);
    }
}

#[test]
fn the_ladder_is_sorted_and_measured_cost_rises_with_size() {
    // The curve is reported cheapest-first whatever order the ladder arrives in,
    // and the book must charge more for more size -- if it did not, the sweep
    // underneath would be wrong and the fitted coefficient meaningless.
    let c = sweep_curve(&load(), "BTC-USD", "buy", &[2.0, 0.5, 1.0]).unwrap();
    let sizes: Vec<f64> = c.curve.iter().map(|p| p.order_size).collect();
    let costs: Vec<f64> = c.curve.iter().map(|p| p.measured_bps).collect();
    assert_eq!(sizes, vec![0.5, 1.0, 2.0]);
    assert!(costs.windows(2).all(|w| w[0] <= w[1]), "{costs:?}");
}

#[test]
fn a_sell_curve_prices_the_other_side_of_the_book() {
    let levels = load();
    let buy = sweep_curve(&levels, "BTC-USD", "buy", &[1.0, 2.0]).unwrap();
    let sell = sweep_curve(&levels, "BTC-USD", "sell", &[1.0, 2.0]).unwrap();
    // Each side rests its own depth, so participation has a different
    // denominator and the recovered coefficient must differ too.
    assert!(sell.avg_depth != buy.avg_depth);
    assert!(sell.coef_bps != buy.coef_bps);
}

#[test]
fn bad_inputs_are_rejected() {
    let levels = load();
    assert!(sweep_curve(&[], "BTC-USD", "buy", &[1.0, 2.0]).is_err());
    // One point cannot separate a curve from a single observation.
    assert!(sweep_curve(&levels, "BTC-USD", "buy", &[1.0]).is_err());
    assert!(sweep_curve(&levels, "BTC-USD", "sideways", &[1.0, 2.0]).is_err());
    // A NaN has no ordering, so it must be rejected before the ladder is sorted.
    assert!(sweep_curve(&levels, "BTC-USD", "buy", &[1.0, f64::NAN]).is_err());
    assert!(sweep_curve(&levels, "BTC-USD", "buy", &[1.0, 1.0]).is_err());
    // A ladder the book cannot fill anywhere leaves nothing honest to fit.
    assert!(sweep_curve(&levels, "BTC-USD", "buy", &[10.0, 20.0]).is_err());
}
