use xexec::holdout::holdout;
use xexec::model::Tick;
use xexec::stability::stability;
use xexec::walkforward::{
    walk_forward, window_sweep, WalkForwardFold, WalkForwardReport, WindowSweepReport,
};

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

fn sweep(sessions: &[Vec<Tick>]) -> WindowSweepReport {
    window_sweep(sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap()
}

/// The sweep is a re-run too: each of its rows must be the walk `pov-walkforward`
/// prints at that window, so a number here can be traced to a fold there.
#[test]
fn the_sweep_is_the_walk_at_every_window_the_sessions_allow() {
    let sessions = in_order([BLENDED, LUMPY, THIN_TAIL, PINCHED, BLENDED]);
    let report = sweep(&sessions);
    let windows: Vec<Option<usize>> = report.walks.iter().map(|w| w.window).collect();
    assert_eq!(windows, vec![None, Some(2), Some(3)]);
    for row in &report.walks {
        let single = walk(&sessions, row.window);
        let last = single.folds.last().unwrap();
        assert_eq!(row.folds, single.folds.len());
        assert_eq!(row.consensus, single.consensus);
        assert_eq!(row.agreeing_caps, single.agreeing_caps);
        assert_eq!(last.input, 4, "every walk ends on the newest session");
        assert_eq!(row.last_verdicts, last.verdicts);
        assert_eq!(row.last_flat_improvement_bps, last.flat_improvement_bps);
    }
    assert_eq!(report.sessions, 5);
    assert_eq!(report.caps, CAPS.to_vec());
}

/// Four sessions leave room for one window, and it is the narrowest: a window
/// of three would leave a single fold. Fewer sessions are the walk's refusal.
#[test]
fn four_sessions_allow_one_window_and_three_are_refused() {
    let four = in_order([LUMPY, BLENDED, THIN_TAIL, PINCHED]);
    let windows: Vec<Option<usize>> = sweep(&four).walks.iter().map(|w| w.window).collect();
    assert_eq!(windows, vec![None, Some(2)]);
    let err = window_sweep(&four[..3], BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap_err();
    assert!(err.to_string().contains("at least 4 sessions"), "{err}");
    let mut swapped = four.clone();
    swapped.swap(1, 2);
    let err = window_sweep(&swapped, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap_err();
    assert!(err.to_string().contains("time order"), "{err}");
}

/// Two walks that both call a cap `mixed` gave the same reading, and neither
/// gave a verdict. Counting that as agreement would report a sweep as settled
/// on exactly the caps no walk could settle.
#[test]
fn caps_every_walk_calls_mixed_match_without_being_settled() {
    let report = sweep(&in_order([LUMPY, BLENDED, PINCHED, THIN_TAIL]));
    for row in &report.walks {
        assert_eq!(row.consensus, ["inert", "mixed", "mixed"]);
    }
    assert_eq!(report.consensus_stable, [true, true, true]);
    assert_eq!(report.settled_caps, 1);
    assert!(!report.all_settled);
}

/// The same session, scored at the same caps, called `unstable` from three
/// sessions of history and a `gain` from two. This is the comparison the
/// consensus cannot make, because here both walks' consensus is `mixed`.
#[test]
fn the_newest_session_can_change_verdict_with_the_window() {
    let report = sweep(&in_order([LUMPY, BLENDED, PINCHED, THIN_TAIL]));
    assert_eq!(
        report.walks[0].last_verdicts,
        ["inert", "unstable", "unstable"]
    );
    assert_eq!(report.walks[1].last_verdicts, ["inert", "gain", "gain"]);
    assert_eq!(report.last_fold_stable, [true, false, false]);
    assert_eq!(report.consensus_stable, [true, true, true]);
    // 0.47747641 at the window of two, less -1.06866706 on the growing walk.
    assert_eq!(report.last_fold_span_bps, [0.0, 1.54614347, 1.54614347]);
}

/// A verdict that survives the window says the sign did. The span beside it is
/// how much of the size did not, and it can be most of the figure.
#[test]
fn a_span_can_sit_under_a_verdict_that_did_not_move() {
    let report = sweep(&in_order([LUMPY, BLENDED, THIN_TAIL, PINCHED]));
    assert_eq!(report.last_fold_stable, [true, true, true]);
    assert_eq!(report.walks[0].last_verdicts, report.walks[1].last_verdicts);
    assert_eq!(report.last_fold_span_bps, [0.0, 0.56041748, 1.58217664]);
    for (c, span) in report.last_fold_span_bps.iter().enumerate() {
        let a = report.walks[0].last_flat_improvement_bps[c];
        let b = report.walks[1].last_flat_improvement_bps[c];
        assert!((span - (a - b).abs()).abs() < 1e-8, "cap {c}: {span}");
    }
}

/// An agreement at one depth of history is not an agreement at another: the
/// growing walk settles two caps as gains that the window of two leaves mixed.
#[test]
fn a_consensus_can_hold_at_one_window_and_not_at_another() {
    let report = sweep(&in_order([LUMPY, THIN_TAIL, PINCHED, BLENDED]));
    assert_eq!(report.walks[0].consensus, ["inert", "gain", "gain"]);
    assert_eq!(report.walks[1].consensus, ["inert", "mixed", "mixed"]);
    assert_eq!(report.consensus_stable, [true, false, false]);
    assert_eq!(report.settled_caps, 1);
    // With a fifth session no cap reads the same along all three walks.
    let five = sweep(&in_order([BLENDED, LUMPY, THIN_TAIL, PINCHED, BLENDED]));
    assert_eq!(five.consensus_stable, [false, false, false]);
    assert_eq!(five.settled_caps, 0);
}

/// Settled is not the same as unmoved. Both walks give every cap one verdict
/// here, and the newest session's figure still shifts between them.
#[test]
fn a_sweep_can_settle_every_cap_and_still_report_a_span() {
    let report = sweep(&in_order([LUMPY, BLENDED, LUMPY, BLENDED]));
    for row in &report.walks {
        assert_eq!(row.consensus, ["inert", "gain", "gain"]);
    }
    assert_eq!(report.settled_caps, 3);
    assert!(report.all_settled);
    assert_eq!(report.last_fold_stable, [true, true, true]);
    assert_eq!(report.last_fold_span_bps, [0.0, 0.18415301, 0.18415301]);
}

/// The sweep lists the walks in the order it ran them and prefers none.
#[test]
fn the_sweep_names_no_best_window() {
    let sessions = in_order([BLENDED, LUMPY, THIN_TAIL, PINCHED, BLENDED]);
    let json = serde_json::to_string(&sweep(&sessions)).unwrap();
    assert!(json.contains("\"walks\":["), "{json}");
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

/// A window of `w` starts on session `w` with sessions `0..w` behind it -- the
/// fold the growing walk already ran there. The sweep lists it under both
/// walks, so it has to say how many folds are really behind its rows.
#[test]
fn each_window_repeats_one_fold_of_the_growing_walk() {
    let sessions = in_order([BLENDED, LUMPY, THIN_TAIL, PINCHED, BLENDED]);
    let report = sweep(&sessions);
    let growing = walk(&sessions, None);
    for w in [2, 3] {
        let windowed = walk(&sessions, Some(w));
        let first = &windowed.folds[0];
        let twin = growing.folds.iter().find(|f| f.input == w).unwrap();
        assert_eq!((first.history_from, first.input), (0, w));
        assert_eq!(first.verdicts, twin.verdicts);
        assert_eq!(first.flat_improvement_bps, twin.flat_improvement_bps);
    }
    assert_eq!(report.folds_run, 3 + 3 + 2);
    assert_eq!(report.distinct_folds, report.folds_run - 2);
    let four = sweep(&in_order([LUMPY, BLENDED, THIN_TAIL, PINCHED]));
    assert_eq!((four.folds_run, four.distinct_folds), (4, 3));
}

/// The third session is scored by two walks from the same two sessions. That
/// is one reading listed twice: it cannot disagree with itself, and it must not
/// count as a session the window was tested on.
#[test]
fn a_session_with_one_history_under_it_was_not_compared() {
    for sessions in [
        in_order([LUMPY, BLENDED, THIN_TAIL, PINCHED]),
        in_order([LUMPY, THIN_TAIL, PINCHED, BLENDED]),
        in_order([BLENDED, LUMPY, THIN_TAIL, PINCHED, BLENDED]),
    ] {
        let report = sweep(&sessions);
        let third = &report.inputs[0];
        assert_eq!((third.input, third.walks, third.histories), (2, 2, 1));
        assert_eq!(third.verdict_stable, [true, true, true]);
        assert_eq!(third.span_bps, [0.0, 0.0, 0.0]);
        assert_eq!(report.compared_inputs, sessions.len() - 3);
    }
}

/// `inputs` is the newest-session comparison made for every scored session, so
/// its last entry has to be that comparison and not a second opinion on it.
#[test]
fn the_last_input_is_the_newest_session_comparison() {
    let report = sweep(&in_order([BLENDED, LUMPY, THIN_TAIL, PINCHED, BLENDED]));
    let shape: Vec<(usize, usize, usize)> = report
        .inputs
        .iter()
        .map(|i| (i.input, i.walks, i.histories))
        .collect();
    assert_eq!(shape, vec![(2, 2, 1), (3, 3, 2), (4, 3, 3)]);
    let last = report.inputs.last().unwrap();
    assert_eq!(last.verdict_stable, report.last_fold_stable);
    assert_eq!(last.span_bps, report.last_fold_span_bps);
    let histories: usize = report.inputs.iter().map(|i| i.histories).sum();
    assert_eq!(histories, report.distinct_folds);
}

/// The newest session is not the only one the window can move. Here the fourth
/// changes verdict at both binding caps while the fifth holds at the widest --
/// a reader of `last_fold_stable` alone would call that cap unmoved.
#[test]
fn a_middle_session_can_move_where_the_newest_does_not() {
    let report = sweep(&in_order([LUMPY, BLENDED, PINCHED, THIN_TAIL, LUMPY]));
    assert_eq!(report.last_fold_stable, [true, false, true]);
    assert_eq!(report.inputs[1].input, 3);
    assert_eq!(report.inputs[1].verdict_stable, [true, false, false]);
    assert_eq!(report.inputs[1].span_bps, [0.0, 1.54614347, 1.54614347]);
    assert_eq!(report.inputs[2].span_bps, [0.0, 1.09081414, 4.32991899]);
}

/// Every cap settled, on four sessions, is one session read from two depths of
/// history. The count belongs beside the verdict it qualifies.
#[test]
fn a_settled_sweep_of_four_sessions_rests_on_one_comparison() {
    let report = sweep(&in_order([LUMPY, BLENDED, LUMPY, BLENDED]));
    assert!(report.all_settled);
    assert_eq!(report.compared_inputs, 1);
    assert_eq!((report.folds_run, report.distinct_folds), (4, 3));
    assert_eq!(report.inputs[1].histories, 2);
    assert_eq!(report.inputs[1].span_bps, [0.0, 0.18415301, 0.18415301]);
}
