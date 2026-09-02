use xexec::schedule::{optimal_schedule, ScheduleSummary};

/// One parent order of 3.0 against 2.0 of volume per interval, priced with a
/// 25 bps temporary and 5 bps permanent coefficient. Hand-checkable: the TWAP
/// trades 0.5 per slice, i.e. 25% participation.
fn plan(sigma_bps: f64) -> ScheduleSummary {
    optimal_schedule("BTC-USD", 6, 3.0, 2.0, 25.0, 5.0, sigma_bps)
        .expect("the sample schedule must be feasible")
}

#[test]
fn the_schedule_beats_the_twap_it_is_measured_against() {
    // The whole point of the search: with inventory risk priced, some
    // front-load is cheaper than trading uniformly. Values hand-checked.
    let m = plan(8.0);
    assert!((m.urgency - 0.7).abs() < 1e-9);
    assert!((m.impact_bps - 13.98295273).abs() < 1e-8);
    assert!((m.risk_bps - 3.58880415).abs() < 1e-8);
    assert!((m.total_bps - 17.57175688).abs() < 1e-8);
    assert!((m.twap_total_bps - 17.78686714).abs() < 1e-8);
    assert!((m.saving_bps - 0.21511026).abs() < 1e-8);
    // It buys the saving by paying *more* impact to carry *less* risk.
    assert!(m.impact_bps > m.twap_impact_bps);
    assert!(m.risk_bps < m.twap_risk_bps);
}

#[test]
fn with_no_volatility_the_twap_is_optimal() {
    // Front-loading only pays for itself against timing risk. Remove the risk
    // and the concave impact law makes uniform trading strictly cheapest, so
    // the optimiser must return the TWAP, not a spuriously urgent schedule.
    let m = plan(0.0);
    assert_eq!(m.urgency, 0.0);
    assert_eq!(m.risk_bps, 0.0);
    assert_eq!(m.saving_bps, 0.0);
    assert!((m.total_bps - m.twap_total_bps).abs() < 1e-9);
    assert!((m.impact_bps - 13.75).abs() < 1e-8);
}

#[test]
fn urgency_rises_with_volatility() {
    // The economic claim of the module: the more the mid can move against you,
    // the earlier you trade. If this stopped holding, the trade-off would be
    // mispriced.
    let urgencies: Vec<f64> = [0.0, 4.0, 8.0, 60.0]
        .iter()
        .map(|s| plan(*s).urgency)
        .collect();
    assert!(urgencies.windows(2).all(|w| w[0] <= w[1]), "{urgencies:?}");
    assert_eq!(urgencies[0], 0.0);
    assert!(urgencies[3] > urgencies[0]);
}

#[test]
fn the_schedule_trades_the_whole_order_and_front_loads_it() {
    // The weights must be a partition of the parent: every slice smaller than
    // the one before, summing to one, with nothing left outstanding at the end.
    let m = plan(8.0);
    let weights: Vec<f64> = m.schedule.iter().map(|s| s.weight).collect();
    assert!(weights.windows(2).all(|w| w[0] >= w[1]), "{weights:?}");
    assert!((weights.iter().sum::<f64>() - 1.0).abs() < 1e-8);
    let sizes: f64 = m.schedule.iter().map(|s| s.size).sum();
    assert!((sizes - 3.0).abs() < 1e-8);
    assert_eq!(m.schedule[5].remaining, 0.0);
    let idx: Vec<i64> = m.schedule.iter().map(|s| s.slice).collect();
    assert_eq!(idx, vec![0, 1, 2, 3, 4, 5]);
}

#[test]
fn slice_costs_are_the_repo_impact_model_weighted_by_size() {
    // Each slice's contribution must be the same two-term law `impact_curve`
    // prices, scaled by the fraction of the parent it trades -- so the totals
    // are in basis points of the parent, not an average of per-slice bps.
    let m = plan(8.0);
    for s in &m.schedule {
        assert!((s.temp_bps - 25.0 * s.participation.sqrt() * s.weight).abs() < 1e-7);
        assert!((s.perm_bps - 5.0 * s.participation * s.weight).abs() < 1e-7);
    }
    let total: f64 = m.schedule.iter().map(|s| s.temp_bps + s.perm_bps).sum();
    assert!((m.impact_bps - total).abs() < 1e-7);
}

#[test]
fn an_order_too_big_for_the_interval_is_refused_not_clipped() {
    // 3.0 against 1.0 of volume over 2 slices needs 150% participation even
    // when spread evenly. Silently clipping would report a cheap schedule that
    // cannot be traded.
    let err = optimal_schedule("BTC-USD", 2, 3.0, 1.0, 25.0, 5.0, 8.0).unwrap_err();
    assert!(err.to_string().contains("uniform schedule"), "{err}");
}

#[test]
fn bad_inputs_are_rejected() {
    assert!(optimal_schedule("BTC-USD", 0, 3.0, 2.0, 25.0, 5.0, 8.0).is_err());
    assert!(optimal_schedule("BTC-USD", 6, 0.0, 2.0, 25.0, 5.0, 8.0).is_err());
    assert!(optimal_schedule("BTC-USD", 6, 3.0, f64::NAN, 25.0, 5.0, 8.0).is_err());
    // A negative coefficient would make impact a reward and the search would
    // run to the most urgent schedule on the grid.
    assert!(optimal_schedule("BTC-USD", 6, 3.0, 2.0, -1.0, 5.0, 8.0).is_err());
    assert!(optimal_schedule("BTC-USD", 6, 3.0, 2.0, 25.0, 5.0, -1.0).is_err());
}
