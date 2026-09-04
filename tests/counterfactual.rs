use xexec::counterfactual::counterfactual;
use xexec::model::Fill;
use xexec::replay::read_fills;
use xexec::shortfall::shortfall;

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

#[test]
fn the_schedule_is_scored_against_benchmarks_not_only_itself() {
    // The point of the release. The desk paid 9.35bps all-in over these
    // intervals; a volume-following allocation of the same quantity over the
    // same path would have paid 8.46. The schedule was worth -0.88bps -- it
    // lost. Post-trade attribution alone could not have told you that, because
    // it has nothing to compare the trajectory against.
    let c = counterfactual(&load(), "BTC-USD", ARRIVAL, 25.0, 5.0).unwrap();
    assert_eq!(c.intervals, 4);
    assert_eq!(c.best_alternative, "volume");
    assert!((c.realised.cost_bps - 9.34506666).abs() < 1e-8);
    assert!((c.alternatives[0].cost_bps - 9.14263953).abs() < 1e-8);
    assert!((c.alternatives[1].cost_bps - 8.46120598).abs() < 1e-8);
    assert!((c.edge_bps - -0.88386068).abs() < 1e-8);
}

#[test]
fn the_realised_leg_reproduces_the_post_trade_attribution() {
    // The comparison is only trustworthy if the realised strategy, priced by
    // this module, is the same execution `shortfall` already attributed. Drift
    // is its realised shortfall and impact is its modelled cost -- if these ever
    // drift apart, the benchmarks are being priced against a different order.
    let fills = load();
    let c = counterfactual(&fills, "BTC-USD", ARRIVAL, 25.0, 5.0).unwrap();
    let s = shortfall(&fills, "BTC-USD", 3.5, ARRIVAL, 25.0, 5.0).unwrap();
    assert!((c.realised.drift_bps - s.realised_bps).abs() < 1e-8);
    assert!((c.realised.impact_bps - s.modelled_bps).abs() < 1e-8);
}

#[test]
fn every_strategy_trades_the_same_quantity() {
    // A benchmark that quietly trades less would win on impact for free. Equal
    // quantity is what makes the unfilled remainder cancel and the comparison
    // fair.
    let c = counterfactual(&load(), "BTC-USD", ARRIVAL, 25.0, 5.0).unwrap();
    assert!((c.realised.qty - c.filled_qty).abs() < 1e-8);
    for a in &c.alternatives {
        assert!((a.qty - c.filled_qty).abs() < 1e-8, "{} traded {}", a.name, a.qty);
    }
}

#[test]
fn the_edge_is_the_gap_to_the_best_alternative() {
    let c = counterfactual(&load(), "BTC-USD", ARRIVAL, 25.0, 5.0).unwrap();
    let best = c
        .alternatives
        .iter()
        .find(|a| a.name == c.best_alternative)
        .unwrap();
    for a in &c.alternatives {
        assert!(best.cost_bps <= a.cost_bps);
    }
    assert!((c.edge_bps - (best.cost_bps - c.realised.cost_bps)).abs() < 1e-8);
}

#[test]
fn cost_is_drift_plus_impact() {
    // The two terms are reported separately so a desk can see whether it lost on
    // timing or on size; the headline must stay their sum.
    let c = counterfactual(&load(), "BTC-USD", ARRIVAL, 25.0, 5.0).unwrap();
    for s in std::iter::once(&c.realised).chain(c.alternatives.iter()) {
        assert!((s.cost_bps - (s.drift_bps + s.impact_bps)).abs() < 1e-8);
        for leg in &s.legs {
            assert!((leg.cost_bps - (leg.drift_bps + leg.impact_bps)).abs() < 1e-8);
        }
    }
}

#[test]
fn a_model_with_no_coefficients_leaves_only_drift() {
    // With no impact law there is nothing to say about size, and the comparison
    // collapses to which strategy was in the market at better prices. The
    // benchmarks must not invent an advantage out of it.
    let c = counterfactual(&load(), "BTC-USD", ARRIVAL, 0.0, 0.0).unwrap();
    for s in std::iter::once(&c.realised).chain(c.alternatives.iter()) {
        assert!(s.impact_bps.abs() < 1e-12);
        assert!((s.cost_bps - s.drift_bps).abs() < 1e-12);
    }
}

#[test]
fn a_counterfactual_the_market_could_not_have_filled_is_refused() {
    // The realised fills are legal -- each took no more than its interval -- but
    // spreading the same quantity evenly would put 2.55 into an interval that
    // only ever traded 0.1. Pricing that through a law undefined above full
    // participation would be fiction, so it errors instead.
    let fills = vec![
        fill(1, "buy", 0.1, 30000.0, 0.1),
        fill(2, "buy", 5.0, 30030.0, 100.0),
    ];
    let err = counterfactual(&fills, "BTC-USD", ARRIVAL, 25.0, 5.0).unwrap_err();
    assert!(err.to_string().contains("could not have filled"), "{err}");
}

#[test]
fn bad_inputs_are_rejected() {
    let fills = load();
    assert!(counterfactual(&[], "BTC-USD", ARRIVAL, 25.0, 5.0).is_err());
    assert!(counterfactual(&fills, "BTC-USD", 0.0, 25.0, 5.0).is_err());
    assert!(counterfactual(&fills, "BTC-USD", ARRIVAL, -1.0, 5.0).is_err());
    assert!(counterfactual(&fills, "BTC-USD", ARRIVAL, 25.0, -1.0).is_err());
    let mixed = vec![
        fill(1, "buy", 0.5, 30000.0, 10.0),
        fill(2, "sell", 0.5, 30010.0, 10.0),
    ];
    assert!(counterfactual(&mixed, "BTC-USD", ARRIVAL, 25.0, 5.0).is_err());
}
