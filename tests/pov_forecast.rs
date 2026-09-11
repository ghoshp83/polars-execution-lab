use xexec::model::Tick;
use xexec::pov::{pov_backtest, pov_forecast};

/// One second per bucket.
const BUCKET: i64 = 1_000_000_000;
/// An hour between sessions, so captures only line up by time into the session.
const HOUR: i64 = 3_600 * BUCKET;

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

/// Three one-tick buckets starting `start` with the given volumes.
fn session(start: i64, volumes: [f64; 3]) -> Vec<Tick> {
    volumes
        .iter()
        .enumerate()
        .map(|(i, v)| tick(start + i as i64 * BUCKET, 100.0 + i as f64, *v))
        .collect()
}

/// Shares 0.1 / 0.6 / 0.3.
fn lumpy() -> Vec<Tick> {
    session(0, [1.0, 6.0, 3.0])
}

/// The opposite shape: 0.6 / 0.1 / 0.3.
fn reshaped() -> Vec<Tick> {
    session(HOUR, [6.0, 1.0, 3.0])
}

/// Half of each: 0.35 / 0.35 / 0.3.
fn blended() -> Vec<Tick> {
    session(2 * HOUR, [3.5, 3.5, 3.0])
}

#[test]
fn one_history_session_is_the_backtest() {
    let fc = pov_forecast(&[lumpy()], &reshaped(), BUCKET, 1.0, 1.0, 10.0, 2.0).unwrap();
    let bt = pov_backtest(&lumpy(), &reshaped(), BUCKET, 1.0, 1.0, 10.0, 2.0).unwrap();
    // An average of one session is that session, so the pooled forecast must
    // collapse to v0.18's backtest -- and to its own naive baseline.
    assert_eq!(fc.forecast.impact_bps, bt.impact_bps);
    assert_eq!(fc.forecast.forecast_cost_bps, bt.forecast_cost_bps);
    assert_eq!(fc.forecast.profile_distance, bt.profile_distance);
    assert_eq!(fc.forecast.price, bt.price);
    assert_eq!(fc.oracle_impact_bps, bt.oracle_impact_bps);
    assert_eq!(fc.naive.impact_bps, fc.forecast.impact_bps);
    assert_eq!(fc.improvement_bps, 0.0);
}

#[test]
fn averaging_opposite_sessions_recovers_a_blended_session() {
    let fc = pov_forecast(
        &[lumpy(), reshaped()],
        &blended(),
        BUCKET,
        1.0,
        1.0,
        10.0,
        2.0,
    )
    .unwrap();
    // The session looks like the average of its history, so the average is the
    // oracle and the most recent session alone is not.
    assert_eq!(fc.sessions, 2);
    assert_eq!(fc.forecast.profile_distance, 0.0);
    assert_eq!(fc.forecast.forecast_cost_bps, 0.0);
    assert!(fc.naive.forecast_cost_bps > 0.0);
    assert!(fc.improvement_bps > 0.0);
}

#[test]
fn a_session_does_not_outvote_the_others_by_trading_more() {
    let heavy = session(HOUR, [60.0, 10.0, 30.0]);
    let fc = pov_forecast(&[lumpy(), heavy], &blended(), BUCKET, 1.0, 1.0, 10.0, 0.0).unwrap();
    // Ten times the volume, the same shape as `reshaped`: shares are averaged,
    // not volumes, so the forecast is still the even blend.
    let shares: Vec<f64> = fc.schedule.iter().map(|s| s.plan_share).collect();
    assert_eq!(shares, vec![0.35, 0.35, 0.3]);
}

#[test]
fn the_average_can_lose_to_the_most_recent_session() {
    let fc = pov_forecast(
        &[lumpy(), reshaped()],
        &reshaped(),
        BUCKET,
        1.0,
        1.0,
        10.0,
        0.0,
    )
    .unwrap();
    // When the session repeats the last one exactly, pooling only adds error.
    // The improvement is reported, not assumed, so it can go negative.
    assert_eq!(fc.naive.forecast_cost_bps, 0.0);
    assert!(fc.forecast.forecast_cost_bps > 0.0);
    assert!(fc.improvement_bps < 0.0);
}

#[test]
fn no_history_is_refused() {
    let err = pov_forecast(&[], &blended(), BUCKET, 1.0, 1.0, 10.0, 0.0).unwrap_err();
    assert_eq!(err.to_string(), "need at least one history capture");
}

#[test]
fn a_bad_history_capture_is_named_by_position() {
    let err =
        pov_forecast(&[lumpy(), vec![]], &blended(), BUCKET, 1.0, 1.0, 10.0, 0.0).unwrap_err();
    assert_eq!(err.to_string(), "history capture 2: no ticks");
}

#[test]
fn history_covering_different_buckets_is_refused() {
    let gappy = vec![tick(HOUR, 100.0, 6.0), tick(HOUR + 2 * BUCKET, 102.0, 3.0)];
    let err = pov_forecast(&[gappy], &blended(), BUCKET, 1.0, 1.0, 10.0, 0.0).unwrap_err();
    assert!(err
        .to_string()
        .starts_with("history capture 1 covers different buckets"));
}

#[test]
fn history_of_another_product_is_refused() {
    let mut other = lumpy();
    for t in &mut other {
        t.product = "ETH-USD".to_string();
    }
    let err = pov_forecast(&[other], &blended(), BUCKET, 1.0, 1.0, 10.0, 0.0).unwrap_err();
    assert_eq!(
        err.to_string(),
        "history capture 1 is ETH-USD but the execution capture is BTC-USD"
    );
}
