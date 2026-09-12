use xexec::model::Tick;
use xexec::pov::{pov_backtest, pov_forecast};

/// One second per bucket.
const BUCKET: i64 = 1_000_000_000;
/// An hour between sessions, so captures only line up by time into the session.
const HOUR: i64 = 3_600 * BUCKET;
/// Pool every session equally: the forecast v0.19 made before the decay existed.
const FLAT: f64 = 0.0;

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
    let fc = pov_forecast(&[lumpy()], &reshaped(), BUCKET, 1.0, 1.0, 10.0, 2.0, FLAT).unwrap();
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
    assert_eq!(fc.weights, vec![1.0]);
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
        FLAT,
    )
    .unwrap();
    // The session looks like the average of its history, so the average is the
    // oracle and the most recent session alone is not.
    assert_eq!(fc.sessions, 2);
    assert_eq!(fc.weights, vec![0.5, 0.5]);
    assert_eq!(fc.forecast.profile_distance, 0.0);
    assert_eq!(fc.forecast.forecast_cost_bps, 0.0);
    assert!(fc.naive.forecast_cost_bps > 0.0);
    assert!(fc.improvement_bps > 0.0);
}

#[test]
fn a_session_does_not_outvote_the_others_by_trading_more() {
    let heavy = session(HOUR, [60.0, 10.0, 30.0]);
    let fc = pov_forecast(
        &[lumpy(), heavy],
        &blended(),
        BUCKET,
        1.0,
        1.0,
        10.0,
        0.0,
        FLAT,
    )
    .unwrap();
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
        FLAT,
    )
    .unwrap();
    // When the session repeats the last one exactly, pooling only adds error.
    // The improvement is reported, not assumed, so it can go negative.
    assert_eq!(fc.naive.forecast_cost_bps, 0.0);
    assert!(fc.forecast.forecast_cost_bps > 0.0);
    assert!(fc.improvement_bps < 0.0);
}

#[test]
fn a_half_life_of_one_session_halves_the_weight_each_step_back() {
    let fc = pov_forecast(
        &[lumpy(), reshaped(), blended()],
        &blended(),
        BUCKET,
        1.0,
        1.0,
        10.0,
        0.0,
        1.0,
    )
    .unwrap();
    // Raw weights 0.25 / 0.5 / 1.0 over 1.75: the newest session weighs four
    // times the oldest, and the weights are reported normalised.
    assert_eq!(fc.half_life, 1.0);
    assert_eq!(fc.weights, vec![0.14285714, 0.28571429, 0.57142857]);
    let total: f64 = fc.weights.iter().sum();
    assert!((total - 1.0).abs() < 1e-7);
}

#[test]
fn a_short_half_life_converges_on_the_naive_forecast() {
    // A half-life of a hundredth of a session leaves the older capture
    // 2^-100 of the weight, so the pooled plan *is* the last session.
    let fc = pov_forecast(
        &[lumpy(), reshaped()],
        &blended(),
        BUCKET,
        1.0,
        1.0,
        10.0,
        0.0,
        0.01,
    )
    .unwrap();
    assert_eq!(fc.weights, vec![0.0, 1.0]);
    assert_eq!(fc.forecast.impact_bps, fc.naive.impact_bps);
    assert_eq!(fc.forecast.profile_distance, fc.naive.profile_distance);
    assert_eq!(fc.improvement_bps, 0.0);
}

#[test]
fn recency_weighting_wins_when_the_profile_has_drifted() {
    // The old session is the odd one out and the market has settled into the
    // recent shape: leaning on the recent sessions beats pooling all three.
    let history = [lumpy(), reshaped(), reshaped()];
    let exec = session(3 * HOUR, [6.0, 1.0, 3.0]);
    let flat = pov_forecast(&history, &exec, BUCKET, 1.0, 1.0, 10.0, 0.0, FLAT).unwrap();
    let decayed = pov_forecast(&history, &exec, BUCKET, 1.0, 1.0, 10.0, 0.0, 0.5).unwrap();
    assert!(decayed.forecast.forecast_cost_bps < flat.forecast.forecast_cost_bps);
    assert!(decayed.forecast.profile_distance < flat.forecast.profile_distance);
    // The naive baseline is the same session either way, so a smaller cost
    // against the oracle is a larger improvement over it.
    assert!(decayed.improvement_bps > flat.improvement_bps);
}

#[test]
fn a_negative_half_life_is_refused() {
    let err = pov_forecast(&[lumpy()], &blended(), BUCKET, 1.0, 1.0, 10.0, 0.0, -1.0).unwrap_err();
    assert_eq!(
        err.to_string(),
        "half_life must be a non-negative finite number, got -1"
    );
}

#[test]
fn no_history_is_refused() {
    let err = pov_forecast(&[], &blended(), BUCKET, 1.0, 1.0, 10.0, 0.0, FLAT).unwrap_err();
    assert_eq!(err.to_string(), "need at least one history capture");
}

#[test]
fn a_bad_history_capture_is_named_by_position() {
    let err = pov_forecast(
        &[lumpy(), vec![]],
        &blended(),
        BUCKET,
        1.0,
        1.0,
        10.0,
        0.0,
        FLAT,
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "history capture 2: no ticks");
}

#[test]
fn history_covering_different_buckets_is_refused() {
    let gappy = vec![tick(HOUR, 100.0, 6.0), tick(HOUR + 2 * BUCKET, 102.0, 3.0)];
    let err = pov_forecast(&[gappy], &blended(), BUCKET, 1.0, 1.0, 10.0, 0.0, FLAT).unwrap_err();
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
    let err = pov_forecast(&[other], &blended(), BUCKET, 1.0, 1.0, 10.0, 0.0, FLAT).unwrap_err();
    assert_eq!(
        err.to_string(),
        "history capture 1 is ETH-USD but the execution capture is BTC-USD"
    );
}
