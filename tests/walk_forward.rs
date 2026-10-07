use xexec::holdout::holdout;
use xexec::model::Tick;
use xexec::stability::stability;
use xexec::walkforward::{walk_forward, WalkForwardFold, WalkForwardReport};

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
fn in_order<const N: usize>(shapes: [[f64; 3]; N]) -> Vec<Vec<Tick>> {
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
    let walk = walk_forward(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0, None).unwrap();
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
    let wa = walk_forward(&a, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0, None).unwrap();
    let wb = walk_forward(&b, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0, None).unwrap();
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
    let walk = walk_forward(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0, None).unwrap();
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
    let walk = walk_forward(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0, None).unwrap();
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
    let walk = walk_forward(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0, None).unwrap();
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
    let err =
        walk_forward(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0, None).unwrap_err();
    assert!(err.to_string().contains("time order"), "{err}");
    assert!(err.to_string().contains("session 2"), "{err}");

    // Overlapping is out of order too: session 1 starts inside session 0.
    let mut overlapping = in_order([LUMPY, BLENDED, THIN_TAIL, PINCHED]);
    overlapping[1] = overlapping[1]
        .iter()
        .map(|t| tick(t.ts_ns - HOUR + BUCKET, t.price, t.size))
        .collect();
    let err = walk_forward(
        &overlapping,
        BUCKET,
        0.6,
        &CAPS,
        &HALF_LIVES,
        25.0,
        5.0,
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("session 1"), "{err}");
}

/// The first fold needs two sessions behind it and one fold has nothing to
/// agree with, so three sessions is one fold short of a walk.
#[test]
fn fewer_than_four_sessions_is_refused() {
    let sessions = in_order([LUMPY, BLENDED, THIN_TAIL, PINCHED]);
    let err = walk_forward(
        &sessions[..3],
        BUCKET,
        0.6,
        &CAPS,
        &HALF_LIVES,
        25.0,
        5.0,
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("at least 4 sessions"), "{err}");
}

