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

// The parent is 3.5 against 3.0 filled, so the sample deliberately leaves a
// remainder: the opportunity term has something to charge.
const PARENT: f64 = 3.5;
const ARRIVAL: f64 = 30000.0;

#[test]
fn the_execution_is_scored_against_the_model_not_only_the_arrival_price() {
    // The point of the release: 3.67bps was paid, the impact model says an order
    // of these sizes was always going to cost 5.68bps, so the desk beat the cost
    // of its own size by 2.01bps. Scoring on realised_bps alone would have called
    // this a 3.67bps cost and told you nothing about the execution.
    let s = shortfall(&load(), "BTC-USD", PARENT, ARRIVAL, 25.0, 5.0).unwrap();
    assert_eq!(s.fills, 4);
    assert!((s.realised_bps - 3.66666667).abs() < 1e-8);
    assert!((s.modelled_bps - 5.67839999).abs() < 1e-8);
    assert!((s.residual_bps - -2.01173332).abs() < 1e-8);
    assert!((s.residual_bps - (s.realised_bps - s.modelled_bps)).abs() < 1e-8);
}

#[test]
fn unfilled_quantity_is_charged_not_dropped() {
    // An algorithm can always flatter its average price by not finishing. The
    // 0.5 that never filled is charged the drift it walked away from, so the
    // headline on the parent (4.00bps) is worse than the 3.67bps it "achieved".
    let s = shortfall(&load(), "BTC-USD", PARENT, ARRIVAL, 25.0, 5.0).unwrap();
    assert!((s.unfilled_qty - 0.5).abs() < 1e-8);
    assert!((s.opportunity_bps - 0.85714286).abs() < 1e-8);
    assert!((s.total_bps - 4.0).abs() < 1e-8);
    assert!(s.total_bps > s.realised_bps);
}

#[test]
fn a_parent_that_fully_filled_pays_no_opportunity_cost() {
    // Same fills, parent sized to what was actually done: nothing was abandoned,
    // so there is no drift to charge and the headline is the realised cost.
    let s = shortfall(&load(), "BTC-USD", 3.0, ARRIVAL, 25.0, 5.0).unwrap();
    assert!((s.fill_rate - 1.0).abs() < 1e-12);
    assert_eq!(s.opportunity_bps, 0.0);
    assert!((s.total_bps - s.realised_bps).abs() < 1e-8);
}

#[test]
fn the_headline_is_the_parent_weighted_sum_of_its_parts() {
    let s = shortfall(&load(), "BTC-USD", PARENT, ARRIVAL, 25.0, 5.0).unwrap();
    // Tolerance is 1e-7, not 1e-8: the parts are reported rounded to 8dp, so
    // recombining them cannot be exact to the last place.
    let parts = s.fill_rate * s.realised_bps + s.opportunity_bps;
    assert!((s.total_bps - parts).abs() < 1e-7);
}

#[test]
fn a_model_with_no_coefficients_explains_nothing() {
    // The attribution must not invent explanatory power. With both coefficients
    // zero every basis point paid is residual, which is what makes residual_bps
    // readable as "the part the model does not explain".
    let s = shortfall(&load(), "BTC-USD", PARENT, ARRIVAL, 0.0, 0.0).unwrap();
    assert_eq!(s.modelled_bps, 0.0);
    assert!((s.residual_bps - s.realised_bps).abs() < 1e-8);
    assert!(s.slices.iter().all(|sl| sl.modelled_bps == 0.0));
}

#[test]
fn a_seller_filling_above_the_arrival_price_shows_a_gain() {
    // Shortfall is signed against the parent, not against the tape: a sell that
    // printed above its decision price earned money and must read negative.
    let fills = vec![
        fill(1, "sell", 1.0, 30030.0, 10.0),
        fill(2, "sell", 1.0, 30060.0, 10.0),
    ];
    let s = shortfall(&fills, "BTC-USD", 2.0, ARRIVAL, 0.0, 0.0).unwrap();
    assert!((s.realised_bps - -15.0).abs() < 1e-8);
    assert!((s.total_bps - -15.0).abs() < 1e-8);
}

#[test]
fn fills_that_mix_sides_are_rejected() {
    // Netting a buy against a sell would produce a signed number with no meaning.
    let fills = vec![
        fill(1, "buy", 1.0, 30030.0, 10.0),
        fill(2, "sell", 1.0, 30060.0, 10.0),
    ];
    let err = shortfall(&fills, "BTC-USD", 2.0, ARRIVAL, 10.0, 0.0).unwrap_err();
    assert!(err.to_string().contains("mix sides"));
}

#[test]
fn bad_inputs_are_rejected() {
    let good = vec![fill(1, "buy", 1.0, 30030.0, 10.0)];
    let msg = |r: anyhow::Result<xexec::shortfall::ShortfallSummary>| r.unwrap_err().to_string();
    assert!(msg(shortfall(&[], "BTC-USD", 1.0, ARRIVAL, 10.0, 0.0)).contains("no fills"));
    assert!(msg(shortfall(&good, "BTC-USD", 0.0, ARRIVAL, 10.0, 0.0)).contains("parent_qty"));
    assert!(msg(shortfall(&good, "BTC-USD", 1.0, -1.0, 10.0, 0.0)).contains("arrival_price"));
    assert!(msg(shortfall(&good, "BTC-USD", 1.0, ARRIVAL, -1.0, 0.0)).contains("coef_bps"));
    // A child cannot take more than its interval held, and the fills cannot
    // total more than the parent: both are reconciliation errors, and reporting
    // a participation or a fill rate above 1 would bury them.
    let over = vec![fill(1, "buy", 11.0, 30030.0, 10.0)];
    assert!(msg(shortfall(&over, "BTC-USD", 20.0, ARRIVAL, 10.0, 0.0))
        .contains("available in its interval"));
    assert!(msg(shortfall(&good, "BTC-USD", 0.5, ARRIVAL, 10.0, 0.0)).contains("against a parent"));
}
