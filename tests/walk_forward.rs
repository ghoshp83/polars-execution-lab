use xexec::holdout::holdout;
use xexec::model::Tick;
use xexec::stability::stability;
use xexec::walkforward::{walk_forward, WalkForwardFold};

/// One second per bucket.
const BUCKET: i64 = 1_000_000_000;
/// An hour between sessions, so captures only line up by time into the session.
const HOUR: i64 = 3_600 * BUCKET;
const HALF_LIVES: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];
const CAPS: [f64; 3] = [0.01, 0.3, 0.5];

/// Shares 0.1 / 0.6 / 0.3.
const LUMPY: [f64; 3] = [1.0, 6.0, 3.0];
/// Half of each: 0.35 / 0.35 / 0.3.
const BLENDED: [f64; 3] = [3.5, 3.5, 3.0];
/// Volume collapses after the first bucket: only the tail binds.
const THIN_TAIL: [f64; 3] = [10.0, 1.0, 0.6];
/// Two even buckets and a thin one, so a tight cap binds in the *even* buckets.
const PINCHED: [f64; 3] = [1.0, 1.0, 0.02];

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

/// Three one-tick buckets with the given volumes, starting `hour` hours in.
fn session(hour: i64, volumes: [f64; 3]) -> Vec<Tick> {
    volumes
        .iter()
        .enumerate()
        .map(|(i, v)| tick(hour * HOUR + i as i64 * BUCKET, 100.0 + i as f64, *v))
        .collect()
}

/// The shapes in the order given, an hour apart, so the order is the time order.
fn in_order(shapes: [[f64; 3]; 4]) -> Vec<Vec<Tick>> {
    shapes
        .iter()
        .enumerate()
        .map(|(i, v)| session(i as i64, *v))
        .collect()
}

fn verdicts(fold: &WalkForwardFold) -> Vec<&str> {
    fold.verdicts.iter().map(String::as_str).collect()
}

