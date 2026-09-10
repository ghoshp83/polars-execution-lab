use xexec::model::Tick;
use xexec::pov::{pov_backtest, pov_schedule};

/// One second per bucket.
const BUCKET: i64 = 1_000_000_000;
/// An hour later, so the two captures only line up by time into the session.
const LATER: i64 = 3_600 * BUCKET;

fn tick(ts_ns: i64, price: f64, size: f64) -> Tick {
    Tick {
        ts_ns,
        product: "BTC-USD".to_string(),
        price,
        size,
        side: "buy".to_string(),
        trade_id: ts_ns,
    }
}

/// The capture the plan is built on: 1.0, 6.0 and 3.0 of volume.
fn lumpy() -> Vec<Tick> {
    vec![
        tick(0, 100.0, 1.0),
        tick(BUCKET, 102.0, 4.0),
        tick(BUCKET + 500_000_000, 104.0, 2.0),
        tick(2 * BUCKET, 110.0, 3.0),
    ]
}

/// The session the plan meets: the same three buckets an hour later, with the
/// busy and the quiet bucket swapped -- 6.0, 1.0 and 3.0 of volume.
fn reshaped() -> Vec<Tick> {
    vec![
        tick(LATER, 200.0, 6.0),
        tick(LATER + BUCKET, 190.0, 1.0),
        tick(LATER + 2 * BUCKET, 210.0, 3.0),
    ]
}

#[test]
fn replaying_a_capture_against_itself_is_the_in_sample_plan() {
    let ticks = lumpy();
    let bt = pov_backtest(&ticks, &ticks, BUCKET, 1.0, 0.5, 10.0, 2.0).unwrap();
    let plan = pov_schedule(&ticks, "BTC-USD", BUCKET, 1.0, 0.5, 10.0, 2.0).unwrap();
    // With no forecast error there is nothing to pay for: the backtest has to
    // collapse to the plan, or it is measuring something other than the forecast.
    assert_eq!(bt.profile_distance, 0.0);
    assert_eq!(bt.tracking_bps, 0.0);
    assert_eq!(bt.forecast_cost_bps, 0.0);
    assert_eq!(bt.max_participation, plan.participation);
    assert!((bt.impact_bps - plan.pov_impact_bps).abs() < 1e-7);
}

#[test]
fn a_mis_forecast_profile_costs_more_than_the_oracle() {
    let bt = pov_backtest(&lumpy(), &reshaped(), BUCKET, 1.0, 1.0, 10.0, 2.0).unwrap();
    // The plan puts 0.6 of the parent into the bucket that turned out to trade
    // 1.0, and the square-root law charges for it.
    assert!(bt.impact_bps > bt.oracle_impact_bps);
    assert!(bt.forecast_cost_bps > 1.0);
}

#[test]
fn participation_is_no_longer_constant_out_of_sample() {
    let bt = pov_backtest(&lumpy(), &reshaped(), BUCKET, 1.0, 1.0, 10.0, 0.0).unwrap();
    let participation: Vec<f64> = bt.schedule.iter().map(|s| s.participation).collect();
    // In sample every bucket would read 0.1; the property does not survive.
    assert_eq!(participation, vec![0.01666667, 0.6, 0.1]);
    assert_eq!(bt.max_participation, 0.6);
    assert_eq!(bt.oracle_participation, 0.1);
}

#[test]
fn the_forecast_can_breach_a_cap_the_oracle_respects() {
    let bt = pov_backtest(&lumpy(), &reshaped(), BUCKET, 1.0, 0.5, 10.0, 0.0).unwrap();
    // Reported, not refused: a breach is the most important thing a forecast
    // error can do, so the backtest must be able to say it happened.
    assert!(!bt.feasible);
    assert!(bt.oracle_feasible);
}

#[test]
fn the_price_is_the_plan_weighted_mean_of_the_execution_vwaps() {
    let bt = pov_backtest(&lumpy(), &reshaped(), BUCKET, 1.0, 1.0, 10.0, 0.0).unwrap();
    // 0.1 * 200 + 0.6 * 190 + 0.3 * 210, against a session VWAP of 202.
    assert_eq!(bt.price, 197.0);
    assert_eq!(bt.session_vwap, 202.0);
    assert!((bt.tracking_bps - -247.52475248).abs() < 1e-6);
}

#[test]
fn the_profile_distance_measures_how_far_the_shape_moved() {
    let bt = pov_backtest(&lumpy(), &reshaped(), BUCKET, 1.0, 1.0, 10.0, 0.0).unwrap();
    // Shares 0.1 / 0.6 / 0.3 against 0.6 / 0.1 / 0.3: half of 0.5 + 0.5 + 0.
    assert_eq!(bt.profile_distance, 0.5);
}

#[test]
fn captures_are_aligned_by_time_into_the_session() {
    let bt = pov_backtest(&lumpy(), &reshaped(), BUCKET, 1.0, 1.0, 10.0, 0.0).unwrap();
    let slots: Vec<i64> = bt.schedule.iter().map(|s| s.slot).collect();
    assert_eq!(bt.buckets, 3);
    assert_eq!(slots, vec![0, 1, 2]);
}

#[test]
fn captures_covering_different_buckets_are_refused() {
    let gappy = vec![
        tick(LATER, 200.0, 6.0),
        tick(LATER + 2 * BUCKET, 210.0, 3.0),
    ];
    let err = pov_backtest(&lumpy(), &gappy, BUCKET, 1.0, 1.0, 10.0, 0.0).unwrap_err();
    assert!(err.to_string().contains("cover different buckets"));
}

#[test]
fn captures_of_different_products_are_refused() {
    let mut other = reshaped();
    for t in &mut other {
        t.product = "ETH-USD".to_string();
    }
    let err = pov_backtest(&lumpy(), &other, BUCKET, 1.0, 1.0, 10.0, 0.0).unwrap_err();
    assert!(err.to_string().contains("the plan capture is BTC-USD"));
}

#[test]
fn an_empty_or_volumeless_capture_is_refused_by_name() {
    let err = pov_backtest(&[], &reshaped(), BUCKET, 1.0, 1.0, 10.0, 0.0).unwrap_err();
    assert_eq!(err.to_string(), "plan capture: no ticks");
    let err = pov_backtest(&lumpy(), &[], BUCKET, 1.0, 1.0, 10.0, 0.0).unwrap_err();
    assert_eq!(err.to_string(), "execution capture: no ticks");
    let flat = vec![tick(LATER, 200.0, 0.0)];
    let err = pov_backtest(&lumpy(), &flat, BUCKET, 1.0, 1.0, 10.0, 0.0).unwrap_err();
    assert_eq!(err.to_string(), "execution capture: zero traded volume");
}

#[test]
fn a_cap_outside_the_unit_interval_is_refused() {
    for cap in [0.0, -0.1, 1.5] {
        let err = pov_backtest(&lumpy(), &reshaped(), BUCKET, 1.0, cap, 10.0, 0.0).unwrap_err();
        assert!(err.to_string().starts_with("cap must be in (0, 1]"));
    }
}
