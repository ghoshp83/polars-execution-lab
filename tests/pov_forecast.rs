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

#[test]
fn a_single_history_session_capped_is_the_backtest_capped() {
    let fc = pov_forecast(&[lumpy()], &reshaped(), BUCKET, 1.0, 0.5, 10.0, 2.0, FLAT).unwrap();
    let bt = pov_backtest(&lumpy(), &reshaped(), BUCKET, 1.0, 0.5, 10.0, 2.0).unwrap();
    // The collapse has to hold under the cap too, or the two commands would be
    // charging the same allocation differently the moment the cap binds.
    assert_eq!(fc.forecast_capped.capped.sizes, bt.capped.sizes);
    assert_eq!(fc.forecast_capped.capped.impact_bps, bt.capped.impact_bps);
    assert_eq!(fc.forecast_capped.spread.sizes, bt.spread.sizes);
    assert_eq!(fc.forecast_capped.spread.impact_bps, bt.spread.impact_bps);
    // One session is its own naive baseline, capped executions included.
    assert_eq!(
        fc.naive_capped.capped.sizes,
        fc.forecast_capped.capped.sizes
    );
}

#[test]
fn the_reshape_makes_the_shortfall_a_property_of_the_session() {
    // The session traded 10 and the cap admits half of it, so the whole parent
    // fits. Water-filling therefore completes whatever shape the forecast has:
    // two forecasts that disagree about every slot still miss nothing.
    let fc = pov_forecast(
        &[blended(), lumpy()],
        &reshaped(),
        BUCKET,
        1.0,
        0.5,
        10.0,
        2.0,
        FLAT,
    )
    .unwrap();
    assert!(fc.forecast_capped.spread.completed);
    assert!(fc.naive_capped.spread.completed);
    assert_eq!(fc.forecast_capped.spread.filled_qty, 1.0);
    assert_eq!(fc.naive_capped.spread.filled_qty, 1.0);
    assert_ne!(
        fc.forecast_capped.spread.sizes,
        fc.naive_capped.spread.sizes
    );
}

#[test]
fn a_cap_the_session_cannot_fill_misses_the_same_under_either_forecast() {
    // cap * exec_volume is 0.5 against a parent of 1.0, so no allocation can
    // fill more than half -- and the reshape misses exactly the other half.
    let fc = pov_forecast(
        &[blended(), lumpy()],
        &reshaped(),
        BUCKET,
        1.0,
        0.05,
        10.0,
        2.0,
        FLAT,
    )
    .unwrap();
    assert_eq!(fc.forecast_capped.spread.unfilled_qty, 0.5);
    assert_eq!(fc.naive_capped.spread.unfilled_qty, 0.5);
    assert_eq!(
        fc.forecast_capped.spread.sizes,
        fc.naive_capped.spread.sizes
    );
}

#[test]
fn deferring_still_charges_the_forecast_for_its_shape() {
    // The reshape hides the difference between the two forecasts; the forward
    // carry does not. The naive plan puts 0.6 into the thinnest slot and cannot
    // recover it before the close, while the pooled one clears.
    let fc = pov_forecast(
        &[blended(), lumpy()],
        &reshaped(),
        BUCKET,
        1.0,
        0.2,
        10.0,
        2.0,
        FLAT,
    )
    .unwrap();
    assert_eq!(fc.naive_capped.capped.unfilled_qty, 0.1);
    assert!(!fc.naive_capped.capped.completed);
    assert_eq!(fc.forecast_capped.capped.unfilled_qty, 0.0);
    assert!(fc.forecast_capped.capped.completed);
    // Deferring can only ever miss more than re-shaping, which is the bracket.
    assert!(fc.naive_capped.capped.unfilled_qty >= fc.naive_capped.spread.unfilled_qty);
    assert!(fc.forecast_capped.capped.unfilled_qty >= fc.forecast_capped.spread.unfilled_qty);
}

