use xexec::holdout::holdout;
use xexec::model::Tick;
use xexec::stability::stability;

/// One second per bucket.
const BUCKET: i64 = 1_000_000_000;
/// An hour between sessions, so captures only line up by time into the session.
const HOUR: i64 = 3_600 * BUCKET;
const HALF_LIVES: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];
const CAPS: [f64; 3] = [0.01, 0.3, 0.5];

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

fn verdicts(fold: &xexec::holdout::HoldoutFold) -> Vec<&str> {
    fold.verdicts.iter().map(String::as_str).collect()
}

/// The rotation is a re-run, not a new calculation: the fold that holds out the
/// last session must be the grid `pov-stability` prints for the same history
/// and input, or the two commands would disagree about the same question.
#[test]
fn the_last_fold_is_the_stability_grid() {
    let sessions = vec![blended(), lumpy(), pinched()];
    let report = holdout(&sessions, BUCKET, 1.0, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    let grid = stability(
        &sessions[..2],
        &sessions[2],
        BUCKET,
        1.0,
        &CAPS,
        &HALF_LIVES,
        25.0,
        5.0,
    )
    .unwrap();
    let last = report.folds.last().unwrap();
    assert_eq!(last.held_out, 2);
    assert_eq!(
        last.flat_improvement_bps,
        grid.rows
            .iter()
            .map(|r| r.flat_improvement_bps)
            .collect::<Vec<_>>()
    );
    assert_eq!(last.inert_caps, grid.inert_caps);
    assert_eq!(last.stable_caps, grid.stable_caps);
    assert_eq!(last.unstable_caps, grid.unstable_caps);
    // This grid is the one that is unstable at 0.3, and the fold must say so.
    assert_eq!(verdicts(last), ["inert", "unstable", "gain"]);
    assert_eq!(
        report.caps,
        grid.rows.iter().map(|r| r.cap).collect::<Vec<_>>()
    );
    assert_eq!(report.half_lives, grid.half_lives);
    assert_eq!(report.sessions, 3);
}

/// Every session takes exactly one turn, in the order given, and each fold
/// pools the rest -- so the folds' counts each cover the whole cap grid.
#[test]
fn every_session_is_held_out_exactly_once() {
    let sessions = vec![blended(), lumpy(), thin_tail(), pinched()];
    let report = holdout(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    assert_eq!(report.sessions, 4);
    assert_eq!(
        report.folds.iter().map(|f| f.held_out).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    for f in &report.folds {
        assert_eq!(f.verdicts.len(), CAPS.len());
        assert_eq!(f.flat_improvement_bps.len(), CAPS.len());
        assert_eq!(f.inert_caps + f.stable_caps + f.unstable_caps, CAPS.len());
    }
}

/// When every fold reads a cap the same way the consensus is that reading --
/// including `inert`, which is agreement that nothing was measured and must
/// stay visible as such instead of hiding behind a bare "agrees".
#[test]
fn a_verdict_every_fold_shares_is_the_consensus() {
    let sessions = vec![blended(), lumpy(), thin_tail()];
    let report = holdout(&sessions, BUCKET, 1.0, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    for f in &report.folds {
        assert_eq!(verdicts(f), ["inert", "gain", "gain"]);
    }
    assert_eq!(report.consensus, ["inert", "gain", "gain"]);
    assert_eq!(report.agreeing_caps, 3);
    assert!(report.all_agree);
}

/// The reason a fold carries a direction and not just `sign_stable`: here no
/// fold has an unstable cap, so every fold's own grid calls 0.3 and 0.5 stable
/// -- yet two folds say pooling costs and one says it pays. Agreement on
/// stability is not agreement on the answer.
#[test]
fn folds_that_are_each_stable_can_still_disagree_on_the_sign() {
    let sessions = vec![lumpy(), pinched(), blended()];
    let report = holdout(&sessions, BUCKET, 0.6, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    for f in &report.folds {
        assert_eq!(f.unstable_caps, 0);
        assert_eq!(f.stable_caps, 2);
    }
    assert_eq!(verdicts(&report.folds[0]), ["inert", "loss", "loss"]);
    assert_eq!(verdicts(&report.folds[1]), ["inert", "loss", "loss"]);
    assert_eq!(verdicts(&report.folds[2]), ["inert", "gain", "gain"]);
    assert_eq!(report.consensus, ["inert", "mixed", "mixed"]);
    assert_eq!(report.agreeing_caps, 1);
    assert!(!report.all_agree);
    // The directions are read off figures the fold itself reports.
    assert!(report.folds[0].flat_improvement_bps[1] < 0.0);
    assert!(report.folds[2].flat_improvement_bps[1] > 0.0);
}

/// The history order is part of the question -- the last history session is
/// the naive forecast and the half-life weights by recency -- so a rotation of
/// the same sessions in a different order is a different report.
#[test]
fn the_order_the_sessions_are_given_in_matters() {
    let a = holdout(
        &[blended(), lumpy(), pinched()],
        BUCKET,
        1.0,
        &CAPS,
        &HALF_LIVES,
        25.0,
        5.0,
    )
    .unwrap();
    let b = holdout(
        &[lumpy(), pinched(), blended()],
        BUCKET,
        1.0,
        &CAPS,
        &HALF_LIVES,
        25.0,
        5.0,
    )
    .unwrap();
    assert_eq!(a.consensus, ["inert", "mixed", "gain"]);
    assert_eq!(b.consensus, ["inert", "mixed", "mixed"]);
}

/// Each fold pools the other sessions; with two sessions that pool is a single
/// capture and there is nothing to rotate.
#[test]
fn fewer_than_three_sessions_is_refused() {
    let err = holdout(
        &[blended(), lumpy()],
        BUCKET,
        1.0,
        &CAPS,
        &HALF_LIVES,
        25.0,
        5.0,
    )
    .unwrap_err();
    assert!(err.to_string().contains("at least 3 sessions"), "{err}");
}

/// A rotation over a handful of sessions must not be boiled down to one figure
/// or one preferred fold: a mean would read as an estimate and a best fold as a
/// recommendation. The check is on the serialised report a caller reads.
#[test]
fn the_rotation_names_no_pooled_figure_and_no_best_fold() {
    let sessions = vec![blended(), lumpy(), thin_tail()];
    let report = holdout(&sessions, BUCKET, 1.0, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    let json = serde_json::to_string(&report).unwrap();
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
