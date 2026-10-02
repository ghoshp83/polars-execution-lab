use xexec::hlsweep::hl_sweep;
use xexec::model::Tick;
use xexec::stability::stability;

/// One second per bucket.
const BUCKET: i64 = 1_000_000_000;
/// An hour between sessions, so captures only line up by time into the session.
const HOUR: i64 = 3_600 * BUCKET;
const HALF_LIVES: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

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

/// Two even buckets and a thin one, so a tight cap binds in the *even* buckets.
fn pinched() -> Vec<Tick> {
    session(4 * HOUR, [1.0, 1.0, 0.02])
}

/// The grid is a re-run, not a new calculation: every row must be exactly the
/// half-life sweep `pov-hl-sweep` would print at that cap.
#[test]
fn every_row_is_the_half_life_sweep_at_that_cap() {
    let history = [blended(), lumpy()];
    let caps = [0.2, 0.3, 0.5];
    let grid = stability(
        &history,
        &thin_tail(),
        BUCKET,
        1.0,
        &caps,
        &HALF_LIVES,
        10.0,
        2.0,
    )
    .unwrap();
    assert_eq!(grid.half_lives, HALF_LIVES.to_vec());
    assert_eq!(grid.rows.len(), caps.len());
    for (row, cap) in grid.rows.iter().zip(caps.iter()) {
        let s = hl_sweep(
            &history,
            &thin_tail(),
            BUCKET,
            1.0,
            *cap,
            &HALF_LIVES,
            10.0,
            2.0,
        )
        .unwrap();
        let gains: Vec<f64> = s.points.iter().map(|p| p.improvement_bps).collect();
        assert_eq!(row.cap, *cap);
        assert_eq!(row.improvement_bps, gains);
        assert_eq!(row.flat_improvement_bps, s.flat_improvement_bps);
        assert_eq!(row.improvement_span_bps, s.improvement_span_bps);
        assert_eq!(row.sign_stable, s.sign_stable);
        assert_eq!(row.inert, s.inert);
    }
}

/// **Stable for want of a sign is not stable.** At a 1% cap both plans are
/// bound in every bucket and trade identically, so every cell is exactly zero.
/// `sign_stable` is true there only because there is no sign at all; counting
/// that row in `stable_caps` would let a cap at which pooling did nothing
/// vouch for pooling. The same cap on a smaller parent is not inert, so the
/// flag is read off the run and not off the cap.
#[test]
fn a_cap_that_binds_both_plans_alike_is_inert_and_not_counted_stable() {
    let run = |parent: f64| {
        stability(
            &[blended(), lumpy()],
            &thin_tail(),
            BUCKET,
            parent,
            &[0.01, 0.3, 0.5],
            &HALF_LIVES,
            25.0,
            5.0,
        )
        .unwrap()
    };
    let grid = run(1.0);
    let inert: Vec<bool> = grid.rows.iter().map(|r| r.inert).collect();
    assert_eq!(inert, vec![true, false, false]);
    let row = &grid.rows[0];
    assert!(row.improvement_bps.iter().all(|v| *v == 0.0));
    assert_eq!(row.flat_improvement_bps, 0.0);
    assert!(row.sign_stable, "an inert row has no sign to flip");
    assert_eq!(grid.inert_caps, 1);
    assert_eq!(grid.stable_caps, 2);
    assert_eq!(grid.unstable_caps, 0);
    assert!(grid.all_sign_stable);

    let smaller = run(0.6);
    assert!(
        !smaller.rows[0].inert,
        "a 1% cap is not inert by itself: {:?}",
        smaller.rows[0].improvement_bps
    );
    assert_eq!(smaller.inert_caps, 0);
}

/// The three counts partition the grid, like `cap_sweep`'s: every row lands in
/// exactly one, so a reader who adds them up gets the grid back. The fixture
/// has one row of each kind, so a count that double-books a row fails here.
#[test]
fn inert_stable_and_unstable_partition_the_grid() {
    let grid = stability(
        &[blended(), lumpy()],
        &pinched(),
        BUCKET,
        1.0,
        &[0.01, 0.3, 0.5],
        &HALF_LIVES,
        25.0,
        5.0,
    )
    .unwrap();
    assert_eq!(
        (grid.inert_caps, grid.stable_caps, grid.unstable_caps),
        (1, 1, 1),
        "the fixture no longer has one row of each kind"
    );
    for r in &grid.rows {
        let kinds = [r.inert, r.sign_stable && !r.inert, !r.sign_stable];
        assert_eq!(kinds.iter().filter(|k| **k).count(), 1, "cap {}", r.cap);
    }
    assert!(!grid.all_sign_stable);
}

