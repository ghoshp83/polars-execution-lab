use xexec::hlsweep::hl_sweep;
use xexec::model::Tick;
use xexec::pov::pov_forecast;

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
/// `pov_forecast` reports at that half-life on its own, including the weight
/// vector the pooling produced.
#[test]
fn every_point_is_the_forecast_at_that_half_life() {
    let history = [blended(), lumpy()];
    let grid = [0.5, 1.0, 2.0];
    let sweep = hl_sweep(&history, &thin_tail(), BUCKET, 1.0, 0.3, &grid, 10.0, 2.0).unwrap();
    assert_eq!(sweep.points.len(), grid.len());
    for (point, hl) in sweep.points.iter().zip(grid.iter()) {
        let fc = pov_forecast(
            &history,
            &thin_tail(),
            BUCKET,
            1.0,
            0.3,
            10.0,
            2.0,
            *hl,
            None,
        )
        .unwrap();
        let gain = &fc.capped_improvement.capped;
        assert_eq!(point.half_life, *hl);
        assert_eq!(point.newest_weight, *fc.weights.last().unwrap());
        assert_eq!(point.improvement_bps, gain.improvement_bps);
        assert_eq!(point.shortfall_qty, gain.shortfall_qty);
        assert_eq!(point.like_for_like, gain.like_for_like);
        assert_eq!(point.breakeven_bps, gain.breakeven_bps);
    }
}

/// **The left-hand limit, pinned.** A half-life short enough puts essentially
/// all the weight on the most recent session, which is precisely the naive plan
/// the forecast is scored against — so the two plans coincide and there is
/// nothing left to improve. A sweep that does not bottom out here has not been
/// run short enough to see its own floor, and the doc comment says so; this is
/// the test that keeps that claim true.
#[test]
fn a_short_half_life_is_the_naive_plan() {
    let sweep = hl_sweep(
        &[blended(), lumpy()],
        &thin_tail(),
        BUCKET,
        1.0,
        0.3,
        &[0.01, 0.02],
        10.0,
        2.0,
    )
    .unwrap();
    for p in &sweep.points {
        assert_eq!(p.newest_weight, 1.0, "half-life {}", p.half_life);
        assert_eq!(p.improvement_bps, 0.0, "half-life {}", p.half_life);
        assert!(p.like_for_like, "half-life {}", p.half_life);
    }
    assert_eq!(sweep.improvement_span_bps, 0.0);
    assert!(sweep.sign_stable);
}

/// **The right-hand limit, pinned.** As the half-life grows the decay flattens
/// and the pooling converges on the equal-weight pool that `half_life = 0`
/// computes directly. `flat_improvement_bps` is reported so a reader can see
/// which limit the grid is heading for; this asserts it really is the limit,
/// and that the grid approaches it rather than wandering.
#[test]
fn a_long_half_life_converges_on_the_flat_pool() {
    let sweep = hl_sweep(
        &[blended(), lumpy()],
        &thin_tail(),
        BUCKET,
        1.0,
        0.3,
        &[1.0, 1e6],
        10.0,
        2.0,
    )
    .unwrap();
    let flat = sweep.flat_improvement_bps;
    let near = (sweep.points[0].improvement_bps - flat).abs();
    let far = (sweep.points[1].improvement_bps - flat).abs();
    assert!(
        far <= near,
        "the grid moved away from the flat pool: {far} > {near}"
    );
    assert!(
        far < 1e-6,
        "1e6 sessions of half-life is still {far} from flat"
    );
}

/// The three summary figures are read off the points, so they have to agree with
/// them — a reader who compares the span against the column of `improvement_bps`
/// must not find a different range there.
#[test]
fn the_span_and_the_extremes_describe_the_points() {
    let sweep = hl_sweep(
        &[blended(), lumpy()],
        &thin_tail(),
        BUCKET,
        1.0,
        0.3,
        &[0.25, 0.5, 1.0, 2.0, 4.0],
        10.0,
        2.0,
    )
    .unwrap();
    let gains: Vec<f64> = sweep.points.iter().map(|p| p.improvement_bps).collect();
    let lo = gains.iter().cloned().fold(f64::INFINITY, f64::min);
    let hi = gains.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert_eq!(sweep.improvement_min_bps, lo);
    assert_eq!(sweep.improvement_max_bps, hi);
    assert_eq!(sweep.improvement_span_bps, hi - lo);
    // `best`/`worst` name grid points, not values off the end of it.
    let at = |hl: f64| {
        sweep
            .points
            .iter()
            .find(|p| p.half_life == hl)
            .unwrap_or_else(|| panic!("{hl} is not a grid point"))
    };
    assert_eq!(at(sweep.best_half_life).improvement_bps, hi);
    assert_eq!(at(sweep.worst_half_life).improvement_bps, lo);
}

/// **The finding the sweep exists for.** Run wide enough and the same sessions
/// say pooling cost money at one half-life and paid at another. `sign_stable`
/// is false there, and it must be: a desk that read a single run would have
/// taken the sign of an unfitted guess for a property of the market.
#[test]
fn a_sign_that_flips_across_the_grid_is_reported_unstable() {
    let sweep = hl_sweep(
        &[blended(), thin_tail()],
        &pinched(),
        BUCKET,
        0.6,
        0.5,
        &[0.25, 0.5, 1.0, 2.0, 4.0],
        25.0,
        5.0,
    )
    .unwrap();
    assert!(
        sweep.points.iter().any(|p| p.improvement_bps > 0.0)
            && sweep.points.iter().any(|p| p.improvement_bps < 0.0),
        "the fixture no longer flips sign, so it proves nothing: {:?}",
        sweep
            .points
            .iter()
            .map(|p| p.improvement_bps)
            .collect::<Vec<_>>()
    );
    assert!(!sweep.sign_stable);
    assert!(sweep.improvement_span_bps > 0.0);
}

/// One point is not a sweep.
#[test]
fn a_single_half_life_is_refused() {
    let err = hl_sweep(
        &[blended(), lumpy()],
        &thin_tail(),
        BUCKET,
        1.0,
        0.3,
        &[1.0],
        10.0,
        2.0,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("at least 2 points"), "{err}");
}

/// The grid is read as an ordering — naive end to flat end — so it has to be one.
#[test]
fn a_grid_that_does_not_increase_is_refused() {
    let err = hl_sweep(
        &[blended(), lumpy()],
        &thin_tail(),
        BUCKET,
        1.0,
        0.3,
        &[2.0, 1.0],
        10.0,
        2.0,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("strictly increasing"), "{err}");
}

/// Zero is the one value that would silently corrupt the sweep rather than
/// merely being wrong: `pov_forecast` reads it as the equal-weight pool, which
/// is the limit the *long* end of the grid approaches, so accepting it at the
/// short end would invert the ordering the whole report is written against. It
/// is refused, and `flat_improvement_bps` is where that case is reported.
#[test]
fn a_half_life_of_zero_or_less_is_refused() {
    for grid in [[0.0, 0.5], [-1.0, 0.5]] {
        let err = hl_sweep(
            &[blended(), lumpy()],
            &thin_tail(),
            BUCKET,
            1.0,
            0.3,
            &grid,
            10.0,
            2.0,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("positive and finite"), "{err}");
        assert!(err.contains("flat_improvement_bps"), "{err}");
    }
}