/// The walk is a re-run, not a new calculation. Its last fold has every other
/// session behind it, so it must be the grid `pov-stability` prints for the
/// same history and input -- and the one fold the rotation gets right too.
#[test]
fn the_last_fold_is_the_stability_grid_and_the_rotations_last_fold() {
    let sessions = in_order([LUMPY, BLENDED, THIN_TAIL, PINCHED]);
    let walk = walk_forward(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    let grid = stability(
        &sessions[..3],
        &sessions[3],
        BUCKET,
        0.6,
        &CAPS,
        &HALF_LIVES,
        25.0,
        5.0,
    )
    .unwrap();
    let rotation = holdout(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    let last = walk.folds.last().unwrap();
    assert_eq!(last.input, 3);
    assert_eq!(
        last.flat_improvement_bps,
        grid.rows
            .iter()
            .map(|r| r.flat_improvement_bps)
            .collect::<Vec<_>>()
    );
    assert_eq!(last.unstable_caps, grid.unstable_caps);
    assert_eq!(verdicts(last), ["inert", "gain", "unstable"]);
    let rotated = rotation.folds.last().unwrap();
    assert_eq!(last.verdicts, rotated.verdicts);
    assert_eq!(last.flat_improvement_bps, rotated.flat_improvement_bps);
    assert_eq!(walk.sessions, 4);
    assert_eq!(walk.half_lives, grid.half_lives);
}

/// The property the command exists for: a fold cannot see a session later than
/// its input. Replace the newest session with a different one and the earlier
/// fold must not move by a digit.
#[test]
fn a_fold_is_untouched_by_the_sessions_after_its_input() {
    let a = in_order([LUMPY, BLENDED, THIN_TAIL, PINCHED]);
    let b = in_order([LUMPY, BLENDED, THIN_TAIL, LUMPY]);
    let wa = walk_forward(&a, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    let wb = walk_forward(&b, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    assert_eq!(
        wa.folds.iter().map(|f| f.input).collect::<Vec<_>>(),
        vec![2, 3]
    );
    assert_eq!(wa.folds[0].verdicts, wb.folds[0].verdicts);
    assert_eq!(
        wa.folds[0].flat_improvement_bps,
        wb.folds[0].flat_improvement_bps
    );
    // The newest session did change, so the last fold is free to differ.
    assert_ne!(
        wa.folds[1].flat_improvement_bps,
        wb.folds[1].flat_improvement_bps
    );
}

/// Why the rotation is not a backtest, measured. Both commands score session 2
/// here, but the rotation also pools session 3 into its history. With that
/// future session the cap at 0.3 reads `unstable`; from the past alone it is a
/// plain `loss`. The rotation's verdict was partly the future's.
#[test]
fn the_same_input_reads_differently_once_the_future_is_removed() {
    let sessions = in_order([LUMPY, BLENDED, THIN_TAIL, PINCHED]);
    let walk = walk_forward(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    let rotation = holdout(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    assert_eq!(walk.folds[0].input, 2);
    assert_eq!(rotation.folds[2].held_out, 2);
    assert_eq!(verdicts(&walk.folds[0]), ["inert", "loss", "loss"]);
    assert_eq!(
        rotation.folds[2].verdicts,
        ["inert", "unstable", "unstable"]
    );
}

/// The disagreement can run the other way. Here every step of the walk says
/// pooling pays at 0.3 and 0.5, while the rotation over the same four sessions
/// calls both caps `mixed` -- its dissenting folds are ones that forecast an
/// early session from later ones.
#[test]
fn a_walk_can_agree_where_the_rotation_does_not() {
    let sessions = in_order([LUMPY, THIN_TAIL, PINCHED, BLENDED]);
    let walk = walk_forward(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    let rotation = holdout(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    for f in &walk.folds {
        assert_eq!(verdicts(f), ["inert", "gain", "gain"]);
    }
    assert_eq!(walk.consensus, ["inert", "gain", "gain"]);
    assert_eq!(walk.agreeing_caps, 3);
    assert!(walk.all_agree);
    assert_eq!(rotation.consensus, ["inert", "mixed", "mixed"]);
}

/// A cap two steps read differently is `mixed`, and the counts follow -- here
/// the tightest cap is a `loss` with two sessions behind it and `inert` with
/// three, which is the history growing as much as the input changing.
#[test]
fn steps_that_disagree_make_the_cap_mixed() {
    let sessions = in_order([BLENDED, LUMPY, THIN_TAIL, PINCHED]);
    let walk = walk_forward(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    assert_eq!(verdicts(&walk.folds[0]), ["loss", "gain", "gain"]);
    assert_eq!(verdicts(&walk.folds[1]), ["inert", "gain", "gain"]);
    assert_eq!(walk.consensus, ["mixed", "gain", "gain"]);
    assert_eq!(walk.agreeing_caps, 2);
    assert!(!walk.all_agree);
    for f in &walk.folds {
        assert_eq!(f.inert_caps + f.stable_caps + f.unstable_caps, CAPS.len());
    }
}

/// "Oldest first" is the whole claim, so it is checked and not assumed: the
/// same four captures with two swapped would quietly forecast from the future.
#[test]
fn sessions_out_of_time_order_are_refused() {
    let mut sessions = in_order([LUMPY, BLENDED, THIN_TAIL, PINCHED]);
    sessions.swap(1, 2);
    let err = walk_forward(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap_err();
    assert!(err.to_string().contains("time order"), "{err}");
    assert!(err.to_string().contains("session 2"), "{err}");

    // Overlapping is out of order too: session 1 starts inside session 0.
    let mut overlapping = in_order([LUMPY, BLENDED, THIN_TAIL, PINCHED]);
    overlapping[1] = overlapping[1]
        .iter()
        .map(|t| tick(t.ts_ns - HOUR + BUCKET, t.price, t.size))
        .collect();
    let err = walk_forward(&overlapping, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap_err();
    assert!(err.to_string().contains("session 1"), "{err}");
}

/// The first fold needs two sessions behind it and one fold has nothing to
/// agree with, so three sessions is one fold short of a walk.
#[test]
fn fewer_than_four_sessions_is_refused() {
    let sessions = in_order([LUMPY, BLENDED, THIN_TAIL, PINCHED]);
    let err = walk_forward(&sessions[..3], BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap_err();
    assert!(err.to_string().contains("at least 4 sessions"), "{err}");
}

/// Two folds are not a sample. The walk must not be boiled down to one figure
/// or one preferred step; the check is on the serialised report a caller reads.
#[test]
fn the_walk_names_no_pooled_figure_and_no_best_fold() {
    let sessions = in_order([LUMPY, THIN_TAIL, PINCHED, BLENDED]);
    let walk = walk_forward(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    let json = serde_json::to_string(&walk).unwrap();
    for word in [
        "best",
        "worst",
        "optimal",
        "argmax",
        "recommend",
        "mean",
        "average",
        "pooled",
    ] {
        assert!(!json.contains(word), "report mentions {word:?}: {json}");
    }
}
