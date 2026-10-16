//! The six sessions under `data/real` are one unbroken ten-minute run of
//! BTC-USD trades from Coinbase's public feed, cut into consecutive 100-second
//! slices. Every other capture in `data/` was written by hand to make a point;
//! these were not, so what the engine reports on them is a measurement.

use xexec::model::Tick;
use xexec::replay::read_ticks;
use xexec::walkforward::{
    share_sweep, walk_forward, walk_forward_share, window_sweep, window_sweep_share,
    WindowSweepReport,
};

/// Ten seconds per bucket: the widest five-second gap in the run is empty, and
/// a bucket with no trade has no price to execute at.
const BUCKET: i64 = 10_000_000_000;
const BUCKETS: i64 = 10;
const CAPS: [f64; 5] = [0.05, 0.1, 0.15, 0.2, 0.25];
const HALF_LIVES: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

fn sessions() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

fn sweep() -> WindowSweepReport {
    window_sweep(&sessions(), BUCKET, 0.2, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap()
}

/// What makes the files a recording and not a fixture: the exchange numbers
/// its trades, and no number is missing from the first tick to the last. A
/// session that had been trimmed or reordered to improve a result would show
/// here.
#[test]
fn the_six_sessions_are_one_unbroken_run_of_trades() {
    let sessions = sessions();
    let ticks: Vec<&Tick> = sessions.iter().flatten().collect();
    assert_eq!(ticks.len(), 1916);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    // The exchange stamps whole microseconds; a tick that is not one was
    // rounded through a float on the way to disk.
    assert!(ticks.iter().all(|t| t.ts_ns % 1_000 == 0));

    let start = sessions[0][0].ts_ns / BUCKET;
    for (i, session) in sessions.iter().enumerate() {
        let first = start + i as i64 * BUCKETS;
        let mut seen: Vec<i64> = session.iter().map(|t| t.ts_ns / BUCKET).collect();
        seen.dedup();
        assert_eq!(seen, (first..first + BUCKETS).collect::<Vec<_>>());
    }
}

/// On the written captures the only cap every walk settles is the inert one,
/// where the plan and the benchmark are the same execution. Here a cap that
/// binds gains at every step of every walk -- the first settled cap that is a
/// result -- and the other four change sign from one session to the next.
#[test]
fn one_binding_cap_gains_at_every_real_step() {
    let sessions = sessions();
    let walk = walk_forward(&sessions, BUCKET, 0.2, &CAPS, &HALF_LIVES, 25.0, 5.0, None).unwrap();
    assert_eq!(walk.consensus, ["mixed", "mixed", "mixed", "mixed", "gain"]);
    for fold in &walk.folds {
        assert_eq!(fold.verdicts[4], "gain");
        assert!(fold.flat_improvement_bps[4] > 0.0);
    }
    // The two tightest caps are inert until the newest session and a loss on it.
    assert_eq!(walk.folds[2].verdicts[..2], ["inert", "inert"]);
    assert_eq!(walk.folds[3].verdicts[..2], ["loss", "loss"]);

    let sweep = sweep();
    assert_eq!(sweep.settled_caps, 1);
    assert!(!sweep.all_settled);
    assert!(sweep.walks.iter().all(|w| w.consensus[4] == "gain"));
}

/// v0.40.0 added `every_input_stable` and could only show it on sessions
/// written for the purpose. On recorded ones the newest session reads the same
/// at every window and at every cap, and an earlier session does not.
#[test]
fn the_newest_real_session_holds_where_an_earlier_one_split() {
    let sweep = sweep();
    assert_eq!(
        (sweep.folds_run, sweep.distinct_folds, sweep.compared_inputs),
        (13, 10, 3)
    );
    assert_eq!(sweep.last_fold_stable, [true; 5]);
    assert_eq!(sweep.every_input_stable, [true, true, true, false, true]);
    assert_eq!(sweep.newest_only_caps, 1);
    assert!(!sweep.inputs[1].verdict_stable[3]);
}

/// The widest disagreement between windows is on the newest session at four
/// caps and on the one before it at the fifth, so reading the last row alone
/// understates that cap by a fifth.
#[test]
fn the_widest_real_span_at_one_cap_is_on_an_earlier_session() {
    let sweep = sweep();
    assert_eq!(sweep.widest_span_input, [5, 5, 5, 4, 5]);
    assert_eq!(
        sweep.last_fold_span_bps,
        [0.21014988, 0.39667092, 0.41470266, 0.42218023, 0.76405639]
    );
    assert_eq!(
        sweep.widest_span_bps,
        [0.21014988, 0.39667092, 0.41470266, 0.51403291, 0.76405639]
    );
}

/// Day two: the same feed, collector and cut, thirty-one hours later.
fn day_two() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d2_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// Six more whole sessions, contiguous among themselves, a day and more after
/// the first six; the gap between the two runs is the point of having them.
#[test]
fn day_two_is_its_own_unbroken_run_a_day_and_more_later() {
    let day_one = sessions();
    let day_two = day_two();
    let ticks: Vec<&Tick> = day_two.iter().flatten().collect();
    assert_eq!(ticks.len(), 5248);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_one = day_one.iter().flatten().map(|t| t.ts_ns).max().unwrap();
    assert!(ticks[0].ts_ns - last_day_one > 24 * 3600 * 1_000_000_000);
    // Over six times the volume of day one: 89 BTC against 14.
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 85.0 && volume < 95.0);
}

