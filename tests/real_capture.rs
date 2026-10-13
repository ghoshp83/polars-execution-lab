//! The six sessions under `data/real` are one unbroken ten-minute run of
//! BTC-USD trades from Coinbase's public feed, cut into consecutive 100-second
//! slices. Every other capture in `data/` was written by hand to make a point;
//! these were not, so what the engine reports on them is a measurement.

use xexec::model::Tick;
use xexec::replay::read_ticks;
use xexec::walkforward::{walk_forward, window_sweep, WindowSweepReport};

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
