use xexec::capsweep::cap_sweep;
use xexec::model::Tick;
use xexec::pov::pov_forecast;

/// One second per bucket.
const BUCKET: i64 = 1_000_000_000;
/// An hour between sessions, so captures only line up by time into the session.
const HOUR: i64 = 3_600 * BUCKET;
/// Pool every session equally.
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

/// Half of each: 0.35 / 0.35 / 0.3.
fn blended() -> Vec<Tick> {
    session(2 * HOUR, [3.5, 3.5, 3.0])
}

/// A session whose volume collapses after the first bucket: only the tail binds.
fn thin_tail() -> Vec<Tick> {
    session(3 * HOUR, [10.0, 1.0, 0.6])
}

/// Two even buckets and a thin one, so a tight cap binds in the *even* buckets
/// too -- the shape that produces a genuine trade-off rather than a dominance.
fn pinched() -> Vec<Tick> {
    session(4 * HOUR, [1.0, 1.0, 0.02])
}

/// The sweep is a re-run, not a new calculation: every point must equal what
/// `pov_forecast` reports at that cap on its own. If these ever drift apart the
/// sweep has started computing its own answer, which is the one thing it must
/// never do.
#[test]
fn every_point_is_the_forecast_at_that_cap() {
    let history = [blended(), lumpy()];
    let grid = [0.1, 0.2, 0.3];
    let sweep = cap_sweep(&history, &thin_tail(), BUCKET, 1.0, &grid, 10.0, 2.0, FLAT).unwrap();
    assert_eq!(sweep.points.len(), grid.len());
    for (point, cap) in sweep.points.iter().zip(grid.iter()) {
        let fc = pov_forecast(
            &history,
            &thin_tail(),
            BUCKET,
            1.0,
            *cap,
            10.0,
            2.0,
            FLAT,
            None,
        )
        .unwrap();
        let gain = &fc.capped_improvement.capped;
        assert_eq!(point.cap, *cap);
        assert_eq!(point.improvement_bps, gain.improvement_bps);
        assert_eq!(point.shortfall_qty, gain.shortfall_qty);
        assert_eq!(point.like_for_like, gain.like_for_like);
        assert_eq!(point.breakeven_bps, gain.breakeven_bps);
        assert_eq!(
            point.forecast_unfilled_qty,
            fc.forecast_capped.capped.unfilled_qty
        );
        assert_eq!(
            point.naive_unfilled_qty,
            fc.naive_capped.capped.unfilled_qty
        );
    }
}

/// The three counts are a partition, and the report says so. A point cannot be
/// both like-for-like and dominated, and one that has a rate is neither -- so
/// the counts must sum to the grid. A reader who adds them up and gets more
/// than the grid is being told the same cap twice.
#[test]
fn the_three_counts_partition_the_grid() {
    for exec in [thin_tail(), pinched()] {
        let sweep = cap_sweep(
            &[blended(), lumpy()],
            &exec,
            BUCKET,
            1.0,
            &[0.1, 0.2, 0.3, 0.4, 0.5],
            10.0,
            2.0,
            FLAT,
        )
        .unwrap();
        assert_eq!(
            sweep.like_for_like_points + sweep.dominated_points + sweep.traded_off_points,
            sweep.points.len()
        );
        for p in &sweep.points {
            let flags = [p.like_for_like, p.dominated, p.breakeven_bps.is_some()];
            assert_eq!(flags.iter().filter(|f| **f).count(), 1, "cap {}", p.cap);
        }
    }
}

/// **The dominance finding, stated as a property rather than a comment.**
///
/// Under the forward carry a plan that gets more away early both misses less
/// *and* pays less per unit filled: the extra fill lands in the
/// low-participation slots and raises their weight in the average. So in a
/// session where only the thin tail binds, one plan dominates outright at every
/// cap and there is no rate to price -- `breakeven_bps` is `null` by
/// construction, not by accident.
#[test]
fn where_only_the_tail_binds_no_cap_needs_a_rate() {
    let sweep = cap_sweep(
        &[blended(), lumpy()],
        &thin_tail(),
        BUCKET,
        1.0,
        &[0.1, 0.2, 0.3, 0.4, 0.5],
        10.0,
        2.0,
        FLAT,
    )
    .unwrap();
    // The cap really does bind somewhere, or the test proves nothing.
    assert!(sweep.points.iter().any(|p| !p.like_for_like));
    assert_eq!(sweep.traded_off_points, 0);
    assert_eq!(sweep.breakeven_min_bps, None);
    assert_eq!(sweep.breakeven_max_bps, None);
    for p in &sweep.points {
        assert!(p.breakeven_bps.is_none(), "cap {} priced a rate", p.cap);
    }
}

/// The contrast that makes the previous test a finding and not a bug: bind the
/// cap in the *even* buckets as well and a real trade-off appears, with a rate
/// attached to it.
#[test]
fn a_cap_that_bites_in_the_even_buckets_does_need_a_rate() {
    let sweep = cap_sweep(
        &[blended(), lumpy()],
        &pinched(),
        BUCKET,
        1.0,
        &[0.2, 0.3, 0.4],
        10.0,
        2.0,
        FLAT,
    )
    .unwrap();
    assert!(
        sweep.traded_off_points > 0,
        "no cap in the grid produced a trade-off"
    );
    let rates: Vec<f64> = sweep
        .points
        .iter()
        .filter_map(|p| p.breakeven_bps)
        .collect();
    let lo = sweep.breakeven_min_bps.unwrap();
    let hi = sweep.breakeven_max_bps.unwrap();
    for r in rates {
        assert!(r > 0.0, "a breakeven rate must be positive, got {r}");
        assert!(lo <= r && r <= hi, "{r} outside [{lo}, {hi}]");
    }
}

/// `like_for_like_from_cap` is a threshold, so it must be read from the top of
/// the grid down. A cap slack enough that neither plan misses anything gives the
/// whole grid back.
#[test]
fn a_grid_the_session_can_absorb_is_like_for_like_throughout() {
    let sweep = cap_sweep(
        &[blended(), lumpy()],
        &lumpy(),
        BUCKET,
        0.1,
        &[0.5, 0.75, 1.0],
        10.0,
        2.0,
        FLAT,
    )
    .unwrap();
    assert_eq!(sweep.like_for_like_points, sweep.points.len());
    assert_eq!(sweep.dominated_points, 0);
    assert_eq!(sweep.traded_off_points, 0);
    assert_eq!(sweep.like_for_like_from_cap, Some(0.5));
}

/// One point is not a sweep.
#[test]
fn a_single_cap_is_refused() {
    let err = cap_sweep(
        &[blended(), lumpy()],
        &thin_tail(),
        BUCKET,
        1.0,
        &[0.25],
        10.0,
        2.0,
        FLAT,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("at least 2 points"), "{err}");
}

/// The grid is read as an ordering, so it has to be one.
#[test]
fn a_grid_that_does_not_increase_is_refused() {
    let err = cap_sweep(
        &[blended(), lumpy()],
        &thin_tail(),
        BUCKET,
        1.0,
        &[0.3, 0.2],
        10.0,
        2.0,
        FLAT,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("strictly increasing"), "{err}");
}

/// A cap is a share of volume; outside `(0, 1]` it is not one.
#[test]
fn a_cap_outside_the_unit_interval_is_refused() {
    for grid in [[0.0, 0.5], [0.5, 1.5]] {
        let err = cap_sweep(
            &[blended(), lumpy()],
            &thin_tail(),
            BUCKET,
            1.0,
            &grid,
            10.0,
            2.0,
            FLAT,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("in (0, 1]"), "{err}");
    }
}