/// Day one's one settled cap was the first result here that was not inert. A
/// second day is the first chance to see whether it repeats, and it does not:
/// the 25% cap is unstable on two of day two's four folds and no cap settles.
/// Every session reads the same at every window, so the disagreement is
/// between days, not between windows.
#[test]
fn the_cap_day_one_settled_does_not_settle_on_day_two() {
    let day_two = day_two();
    let walk = walk_forward(&day_two, BUCKET, 0.2, &CAPS, &HALF_LIVES, 25.0, 5.0, None).unwrap();
    let at_25: Vec<&str> = walk.folds.iter().map(|f| f.verdicts[4].as_str()).collect();
    assert_eq!(at_25, ["unstable", "gain", "gain", "unstable"]);
    // 0.2 BTC is under 2.5% of every day-two session, so on the later folds no
    // cap binds and all five caps are one plan.
    for fold in &walk.folds[2..] {
        assert!(fold
            .flat_improvement_bps
            .iter()
            .all(|b| *b == fold.flat_improvement_bps[0]));
    }

    let sweep = window_sweep(&day_two, BUCKET, 0.2, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    assert_eq!(sweep.settled_caps, 0);
    assert_eq!(sweep.every_input_stable, [true; 5]);
    assert_eq!(sweep.newest_only_caps, 0);
}

/// Walked as one history the 25% cap gains on nine folds out of ten and the
/// newest session is unstable at every cap; scored from exactly six sessions,
/// so that day two's first fold sees only day one, it gains on five of six.
#[test]
fn across_two_days_no_cap_settles() {
    let mut both = sessions();
    both.extend(day_two());
    let walk = walk_forward(&both, BUCKET, 0.2, &CAPS, &HALF_LIVES, 25.0, 5.0, None).unwrap();
    assert_eq!(walk.consensus, ["mixed"; 5]);
    let at_25: Vec<&str> = walk.folds.iter().map(|f| f.verdicts[4].as_str()).collect();
    assert_eq!(at_25[..9], ["gain"; 9]);
    assert_eq!(walk.folds[9].verdicts, ["unstable"; 5]);

    let window = walk_forward(&both, BUCKET, 0.2, &CAPS, &HALF_LIVES, 25.0, 5.0, Some(6)).unwrap();
    let at_25: Vec<&str> = window
        .folds
        .iter()
        .map(|f| f.verdicts[4].as_str())
        .collect();
    assert_eq!(at_25, ["gain", "gain", "gain", "gain", "gain", "unstable"]);
    assert_eq!(window.consensus, ["mixed"; 5]);
}

/// Day one's one settled cap was the fixed parent's doing. Its sessions traded
/// from 0.83 to 5.73 BTC, so 0.2 BTC was 3.5% of one and 24% of another. A
/// share of 0.085 is the same 0.2 against day one's mean session, taken from
/// each fold's history alone, and with it nothing settles.
#[test]
fn sized_to_its_own_history_day_one_settles_no_cap() {
    let day_one = sessions();
    assert_eq!(sweep().settled_caps, 1);
    let sized = window_sweep_share(&day_one, BUCKET, 0.085, &CAPS, &HALF_LIVES, 25.0, 5.0).unwrap();
    assert_eq!(sized.settled_caps, 0);
    assert_eq!(sized.walks[0].consensus, ["mixed"; 5]);
    let grown =
        walk_forward_share(&day_one, BUCKET, 0.085, &CAPS, &HALF_LIVES, 25.0, 5.0, None).unwrap();
    let parents: Vec<f64> = grown.folds.iter().map(|f| f.parent_qty).collect();
    assert_eq!(parents, [0.35185609, 0.25819891, 0.21457672, 0.19137286]);
    let at_25: Vec<&str> = grown.folds.iter().map(|f| f.verdicts[4].as_str()).collect();
    assert_eq!(at_25, ["inert", "unstable", "gain", "gain"]);
}

/// Sizing by share moved the question from the size to the share, and the
/// share is a number nobody fits either. Swept from 5% to 60% of each fold's
/// history, day one settles no cap at any share, and the newest session's
/// verdict changes with the share at every cap: at 25% it gains at the two
/// smallest shares and loses at the two largest.
#[test]
fn across_shares_day_one_settles_nothing_and_the_newest_session_flips() {
    let report = share_sweep(
        &sessions(),
        BUCKET,
        &[0.05, 0.085, 0.15, 0.3, 0.6],
        &CAPS,
        &HALF_LIVES,
        25.0,
        5.0,
    )
    .unwrap();
    assert_eq!(report.settled_at, [0; 5]);
    assert_eq!(report.held_caps, 0);
    assert_eq!(report.last_share_stable, [false; 5]);
    let at_25: Vec<&str> = report
        .shares
        .iter()
        .map(|r| r.last_verdicts[4].as_str())
        .collect();
    assert_eq!(at_25, ["gain", "gain", "unstable", "loss", "loss"]);
}