#[test]
fn filled_impact_prices_the_quantity_that_actually_traded() {
    // `impact_bps` is per unit of parent, so an execution the cap left short
    // reports a smaller number for having traded less. `filled_impact_bps`
    // divides by what filled instead, so it does not fall with the shortfall.
    let fc = pov_forecast(
        &[blended(), lumpy()],
        &reshaped(),
        BUCKET,
        1.0,
        0.2,
        10.0,
        2.0,
        FLAT,
    )
    .unwrap();
    // The pooled plan cleared, so there is nothing to re-base: the two agree.
    assert!(fc.forecast_capped.capped.completed);
    assert_eq!(
        fc.forecast_capped.capped.filled_impact_bps,
        fc.forecast_capped.capped.impact_bps
    );
    // The naive plan missed 0.1 of the parent, and that is exactly what made
    // its `impact_bps` the smaller of the two numbers.
    assert!(!fc.naive_capped.capped.completed);
    assert!(fc.naive_capped.capped.filled_impact_bps > fc.naive_capped.capped.impact_bps);
}

#[test]
fn the_capped_improvement_reports_what_the_cheaper_plan_did_not_fill() {
    // Under the forward carry the naive plan misses 0.1 and the pooled one
    // clears. Netting that into a single number would let a plan look better
    // for trading less, so the shortfall is reported beside the improvement.
    let fc = pov_forecast(
        &[blended(), lumpy()],
        &reshaped(),
        BUCKET,
        1.0,
        0.2,
        10.0,
        2.0,
        FLAT,
    )
    .unwrap();
    let gain = &fc.capped_improvement.capped;
    // Negative: the pooled plan missed *less* than the naive one.
    assert_eq!(gain.shortfall_qty, -0.1);
    assert!(!gain.like_for_like);
    // The reshape fills the whole parent for either forecast, so there the same
    // comparison is like for like and the shortfall term vanishes.
    let spread = &fc.capped_improvement.spread;
    assert_eq!(spread.shortfall_qty, 0.0);
    assert!(spread.like_for_like);
}

#[test]
fn a_slack_cap_leaves_the_capped_improvement_the_uncapped_one() {
    // With the cap slack neither execution binds, both fill the whole parent,
    // and `filled_impact_bps` is just the plan's own impact. The capped
    // question then has to give back the uncapped answer.
    let fc = pov_forecast(
        &[blended(), lumpy()],
        &reshaped(),
        BUCKET,
        1.0,
        1.0,
        10.0,
        2.0,
        FLAT,
    )
    .unwrap();
    assert_eq!(fc.forecast_capped.capped.capped_slots, 0);
    assert_eq!(fc.naive_capped.capped.capped_slots, 0);
    assert_eq!(
        fc.forecast_capped.capped.filled_impact_bps,
        fc.forecast.impact_bps
    );
    assert_eq!(
        fc.naive_capped.capped.filled_impact_bps,
        fc.naive.impact_bps
    );
    assert!(fc.capped_improvement.capped.like_for_like);
    // Equal up to the rounding, which lands differently on a difference of
    // rounded numbers than on a rounded difference.
    assert!((fc.capped_improvement.capped.improvement_bps - fc.improvement_bps).abs() <= 1e-8);
}

#[test]
fn a_session_too_thin_for_the_parent_leaves_the_forecasts_nothing_to_win() {
    // `cap * exec_volume` is 0.5 against a parent of 1.0, so the reshape fills
    // exactly half whatever the forecast said. Both plans miss the same and
    // trade the same, so pooling is worth nothing under the spread.
    let fc = pov_forecast(
        &[blended(), lumpy()],
        &reshaped(),
        BUCKET,
        1.0,
        0.05,
        10.0,
        2.0,
        FLAT,
    )
    .unwrap();
    let spread = &fc.capped_improvement.spread;
    assert_eq!(spread.shortfall_qty, 0.0);
    assert!(spread.like_for_like);
    assert_eq!(spread.improvement_bps, 0.0);
}