/// **The finding the grid exists for.** On the same sessions the half-life
/// decides the sign at one cap and not at the other, so stability is a property
/// of the cap as well as the sessions — which is what a single half-life sweep,
/// run at one cap, cannot show. The counts must partition the rows and the
/// summary must follow from them.
#[test]
fn a_ladder_that_is_stable_at_one_cap_and_not_another_is_reported_mixed() {
    let grid = stability(
        &[blended(), thin_tail()],
        &pinched(),
        BUCKET,
        0.6,
        &[0.3, 0.5],
        &HALF_LIVES,
        25.0,
        5.0,
    )
    .unwrap();
    let flags: Vec<bool> = grid.rows.iter().map(|r| r.sign_stable).collect();
    assert_eq!(
        flags,
        vec![true, false],
        "the fixture no longer splits, so it proves nothing: {:?}",
        grid.rows
            .iter()
            .map(|r| &r.improvement_bps)
            .collect::<Vec<_>>()
    );
    assert_eq!(grid.inert_caps, 0);
    assert_eq!(grid.stable_caps, 1);
    assert_eq!(grid.unstable_caps, 1);
    assert_eq!(
        grid.inert_caps + grid.stable_caps + grid.unstable_caps,
        grid.rows.len()
    );
    assert!(!grid.all_sign_stable);
}

/// `all_sign_stable` is the one-word answer, so it has to be `true` when no row
/// flipped.
#[test]
fn a_ladder_with_no_flip_is_all_sign_stable() {
    let grid = stability(
        &[blended(), lumpy()],
        &thin_tail(),
        BUCKET,
        1.0,
        &[0.3, 0.5],
        &HALF_LIVES,
        10.0,
        2.0,
    )
    .unwrap();
    assert_eq!(grid.unstable_caps, 0);
    assert!(grid.all_sign_stable);
}

/// **The refusal, pinned.** A two-parameter grid scored on the session it is
/// fitted to must not name a best cell: that would be a cap and a half-life
/// chosen with the answer in hand, and it would read as a recommendation however
/// it was documented. The check is on the serialised report, which is what a
/// caller actually reads, so a field added later under any `best`/`worst`/
/// `optimal` name fails here.
#[test]
fn the_grid_names_no_best_cell() {
    let grid = stability(
        &[blended(), thin_tail()],
        &pinched(),
        BUCKET,
        0.6,
        &[0.3, 0.5],
        &HALF_LIVES,
        25.0,
        5.0,
    )
    .unwrap();
    let json = serde_json::to_string(&grid).unwrap();
    for word in ["best", "worst", "optimal", "argmax", "recommend"] {
        assert!(!json.contains(word), "{word} appeared in {json}");
    }
}

/// One cap is not a grid; `pov-hl-sweep` already answers that question.
#[test]
fn a_single_cap_is_refused() {
    let err = stability(
        &[blended(), lumpy()],
        &thin_tail(),
        BUCKET,
        1.0,
        &[0.3],
        &HALF_LIVES,
        10.0,
        2.0,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("at least 2 points"), "{err}");
}

/// The cap grid follows `cap_sweep`'s rules, so a grid one command accepts the
/// other does too.
#[test]
fn a_cap_grid_out_of_range_or_out_of_order_is_refused() {
    for (caps, want) in [
        (vec![0.5, 0.3], "strictly increasing"),
        (vec![0.5, 1.5], "(0, 1]"),
        (vec![0.0, 0.5], "(0, 1]"),
    ] {
        let err = stability(
            &[blended(), lumpy()],
            &thin_tail(),
            BUCKET,
            1.0,
            &caps,
            &HALF_LIVES,
            10.0,
            2.0,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains(want), "{caps:?}: {err}");
    }
}

/// The half-life grid is validated by `hl_sweep`, not re-implemented here — so
/// its refusal, including the reason zero is refused, reaches the caller intact.
#[test]
fn a_half_life_grid_containing_zero_is_refused_with_the_sweeps_reason() {
    let err = stability(
        &[blended(), lumpy()],
        &thin_tail(),
        BUCKET,
        1.0,
        &[0.3, 0.5],
        &[0.0, 1.0],
        10.0,
        2.0,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("flat_improvement_bps"), "{err}");
}