/// Two folds are not a sample. The walk must not be boiled down to one figure
/// or one preferred step; the check is on the serialised report a caller reads.
#[test]
fn the_walk_names_no_pooled_figure_and_no_best_fold() {
    let sessions = in_order([LUMPY, THIN_TAIL, PINCHED, BLENDED]);
    let walk = walk_forward(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0, None).unwrap();
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

fn walk(sessions: &[Vec<Tick>], window: Option<usize>) -> WalkForwardReport {
    walk_forward(sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0, window).unwrap()
}

/// Leaving the window off must still be the walk it always was: the history
/// grows from session 0, and the report says so instead of leaving it implied.
/// A window of two starts on the same fold, since both pool sessions 0 and 1.
#[test]
fn without_a_window_the_history_grows_from_the_first_session() {
    let sessions = in_order([LUMPY, BLENDED, PINCHED, THIN_TAIL]);
    let expanding = walk(&sessions, None);
    let rolling = walk(&sessions, Some(2));
    assert_eq!(expanding.window, None);
    assert_eq!(rolling.window, Some(2));
    let from = |w: &WalkForwardReport| w.folds.iter().map(|f| f.history_from).collect::<Vec<_>>();
    assert_eq!(from(&expanding), vec![0, 0]);
    assert_eq!(from(&rolling), vec![0, 1]);
    assert_eq!(expanding.folds[0].verdicts, rolling.folds[0].verdicts);
    assert_eq!(
        expanding.folds[0].flat_improvement_bps,
        rolling.folds[0].flat_improvement_bps
    );
}

/// A windowed fold is a re-run too: the grid `pov-stability` prints for just
/// the sessions inside the window. Here that changes the reading -- with the
/// oldest session pooled in, the last fold is `unstable` at 0.3 and 0.5; from
/// the two sessions before the input it is a `gain` at both.
#[test]
fn a_windowed_fold_is_the_stability_grid_on_its_window_alone() {
    let sessions = in_order([LUMPY, BLENDED, PINCHED, THIN_TAIL]);
    let rolling = walk(&sessions, Some(2));
    let grid = stability(
        &sessions[1..3],
        &sessions[3],
        BUCKET,
        0.6,
        &CAPS,
        &HALF_LIVES,
        25.0,
        5.0,
    )
    .unwrap();
    let last = rolling.folds.last().unwrap();
    assert_eq!((last.input, last.history_from), (3, 1));
    assert_eq!(
        last.flat_improvement_bps,
        grid.rows
            .iter()
            .map(|r| r.flat_improvement_bps)
            .collect::<Vec<_>>()
    );
    assert_eq!(verdicts(last), ["inert", "gain", "gain"]);
    let expanding = walk(&sessions, None);
    assert_eq!(
        verdicts(expanding.folds.last().unwrap()),
        ["inert", "unstable", "unstable"]
    );
}

/// The property the window exists for: a fold cannot see a session older than
/// its window. Replace the oldest session and the windowed last fold must not
/// move by a digit, while the expanding one, which still pools it, does.
#[test]
fn a_windowed_fold_is_untouched_by_sessions_older_than_its_window() {
    let a = in_order([LUMPY, BLENDED, PINCHED, THIN_TAIL]);
    let b = in_order([THIN_TAIL, BLENDED, PINCHED, THIN_TAIL]);
    assert_eq!(
        walk(&a, Some(2)).folds[1].flat_improvement_bps,
        walk(&b, Some(2)).folds[1].flat_improvement_bps
    );
    assert_ne!(
        walk(&a, None).folds[1].flat_improvement_bps,
        walk(&b, None).folds[1].flat_improvement_bps
    );
}

/// An agreement can rest on the oldest session. The expanding walk says pooling
/// pays at 0.3 and 0.5 at every step here; held to two sessions of history,
/// the last step reads both caps `unstable` and only the inert cap agrees.
#[test]
fn an_agreement_can_rest_on_the_oldest_session() {
    let sessions = in_order([LUMPY, THIN_TAIL, PINCHED, BLENDED]);
    let expanding = walk(&sessions, None);
    let rolling = walk(&sessions, Some(2));
    assert_eq!(expanding.consensus, ["inert", "gain", "gain"]);
    assert!(expanding.all_agree);
    assert_eq!(
        verdicts(rolling.folds.last().unwrap()),
        ["inert", "unstable", "unstable"]
    );
    assert_eq!(rolling.consensus, ["inert", "mixed", "mixed"]);
    assert_eq!(rolling.agreeing_caps, 1);
    assert!(!rolling.all_agree);
}

/// Why nothing here picks a window. The same five sessions agree on two caps
/// with a growing history, on none with a window of two and on all three with
/// a window of three -- and the widest window is also the one with a fold
/// fewer, so the fullest agreement is the one with the least behind it.
#[test]
fn three_walks_over_the_same_sessions_give_three_answers() {
    let sessions = in_order([BLENDED, LUMPY, THIN_TAIL, PINCHED, BLENDED]);
    let expanding = walk(&sessions, None);
    let two = walk(&sessions, Some(2));
    let three = walk(&sessions, Some(3));
    assert_eq!(expanding.agreeing_caps, 2);
    assert_eq!(two.agreeing_caps, 0);
    assert_eq!(three.agreeing_caps, 3);
    assert_eq!(expanding.folds.len(), 3);
    assert_eq!(two.folds.len(), 3);
    assert_eq!(three.folds.len(), 2);
    let spans = |w: &WalkForwardReport| {
        w.folds
            .iter()
            .map(|f| (f.history_from, f.input))
            .collect::<Vec<_>>()
    };
    assert_eq!(spans(&two), vec![(0, 2), (1, 3), (2, 4)]);
    assert_eq!(spans(&three), vec![(0, 3), (1, 4)]);
}

/// A window of one is the pool of one the walk already refuses, and a window
/// that leaves a single fold leaves nothing for it to agree with.
#[test]
fn a_window_too_narrow_or_too_wide_is_refused() {
    let four = in_order([LUMPY, BLENDED, THIN_TAIL, PINCHED]);
    for w in [0, 1] {
        let err =
            walk_forward(&four, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0, Some(w)).unwrap_err();
        assert!(err.to_string().contains("at least 2 sessions"), "{err}");
    }
    let err = walk_forward(&four, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0, Some(3)).unwrap_err();
    assert!(
        err.to_string()
            .contains("window of 3 needs at least 5 sessions, got 4"),
        "{err}"
    );
    let five = in_order([LUMPY, BLENDED, THIN_TAIL, PINCHED, LUMPY]);
    assert_eq!(walk(&five, Some(3)).folds.len(), 2);
}

/// The window is an input, never an output: the report repeats the one it was
/// given and must not grow a field that prefers one.
#[test]
fn a_windowed_walk_names_no_best_window() {
    let sessions = in_order([BLENDED, LUMPY, THIN_TAIL, PINCHED, BLENDED]);
    let json = serde_json::to_string(&walk(&sessions, Some(3))).unwrap();
    assert!(json.contains("\"window\":3"), "{json}");
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
