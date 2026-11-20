//! The six sessions under `data/real` are one unbroken ten-minute run of
//! BTC-USD trades from Coinbase's public feed, cut into consecutive 100-second
//! slices. Every other capture in `data/` was written by hand to make a point;
//! these were not, so what the engine reports on them is a measurement.

use xexec::curve::sweep_curve;
use xexec::depth::{depth_metrics, queue_metrics};
use xexec::grid::{
    book_grid_across, book_grid_phase, book_grid_phase_across, book_grid_sweep, GridPhaseAcross,
};
use xexec::model::{BookLevel, Tick};
use xexec::replay::{read_book, read_ticks};
use xexec::sweep::sweep_cost;
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

/// Day three: the same feed, collector and cut, on a Saturday morning.
fn day_three() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d3_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A third unbroken run, two and a half days after the second, and the
/// quietest of the three: under 3 BTC against 14 and 89, with a last session
/// that traded less than a tenth of one.
#[test]
fn day_three_is_its_own_unbroken_run_and_the_quietest() {
    let day_two = day_two();
    let day_three = day_three();
    let ticks: Vec<&Tick> = day_three.iter().flatten().collect();
    assert_eq!(ticks.len(), 1986);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_two = day_two.iter().flatten().map(|t| t.ts_ns).max().unwrap();
    assert!(ticks[0].ts_ns - last_day_two > 48 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 2.5 && volume < 3.0);
    let last: f64 = day_three[5].iter().map(|t| t.size).sum();
    assert!(last < 0.1);
}

/// On the quietest day the share sweep settles nothing either. What does hold
/// across every share is the newest session's reading at four caps -- and it
/// holds because that session is inert there: a 0.067 BTC session gives every
/// half-life the same figure. `last_share_stable` is a verdict that did not
/// move, not a verdict that says gain or loss.
#[test]
fn on_the_quietest_day_what_holds_across_shares_is_only_inert() {
    let report = share_sweep(
        &day_three(),
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
    assert_eq!(report.last_share_stable, [true, true, true, true, false]);
    for row in &report.shares {
        assert!(row.last_verdicts[..4].iter().all(|v| v == "inert"));
    }
    let at_25: Vec<&str> = report
        .shares
        .iter()
        .map(|r| r.last_verdicts[4].as_str())
        .collect();
    assert_eq!(at_25, ["unstable", "inert", "inert", "inert", "inert"]);
}

/// The first recorded book: the top ten levels of each side after every
/// `level2_batch` update, over the same 100 seconds as the first day-three
/// session. Ten levels of BTC-USD hold well under one bitcoin, so the README's
/// 0.2 BTC parent swept at once does not always fit, and two never does.
#[test]
fn the_recorded_book_covers_its_session_and_runs_out_of_levels() {
    let book = read_book("data/real/btc_usd_d3_1_book.ndjson").expect("recorded book must load");
    let session = &day_three()[0];
    let first = book.iter().map(|l| l.ts_ns).min().unwrap();
    let last = book.iter().map(|l| l.ts_ns).max().unwrap();
    assert_eq!(first / BUCKET, session[0].ts_ns / BUCKET);
    assert_eq!(last / BUCKET, session[session.len() - 1].ts_ns / BUCKET);

    let depth = depth_metrics(&book, "BTC-USD").unwrap();
    assert_eq!(depth.snapshots, 1431);
    assert!((depth.avg_ask_depth - 0.66147753).abs() < 1e-8);
    assert!((depth.avg_bid_depth - 0.73534154).abs() < 1e-8);
    assert!(depth.avg_spread < 0.015);

    let parent = sweep_cost(&book, "BTC-USD", "buy", 0.2).unwrap();
    assert_eq!(parent.filled_snapshots, 1279);
    let two = sweep_cost(&book, "BTC-USD", "buy", 2.0).unwrap();
    assert_eq!(two.filled_snapshots, 0);
    // The bid holds more size and still costs more to hit: it sits further out.
    let sell = sweep_cost(&book, "BTC-USD", "sell", 0.2).unwrap();
    assert!(sell.avg_slippage_bps > 7.0 * parent.avg_slippage_bps);

    let curve = sweep_curve(
        &book,
        "BTC-USD",
        "buy",
        &[0.01, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0],
    )
    .unwrap();
    assert_eq!(curve.fitted_points, 3);
    assert!((curve.coef_bps - 0.0848413).abs() < 1e-8);
}

/// Day four: the same feed, collector and cut, on the Sunday morning after.
fn day_four() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d4_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A fourth unbroken run, the morning after the third: 938 trades and about
/// four and a half BTC, with no session so quiet that it goes inert.
#[test]
fn day_four_is_its_own_unbroken_run_the_morning_after() {
    let day_three = day_three();
    let day_four = day_four();
    let ticks: Vec<&Tick> = day_four.iter().flatten().collect();
    assert_eq!(ticks.len(), 938);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_three = day_three.iter().flatten().map(|t| t.ts_ns).max().unwrap();
    assert!(ticks[0].ts_ns - last_day_three > 20 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 4.5 && volume < 4.6);
    assert!(day_four[5].iter().map(|t| t.size).sum::<f64>() > 1.0);
}

/// Day three's `last_share_stable` held only where its newest session was
/// inert. Day four's newest session traded over a bitcoin, and the reading
/// moves with the share at every cap: nothing holds once nothing is inert.
#[test]
fn when_the_newest_session_trades_nothing_holds_across_shares() {
    let report = share_sweep(
        &day_four(),
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
    for row in &report.shares {
        assert!(row.last_verdicts.iter().all(|v| v != "inert"));
    }
    let at_25: Vec<&str> = report
        .shares
        .iter()
        .map(|r| r.last_verdicts[4].as_str())
        .collect();
    assert_eq!(at_25, ["loss", "unstable", "unstable", "gain", "unstable"]);
}

/// The deep book: fifty levels a side, one snapshot every five seconds, over
/// all six day-four sessions. Every size to 5 BTC fills in every snapshot. Cut
/// to its own top ten levels, the same snapshots fill 2 BTC in 32 of 119 and
/// 5 BTC in none, and the curve fits three points to a coefficient under a
/// third of the deep one: a shallow recording sets a floor, not a measurement.
#[test]
fn fifty_levels_fill_what_ten_levels_of_the_same_book_could_not() {
    let book = read_book("data/real/btc_usd_d4_book.ndjson").expect("recorded book must load");
    let day = day_four();
    let first = book.iter().map(|l| l.ts_ns).min().unwrap();
    let last = book.iter().map(|l| l.ts_ns).max().unwrap();
    assert_eq!(first / BUCKET, day[0][0].ts_ns / BUCKET);
    assert!(last < (day[0][0].ts_ns / BUCKET + 60) * BUCKET);
    assert_eq!(book.iter().map(|l| l.level).max(), Some(49));

    let depth = depth_metrics(&book, "BTC-USD").unwrap();
    assert_eq!(depth.snapshots, 119);
    assert!((depth.avg_ask_depth - 9.67633378).abs() < 1e-8);
    for side in ["buy", "sell"] {
        let five = sweep_cost(&book, "BTC-USD", side, 5.0).unwrap();
        assert_eq!(five.filled_snapshots, 119);
    }
    let top: Vec<_> = book.iter().filter(|l| l.level < 10).cloned().collect();
    assert_eq!(
        sweep_cost(&top, "BTC-USD", "buy", 2.0)
            .unwrap()
            .filled_snapshots,
        32
    );
    assert_eq!(
        sweep_cost(&top, "BTC-USD", "buy", 5.0)
            .unwrap()
            .filled_snapshots,
        0
    );

    let sizes = [0.01, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0];
    let deep = sweep_curve(&book, "BTC-USD", "buy", &sizes).unwrap();
    assert_eq!(deep.fitted_points, 8);
    assert!((deep.coef_bps - 1.74400041).abs() < 1e-8);
    let shallow = sweep_curve(&top, "BTC-USD", "buy", &sizes).unwrap();
    assert_eq!(shallow.fitted_points, 3);
    assert!((shallow.coef_bps - 0.48342117).abs() < 1e-8);
}

/// Day five: the same feed, collector and cut, on the Monday morning after.
fn day_five() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d5_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A fifth unbroken run, the weekday after the weekend: 2,425 trades and five
/// times day four's volume, the busiest run since day two.
#[test]
fn day_five_is_its_own_unbroken_run_on_a_weekday() {
    let day_four = day_four();
    let day_five = day_five();
    let ticks: Vec<&Tick> = day_five.iter().flatten().collect();
    assert_eq!(ticks.len(), 2425);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_four = day_four.iter().flatten().map(|t| t.ts_ns).max().unwrap();
    assert!(ticks[0].ts_ns - last_day_four > 20 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 22.7 && volume < 22.8);
}

/// Day five's newest session is inert at the tightest cap at every share, so
/// that cap reads the same at every share's newest step; the other four move
/// with the share. One share settles the inert cap, and none settles the rest.
#[test]
fn on_day_five_only_the_inert_cap_holds_across_shares() {
    let report = share_sweep(
        &day_five(),
        BUCKET,
        &[0.05, 0.085, 0.15, 0.3, 0.6],
        &CAPS,
        &HALF_LIVES,
        25.0,
        5.0,
    )
    .unwrap();
    assert_eq!(report.held_caps, 0);
    assert_eq!(report.settled_at, [1, 0, 0, 0, 0]);
    assert_eq!(report.last_share_stable, [true, false, false, false, false]);
    for row in &report.shares {
        assert_eq!(row.last_verdicts[0], "inert");
    }
}

/// Thirty seconds of the fifty-level book at the full update rate, with copies
/// of the previous snapshot left out, beside the five-second grid `--every 5`
/// would have kept from it. The grid keeps the depth and the curve: the
/// coefficient moves under 4%. It loses the top of book, which moves faster
/// than five seconds: the spread averages a quarter of the full-rate figure,
/// and the queue imbalance changes sign.
#[test]
fn a_five_second_grid_keeps_the_curve_and_loses_the_top_of_book() {
    let book = read_book("data/real/btc_usd_d5_book_full.ndjson").expect("recorded book must load");
    let mut snapshots: std::collections::BTreeMap<i64, Vec<(String, i64, u64, u64)>> =
        Default::default();
    for l in &book {
        let row = (l.side.clone(), l.level, l.price.to_bits(), l.size.to_bits());
        snapshots.entry(l.ts_ns).or_default().push(row);
    }
    assert_eq!(snapshots.len(), 330);
    let written: Vec<_> = snapshots.values().collect();
    assert!(written.windows(2).all(|pair| pair[0] != pair[1]));

    let mut kept = Vec::new();
    for &ts in snapshots.keys() {
        if kept.last().is_none_or(|&last| ts - last >= 5_000_000_000) {
            kept.push(ts);
        }
    }
    let grid: Vec<_> = book
        .iter()
        .filter(|l| kept.contains(&l.ts_ns))
        .cloned()
        .collect();
    assert_eq!(kept.len(), 6);

    let sizes = [0.01, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0];
    let full = sweep_curve(&book, "BTC-USD", "buy", &sizes).unwrap();
    let thin = sweep_curve(&grid, "BTC-USD", "buy", &sizes).unwrap();
    assert_eq!((full.fitted_points, thin.fitted_points), (7, 7));
    assert!((full.coef_bps - 1.9721695).abs() < 1e-8);
    assert!((thin.coef_bps - 2.03941096).abs() < 1e-8);
    assert!((thin.coef_bps / full.coef_bps - 1.0).abs() < 0.04);

    let full_depth = depth_metrics(&book, "BTC-USD").unwrap();
    let thin_depth = depth_metrics(&grid, "BTC-USD").unwrap();
    assert!((full_depth.avg_spread - 0.0400303).abs() < 1e-8);
    assert!(full_depth.avg_spread > 4.0 * thin_depth.avg_spread);
    let full_queue = queue_metrics(&book, "BTC-USD").unwrap();
    let thin_queue = queue_metrics(&grid, "BTC-USD").unwrap();
    assert!((full_queue.avg_queue_imbalance - 0.17003025).abs() < 1e-8);
    assert!(thin_queue.avg_queue_imbalance < 0.0);

    // Monday's ask is thinner than Sunday's: 5 BTC lifts all fifty levels and
    // does not fill, while the deeper bid takes it in every snapshot.
    let lift = sweep_cost(&book, "BTC-USD", "buy", 5.0).unwrap();
    let hit = sweep_cost(&book, "BTC-USD", "sell", 5.0).unwrap();
    assert_eq!((lift.filled_snapshots, hit.filled_snapshots), (0, 330));
}

fn day_six() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d6_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A sixth unbroken run, the Tuesday after day five: 1,738 trades and under
/// sixteen BTC, quieter than Monday.
#[test]
fn day_six_is_its_own_unbroken_run() {
    let day_five = day_five();
    let day_six = day_six();
    let ticks: Vec<&Tick> = day_six.iter().flatten().collect();
    assert_eq!(ticks.len(), 1738);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_five = day_five.iter().flatten().map(|t| t.ts_ns).max().unwrap();
    assert!(ticks[0].ts_ns - last_day_five > 20 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 15.97 && volume < 15.98);
}

const GRID: [f64; 6] = [0.25, 0.5, 1.0, 2.0, 5.0, 10.0];

/// Two full-rate books, thirty seconds each, swept from a quarter second to ten.
/// No spacing keeps the touch on both days: the spread a grid sees runs more
/// than twice the book's at one spacing and under half of it at another, and
/// it does not settle as the grid tightens. The depth imbalance stays within
/// 0.035 of full rate to five seconds on both days.
#[test]
fn no_grid_spacing_keeps_the_touch_on_both_full_rate_books() {
    for day in ["d5", "d6"] {
        let book = read_book(format!("data/real/btc_usd_{day}_book_full.ndjson")).unwrap();
        let sweep = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
        let ratios: Vec<f64> = sweep.rows.iter().map(|r| r.spread_ratio).collect();
        assert!(ratios.iter().any(|&r| r > 2.0), "{day} {ratios:?}");
        assert!(ratios.iter().any(|&r| r < 0.5), "{day} {ratios:?}");
        for r in sweep.rows.iter().filter(|r| r.every_s <= 5.0) {
            let drift = r.avg_depth_imbalance - sweep.full.avg_depth_imbalance;
            assert!(drift.abs() < 0.035, "{day} {} {drift}", r.every_s);
        }
    }
}

/// Whether a grid keeps the queue's side depends on the day. Monday's
/// bid-heavy queue (+0.170) reads ask-heavy from five seconds out; Tuesday's
/// ask-heavy queue (-0.297) keeps its side at every spacing, but ten seconds
/// reads it more than twice as heavy.
#[test]
fn the_queue_keeps_its_side_on_one_day_and_loses_it_on_the_other() {
    let book = read_book("data/real/btc_usd_d5_book_full.ndjson").unwrap();
    let monday = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    let kept: Vec<usize> = monday.rows.iter().map(|r| r.snapshots).collect();
    assert_eq!(kept, [95, 53, 28, 15, 6, 3]);
    let sides: Vec<bool> = monday.rows.iter().map(|r| r.queue_sign_kept).collect();
    assert_eq!(sides, [true, true, true, true, false, false]);
    assert_eq!(monday.sign_flips, 2);
    // The five-second row is v0.47.0's hand-thinned grid, now the engine's.
    assert!((monday.rows[4].spread_ratio - 0.24981077).abs() < 1e-8);

    let book = read_book("data/real/btc_usd_d6_book_full.ndjson").unwrap();
    let tuesday = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(tuesday.full.snapshots, 440);
    assert_eq!(tuesday.sign_flips, 0);
    assert!((tuesday.full.avg_queue_imbalance + 0.29748286).abs() < 1e-8);
    assert!(tuesday.rows[5].avg_queue_imbalance < 2.0 * tuesday.full.avg_queue_imbalance);
    // Two seconds and wider only ever land on a one-cent touch.
    for r in &tuesday.rows[3..] {
        assert!((r.avg_spread - 0.01).abs() < 1e-8);
    }
}

fn day_seven() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d7_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A seventh unbroken run, the Wednesday after day six: 2,778 trades and over
/// forty-four BTC, most of it in the first three sessions.
#[test]
fn day_seven_is_its_own_unbroken_run() {
    let day_six = day_six();
    let day_seven = day_seven();
    let ticks: Vec<&Tick> = day_seven.iter().flatten().collect();
    assert_eq!(ticks.len(), 2778);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_six = day_six.iter().flatten().map(|t| t.ts_ns).max().unwrap();
    assert!(ticks[0].ts_ns - last_day_six > 20 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 44.62 && volume < 44.63);
}

fn full_rate_books(days: &[&str]) -> Vec<Vec<BookLevel>> {
    days.iter()
        .map(|d| read_book(format!("data/real/btc_usd_{d}_book_full.ndjson")).unwrap())
        .collect()
}

/// Wednesday's queue sits almost level (+0.017), so a grid flips it from a
/// quarter second out -- and its depth imbalance drifts past 0.035 at five
/// seconds, which neither earlier book did.
#[test]
fn a_near_level_queue_flips_at_a_quarter_second() {
    let book = read_book("data/real/btc_usd_d7_book_full.ndjson").unwrap();
    let wednesday = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(wednesday.full.snapshots, 490);
    assert!((wednesday.full.avg_queue_imbalance - 0.01660519).abs() < 1e-8);
    let sides: Vec<bool> = wednesday.rows.iter().map(|r| r.queue_sign_kept).collect();
    assert_eq!(sides, [false, true, false, false, true, true]);
    assert_eq!(wednesday.sign_flips, 3);
    let drift = wednesday.rows[4].avg_depth_imbalance - wednesday.full.avg_depth_imbalance;
    assert!(drift.abs() > 0.035, "{drift}");
}

/// Two books held four spacings out of six; the third holds one. A spacing
/// that keeps the queue's side on every book recorded so far is not shown to
/// keep it on the next.
#[test]
fn a_third_book_leaves_one_spacing_held_of_the_four_two_books_held() {
    let two = book_grid_across(&full_rate_books(&["d5", "d6"]), "BTC-USD", &GRID).unwrap();
    let kept: Vec<usize> = two.spacings.iter().map(|s| s.sign_kept_books).collect();
    assert_eq!(kept, [2, 2, 2, 2, 1, 1]);
    assert_eq!(two.held_spacings, 4);

    let three = book_grid_across(&full_rate_books(&["d5", "d6", "d7"]), "BTC-USD", &GRID).unwrap();
    let kept: Vec<usize> = three.spacings.iter().map(|s| s.sign_kept_books).collect();
    assert_eq!(kept, [2, 3, 2, 2, 2, 2]);
    assert_eq!(three.held_spacings, 1);
    // Even the one held spacing does not keep the touch: its spread runs high
    // on every book.
    assert!(three.spacings[1].spread_ratio_min > 1.0);
}

fn day_eight() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d8_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// An eighth unbroken run, the Thursday after day seven: 2,987 trades and
/// over thirty-three BTC, most of it in the third and fourth sessions.
#[test]
fn day_eight_is_its_own_unbroken_run() {
    let day_seven = day_seven();
    let day_eight = day_eight();
    let ticks: Vec<&Tick> = day_eight.iter().flatten().collect();
    assert_eq!(ticks.len(), 2987);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_seven = day_seven.iter().flatten().map(|t| t.ts_ns).max().unwrap();
    assert!(ticks[0].ts_ns - last_day_seven > 20 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 33.19 && volume < 33.20);
}

/// Thursday's queue rests firmly on the bid (+0.114) and keeps its side to
/// five seconds, so a fourth book takes no held spacing away. Leaving each
/// book out in turn shows where the losses came from: without Wednesday four
/// spacings hold, without any other book one or two.
#[test]
fn a_fourth_book_takes_nothing_away_and_one_book_takes_three() {
    let book = read_book("data/real/btc_usd_d8_book_full.ndjson").unwrap();
    let thursday = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(thursday.full.snapshots, 470);
    let sides: Vec<bool> = thursday.rows.iter().map(|r| r.queue_sign_kept).collect();
    assert_eq!(sides, [true, true, true, true, true, false]);
    assert_eq!(thursday.sign_flips, 1);
    // Thursday's depth imbalance drifts past 0.035 at five seconds too.
    let drift = thursday.rows[4].avg_depth_imbalance - thursday.full.avg_depth_imbalance;
    assert!(drift.abs() > 0.035, "{drift}");

    let three = book_grid_across(&full_rate_books(&["d5", "d6", "d7"]), "BTC-USD", &GRID).unwrap();
    assert_eq!(three.held_without, [3, 1, 4]);
    let four = book_grid_across(
        &full_rate_books(&["d5", "d6", "d7", "d8"]),
        "BTC-USD",
        &GRID,
    )
    .unwrap();
    let kept: Vec<usize> = four.spacings.iter().map(|s| s.sign_kept_books).collect();
    assert_eq!(kept, [3, 4, 3, 3, 3, 2]);
    assert_eq!(four.held_spacings, three.held_spacings);
    assert_eq!(four.held_without, [2, 1, 4, 1]);
}

fn day_nine() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d9_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A ninth unbroken run, a week after day eight: 2,355 trades and over thirty
/// BTC, more than half of it in the last session alone.
#[test]
fn day_nine_is_its_own_unbroken_run() {
    let day_eight = day_eight();
    let day_nine = day_nine();
    let ticks: Vec<&Tick> = day_nine.iter().flatten().collect();
    assert_eq!(ticks.len(), 2355);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_eight = day_eight.iter().flatten().map(|t| t.ts_ns).max().unwrap();
    assert!(ticks[0].ts_ns - last_day_eight > 6 * 24 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 30.73 && volume < 30.74);
    let last: f64 = day_nine[5].iter().map(|t| t.size).sum();
    assert!(last > volume / 2.0);
}

/// The second Thursday's queue rests deep on the ask (-0.536) and is the first
/// book to keep its side at every spacing, so a fifth book takes nothing away:
/// one spacing still holds. `sign_lost_by` names who breaks the rest -- the
/// Wednesday book alone at 0.25, 1 and 2 s, the Monday book alone at 5 s, and
/// two books at 10 s, which is why no single absence frees that one.
#[test]
fn a_book_that_never_flips_still_holds_one_spacing_and_the_losers_are_named() {
    let book = read_book("data/real/btc_usd_d9_book_full.ndjson").unwrap();
    let second_thursday = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(second_thursday.full.snapshots, 393);
    assert!(second_thursday.full.avg_queue_imbalance < -0.5);
    assert!(second_thursday.rows.iter().all(|r| r.queue_sign_kept));
    assert_eq!(second_thursday.sign_flips, 0);
    // A one-tick spread throughout, so every grid keeps the touch here.
    assert!(second_thursday.rows.iter().all(|r| r.spread_ratio == 1.0));

    let five = book_grid_across(
        &full_rate_books(&["d5", "d6", "d7", "d8", "d9"]),
        "BTC-USD",
        &GRID,
    )
    .unwrap();
    let lost: Vec<Vec<usize>> = five
        .spacings
        .iter()
        .map(|s| s.sign_lost_by.clone())
        .collect();
    assert_eq!(
        lost,
        [vec![2], vec![], vec![2], vec![2], vec![0], vec![0, 3]]
    );
    assert_eq!(five.held_spacings, 1);
    assert_eq!(five.held_without, [2, 1, 4, 1, 1]);
}

fn day_ten() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d10_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A tenth unbroken run, the morning after day nine: 3,042 trades and the same
/// thirty BTC, but spread out -- no session holds a third of it, where day
/// nine's last session held more than half.
#[test]
fn day_ten_is_its_own_unbroken_run() {
    let day_nine = day_nine();
    let day_ten = day_ten();
    let ticks: Vec<&Tick> = day_ten.iter().flatten().collect();
    assert_eq!(ticks.len(), 3042);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_nine = day_nine.iter().flatten().map(|t| t.ts_ns).max().unwrap();
    assert!(ticks[0].ts_ns - last_day_nine > 23 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 30.63 && volume < 30.65);
    for session in &day_ten {
        let v: f64 = session.iter().map(|t| t.size).sum();
        assert!(v < volume / 3.0);
    }
}

/// The Friday's queue rests on the bid (+0.489) and keeps its side at every
/// spacing, the second book in a row to do so, but unlike day nine it loses the
/// touch: five seconds lands on one-cent spreads against 0.13 at full rate. A
/// sixth book takes nothing away.
#[test]
fn a_sixth_book_keeps_its_side_and_loses_its_touch() {
    let book = read_book("data/real/btc_usd_d10_book_full.ndjson").unwrap();
    let friday = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(friday.full.snapshots, 356);
    assert!(friday.full.avg_queue_imbalance > 0.4);
    assert_eq!(friday.sign_flips, 0);
    assert!(friday.rows[4].spread_ratio < 0.1);

    let six = book_grid_across(
        &full_rate_books(&["d5", "d6", "d7", "d8", "d9", "d10"]),
        "BTC-USD",
        &GRID,
    )
    .unwrap();
    assert_eq!(six.held_spacings, 1);
    assert_eq!(six.held_without, [2, 1, 4, 1, 1, 1]);
}

/// Each book at ten starts per spacing. Eleven of the thirty-six book-and-spacing
/// verdicts depend on where the grid began, and three that `book-grid` reports as
/// flips are kept by most starts. Only the half second holds at every start on
/// every book. Day seven's quarter second is the one thing the start cannot move.
#[test]
fn where_a_grid_starts_decides_eleven_of_thirty_six_verdicts() {
    let names = ["d5", "d6", "d7", "d8", "d9", "d10"];
    let books = full_rate_books(&names);
    let runs: Vec<_> = books
        .iter()
        .map(|b| book_grid_phase(b, "BTC-USD", &GRID, 10).unwrap())
        .collect();
    let kept: Vec<Vec<usize>> = runs
        .iter()
        .map(|p| p.spacings.iter().map(|s| s.sign_kept_phases).collect())
        .collect();
    assert_eq!(
        kept,
        [
            vec![10, 10, 10, 10, 7, 6],
            vec![10, 10, 10, 10, 10, 8],
            vec![0, 10, 4, 3, 6, 7],
            vec![10, 10, 9, 9, 6, 7],
            vec![10; 6],
            vec![10; 6],
        ]
    );
    let counts: Vec<(usize, usize, usize)> = runs
        .iter()
        .map(|p| (p.held_spacings, p.lost_spacings, p.split_spacings))
        .collect();
    assert_eq!(
        counts,
        [
            (4, 0, 2),
            (5, 0, 1),
            (1, 1, 4),
            (2, 0, 4),
            (6, 0, 0),
            (6, 0, 0)
        ]
    );
    // Flipped at start 0, kept by most starts: day five at 5 s and 10 s, day
    // eight at 10 s.
    let mut misread = Vec::new();
    for (name, p) in names.iter().zip(&runs) {
        for s in &p.spacings {
            if !s.first_phase_kept && s.sign_kept_phases * 2 > s.phases_run {
                misread.push((*name, s.every_s));
            }
        }
    }
    assert_eq!(misread, [("d5", 5.0), ("d5", 10.0), ("d8", 10.0)]);
    let held_everywhere: Vec<f64> = (0..GRID.len())
        .filter(|&i| runs.iter().all(|p| p.spacings[i].sign_kept_phases == 10))
        .map(|i| GRID[i])
        .collect();
    assert_eq!(held_everywhere, [0.5]);
}

fn day_eleven() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d11_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// An eleventh unbroken run, the Monday after day ten's Friday: 1,882 trades and
/// under half day ten's volume, with one session holding more than a third.
#[test]
fn day_eleven_is_its_own_unbroken_run_after_a_weekend() {
    let day_ten = day_ten();
    let day_eleven = day_eleven();
    let ticks: Vec<&Tick> = day_eleven.iter().flatten().collect();
    assert_eq!(ticks.len(), 1882);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_ten = day_ten.iter().flatten().map(|t| t.ts_ns).max().unwrap();
    assert!(ticks[0].ts_ns - last_day_ten > 72 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 12.72 && volume < 12.73);
    let fifth: f64 = day_eleven[4].iter().map(|t| t.size).sum();
    assert!(fifth > volume / 3.0);
}

/// The Monday's queue rests on the ask (-0.699) and keeps its side at every
/// spacing, the third book in a row to do so. Its grids see a tighter touch than
/// the book had: from half a second on, the spread is 0.58 of the full-rate one.
/// A seventh book takes nothing away.
#[test]
fn a_seventh_book_keeps_its_side_and_sees_a_tighter_touch() {
    let book = read_book("data/real/btc_usd_d11_book_full.ndjson").unwrap();
    let monday = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(monday.full.snapshots, 270);
    assert!(monday.full.avg_queue_imbalance < -0.6);
    assert_eq!(monday.sign_flips, 0);
    for r in &monday.rows[1..] {
        assert!((r.spread_ratio - 0.58064524).abs() < 1e-8);
    }

    let seven = book_grid_across(
        &full_rate_books(&["d5", "d6", "d7", "d8", "d9", "d10", "d11"]),
        "BTC-USD",
        &GRID,
    )
    .unwrap();
    assert_eq!(seven.held_spacings, 1);
    assert_eq!(seven.held_without, [2, 1, 4, 1, 1, 1, 1]);
}

/// Seven books at ten starts. The half second still holds on all of them, so the
/// count `book-grid-across` gives survives. What it says about each book does not:
/// read at every start, day seven alone takes one spacing away, not three, and
/// day five none. Six book-and-spacing verdicts held at start 0 split elsewhere.
#[test]
fn read_at_every_start_one_book_takes_one_spacing_away_not_three() {
    let books = full_rate_books(&["d5", "d6", "d7", "d8", "d9", "d10", "d11"]);
    let first = book_grid_across(&books, "BTC-USD", &GRID).unwrap();
    let every = book_grid_phase_across(&books, "BTC-USD", &GRID, 10).unwrap();
    assert_eq!(every.held_spacings, 1);
    assert_eq!(every.first_phase_held_spacings, first.held_spacings);
    assert_eq!(first.held_without, [2, 1, 4, 1, 1, 1, 1]);
    assert_eq!(every.held_without, [1, 1, 2, 1, 1, 1, 1]);
    let held: Vec<usize> = every.spacings.iter().map(|s| s.held_books).collect();
    let at_start: Vec<usize> = every.spacings.iter().map(|s| s.first_phase_books).collect();
    assert_eq!(held, [6, 7, 5, 5, 4, 3]);
    assert_eq!(at_start, [6, 7, 6, 6, 6, 5]);
    let lost: Vec<&Vec<usize>> = every.spacings.iter().map(|s| &s.lost_by).collect();
    assert_eq!(
        lost,
        [&vec![2], &vec![], &vec![], &vec![], &vec![], &vec![]]
    );
    let split: Vec<&Vec<usize>> = every.spacings.iter().map(|s| &s.split_by).collect();
    assert_eq!(
        split,
        [
            &vec![],
            &vec![],
            &vec![2, 3],
            &vec![2, 3],
            &vec![0, 2, 3],
            &vec![0, 1, 2, 3]
        ]
    );
}

fn day_twelve() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d12_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A twelfth unbroken run and the first recorded in the evening, the day after
/// day eleven's morning: 3,049 trades, three and a half times day eleven's volume,
/// with the first session holding more than a third.
#[test]
fn day_twelve_is_its_own_unbroken_run_in_an_evening() {
    let day_eleven = day_eleven();
    let day_twelve = day_twelve();
    let ticks: Vec<&Tick> = day_twelve.iter().flatten().collect();
    assert_eq!(ticks.len(), 3049);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_eleven = day_eleven.iter().flatten().map(|t| t.ts_ns).max().unwrap();
    assert!(ticks[0].ts_ns - last_day_eleven > 24 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 44.22 && volume < 44.23);
    let first: f64 = day_twelve[0].iter().map(|t| t.size).sum();
    assert!(first > volume / 3.0);
}

/// The evening queue is nearly flat (-0.039) and flips once, at 5 s. An eighth
/// book still leaves the half second held, and takes nothing away on its own.
#[test]
fn an_eighth_book_with_a_flat_queue_flips_once_and_takes_nothing_away() {
    let book = read_book("data/real/btc_usd_d12_book_full.ndjson").unwrap();
    let evening = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(evening.full.snapshots, 445);
    assert!(evening.full.avg_queue_imbalance.abs() < 0.05);
    assert_eq!(evening.sign_flips, 1);

    let eight = book_grid_across(
        &full_rate_books(&["d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12"]),
        "BTC-USD",
        &GRID,
    )
    .unwrap();
    assert_eq!(eight.held_spacings, 1);
    assert_eq!(eight.held_without, [1, 1, 4, 1, 1, 1, 1, 1]);
}

/// Eight books at ten starts, counted in (book, start) pairs. Only the half
/// second keeps the side at all 80, but no split spacing is near lost: the
/// least kept, 10 s, still keeps it at 64. A split verdict here reads as
/// mostly held, not as a coin toss.
#[test]
fn every_split_spacing_is_kept_by_most_book_and_start_pairs() {
    let books = full_rate_books(&["d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12"]);
    let every = book_grid_phase_across(&books, "BTC-USD", &GRID, 10).unwrap();
    assert_eq!(every.held_spacings, 1);
    assert_eq!(every.held_without, [1, 1, 2, 1, 1, 1, 1, 1]);
    let kept: Vec<usize> = every.spacings.iter().map(|s| s.kept_starts).collect();
    assert_eq!(kept, [70, 80, 68, 67, 65, 64]);
    for s in &every.spacings {
        assert_eq!(s.starts_run, 80);
        assert!(s.kept_starts * 5 >= s.starts_run * 4);
    }
    assert_eq!(every.spacings[5].split_by, vec![0, 1, 2, 3, 7]);
}

fn day_thirteen() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d13_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A thirteenth unbroken run, the next late afternoon: 5,228 trades, more than
/// double day twelve's volume and just above day two's, the most so far.
#[test]
fn day_thirteen_is_its_own_unbroken_run_with_the_most_volume_yet() {
    let day_twelve = day_twelve();
    let day_thirteen = day_thirteen();
    let ticks: Vec<&Tick> = day_thirteen.iter().flatten().collect();
    assert_eq!(ticks.len(), 5228);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_twelve = day_twelve.iter().flatten().map(|t| t.ts_ns).max().unwrap();
    assert!(ticks[0].ts_ns - last_day_twelve > 20 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 89.84 && volume < 89.85);
    let twelve: f64 = day_twelve.iter().flatten().map(|t| t.size).sum();
    assert!(volume > 2.0 * twelve);
}

/// The ninth book leans firmly to the ask side (-0.43) and never flips: every
/// spacing at every start keeps its side, so it takes nothing away.
#[test]
fn a_ninth_book_keeps_its_side_at_every_spacing_and_every_start() {
    let book = read_book("data/real/btc_usd_d13_book_full.ndjson").unwrap();
    let sweep = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(sweep.full.snapshots, 475);
    assert!(sweep.full.avg_queue_imbalance < -0.4);
    assert_eq!(sweep.sign_flips, 0);
    let phase = book_grid_phase(&book, "BTC-USD", &GRID, 10).unwrap();
    assert!(phase
        .spacings
        .iter()
        .all(|s| s.sign_kept_phases == 10 && s.phases_run == 10));

    let nine = book_grid_across(&full_rate_books(&NINE), "BTC-USD", &GRID).unwrap();
    assert_eq!(nine.held_spacings, 1);
    assert_eq!(nine.held_without, [1, 1, 4, 1, 1, 1, 1, 1, 1]);
}

const NINE: [&str; 9] = ["d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12", "d13"];

/// Nine books at ten starts. The totals read every split as mostly kept, but
/// day seven (book 2) misses the most starts at four of the five split
/// spacings, and at 2 s keeps only 3 of its 10. Whether it misses alone is
/// `next_most_missed`'s to say (below): mostly it does not.
#[test]
fn the_weakest_book_behind_a_mostly_kept_spacing_is_day_seven() {
    let every = book_grid_phase_across(&full_rate_books(&NINE), "BTC-USD", &GRID, 10).unwrap();
    assert_eq!(every.held_spacings, 1);
    assert_eq!(every.held_without, [1, 1, 2, 1, 1, 1, 1, 1, 1]);
    let kept: Vec<usize> = every.spacings.iter().map(|s| s.kept_starts).collect();
    assert_eq!(kept, [80, 90, 78, 77, 75, 74]);
    let most: Vec<(usize, usize)> = every
        .spacings
        .iter()
        .map(|s| (s.most_missed, s.most_missed_book))
        .collect();
    assert_eq!(most, [(10, 2), (0, 0), (6, 2), (7, 2), (4, 2), (4, 0)]);
    // Mostly kept in total, yet one book loses the side at most of its starts.
    let two = &every.spacings[3];
    assert!(two.kept_starts * 5 >= two.starts_run * 4);
    assert!(two.most_missed * 2 > every.books[2].spacings[3].phases_run);
}

fn day_fourteen() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d14_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A fourteenth unbroken run, the first late morning: 3,288 trades, the next
/// day, at under a third of day thirteen's volume.
#[test]
fn day_fourteen_is_its_own_unbroken_run_in_a_late_morning() {
    let day_thirteen = day_thirteen();
    let day_fourteen = day_fourteen();
    let ticks: Vec<&Tick> = day_fourteen.iter().flatten().collect();
    assert_eq!(ticks.len(), 3288);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_thirteen = day_thirteen
        .iter()
        .flatten()
        .map(|t| t.ts_ns)
        .max()
        .unwrap();
    assert!(ticks[0].ts_ns - last_day_thirteen > 18 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 26.95 && volume < 26.96);
    let thirteen: f64 = day_thirteen.iter().flatten().map(|t| t.size).sum();
    assert!(volume * 3.0 < thirteen);
}

const TEN: [&str; 10] = [
    "d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12", "d13", "d14",
];

/// The tenth book leans to the bid (+0.07), where the ninth leaned to the ask,
/// and its first-start grid never flips. Started ten ways it still loses the
/// side at some starts from 2 s, yet takes no spacing away from the rest.
#[test]
fn a_tenth_book_leans_to_the_bid_and_splits_only_from_two_seconds() {
    let book = read_book("data/real/btc_usd_d14_book_full.ndjson").unwrap();
    let sweep = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(sweep.full.snapshots, 362);
    assert!(sweep.full.avg_queue_imbalance > 0.05);
    assert_eq!(sweep.sign_flips, 0);
    let phase = book_grid_phase(&book, "BTC-USD", &GRID, 10).unwrap();
    let kept: Vec<usize> = phase.spacings.iter().map(|s| s.sign_kept_phases).collect();
    assert_eq!(kept, [10, 10, 10, 9, 9, 6]);

    let ten = book_grid_across(&full_rate_books(&TEN), "BTC-USD", &GRID).unwrap();
    assert_eq!(ten.held_spacings, 1);
    assert_eq!(ten.held_without, [1, 1, 4, 1, 1, 1, 1, 1, 1, 1]);
}

/// The weakest book stands alone only where it loses every start. On nine
/// books day seven misses all ten at 0.25 s and no other book misses one, but
/// at 1 s and 2 s a runner-up misses five, and at 5 s and 10 s it ties. A
/// tenth book changes none of that.
#[test]
fn the_weakest_book_stands_alone_only_where_it_loses_every_start() {
    let read = |days: &[&str]| -> Vec<(usize, usize, usize)> {
        book_grid_phase_across(&full_rate_books(days), "BTC-USD", &GRID, 10)
            .unwrap()
            .spacings
            .iter()
            .map(|s| (s.most_missed, s.most_missed_book, s.next_most_missed))
            .collect()
    };
    assert_eq!(
        read(&NINE),
        [
            (10, 2, 0),
            (0, 0, 0),
            (6, 2, 5),
            (7, 2, 5),
            (4, 2, 4),
            (4, 0, 4)
        ]
    );
    assert_eq!(
        read(&TEN),
        [
            (10, 2, 0),
            (0, 0, 0),
            (6, 2, 5),
            (7, 2, 5),
            (4, 2, 4),
            (4, 0, 4)
        ]
    );
    let every = book_grid_phase_across(&full_rate_books(&TEN), "BTC-USD", &GRID, 10).unwrap();
    assert_eq!(every.held_spacings, 1);
    assert_eq!(every.held_without, [1, 1, 2, 1, 1, 1, 1, 1, 1, 1]);
    let kept: Vec<usize> = every.spacings.iter().map(|s| s.kept_starts).collect();
    assert_eq!(kept, [90, 100, 88, 86, 84, 80]);
    // Alone at 0.25 s: every miss there is the weakest book's.
    let quarter = &every.spacings[0];
    assert_eq!(
        quarter.starts_run - quarter.kept_starts,
        quarter.most_missed
    );
    assert_eq!(quarter.lost_by, vec![2]);
}

fn day_fifteen() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d15_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A fifteenth unbroken run, the Monday after: 2,794 trades, more than three
/// days after day fourteen, at about the same volume.
#[test]
fn day_fifteen_is_its_own_unbroken_run_after_a_weekend() {
    let day_fourteen = day_fourteen();
    let day_fifteen = day_fifteen();
    let ticks: Vec<&Tick> = day_fifteen.iter().flatten().collect();
    assert_eq!(ticks.len(), 2794);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_fourteen = day_fourteen
        .iter()
        .flatten()
        .map(|t| t.ts_ns)
        .max()
        .unwrap();
    assert!(ticks[0].ts_ns - last_day_fourteen > 90 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 27.29 && volume < 27.30);
    let fourteen: f64 = day_fourteen.iter().flatten().map(|t| t.size).sum();
    assert!((volume - fourteen).abs() < 0.5);
}

const ELEVEN: [&str; 11] = [
    "d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12", "d13", "d14", "d15",
];

/// The eleventh book leans to the bid (+0.14), as the tenth did, and its
/// first-start grid never flips. Started ten ways it loses the side at some
/// starts from 2 s, yet the half second still holds on every book.
#[test]
fn an_eleventh_book_leans_to_the_bid_and_takes_nothing_away() {
    let book = read_book("data/real/btc_usd_d15_book_full.ndjson").unwrap();
    let sweep = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(sweep.full.snapshots, 463);
    assert!(sweep.full.avg_queue_imbalance > 0.1);
    assert_eq!(sweep.sign_flips, 0);
    let phase = book_grid_phase(&book, "BTC-USD", &GRID, 10).unwrap();
    let kept: Vec<usize> = phase.spacings.iter().map(|s| s.sign_kept_phases).collect();
    assert_eq!(kept, [10, 10, 10, 8, 7, 6]);

    let eleven = book_grid_across(&full_rate_books(&ELEVEN), "BTC-USD", &GRID).unwrap();
    assert_eq!(eleven.held_spacings, 1);
    assert_eq!(eleven.held_without, [1, 1, 4, 1, 1, 1, 1, 1, 1, 1, 1]);
}

/// Set the weakest book aside and the rest still miss starts at every split
/// spacing. Its share of the misses falls as the spacing widens: all of them
/// at 0.25 s, half or less from 1 s, a sixth at 10 s on eleven books. Leaving
/// it out of the recording would not make a wide grid safe.
#[test]
fn the_weakest_book_takes_a_shrinking_share_of_the_misses() {
    let read = |days: &[&str]| {
        book_grid_phase_across(&full_rate_books(days), "BTC-USD", &GRID, 10).unwrap()
    };
    let rest = |every: &GridPhaseAcross| -> Vec<(usize, usize)> {
        every
            .spacings
            .iter()
            .map(|s| (s.kept_without_weakest, s.run_without_weakest))
            .collect()
    };
    assert_eq!(
        rest(&read(&TEN)),
        [(90, 90), (90, 90), (84, 90), (83, 90), (78, 90), (74, 90)]
    );
    let every = read(&ELEVEN);
    assert_eq!(
        rest(&every),
        [
            (100, 100),
            (100, 100),
            (94, 100),
            (91, 100),
            (85, 100),
            (80, 100)
        ]
    );
    assert_eq!(every.held_spacings, 1);
    assert_eq!(every.held_without, [1, 1, 2, 1, 1, 1, 1, 1, 1, 1, 1]);
    let missed: Vec<usize> = every
        .spacings
        .iter()
        .map(|s| s.starts_run - s.kept_starts)
        .collect();
    assert_eq!(missed, [10, 0, 12, 16, 19, 24]);
    let most: Vec<usize> = every.spacings.iter().map(|s| s.most_missed).collect();
    assert_eq!(most, [10, 0, 6, 7, 4, 4]);
    for (s, m) in every.spacings.iter().zip(&missed) {
        assert_eq!(
            m - (s.run_without_weakest - s.kept_without_weakest),
            s.most_missed
        );
    }
    // Alone at 0.25 s; a sixth of the misses at 10 s.
    assert_eq!(missed[0], every.spacings[0].most_missed);
    assert_eq!(every.spacings[5].most_missed * 6, missed[5]);
}

fn day_sixteen() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d16_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A sixteenth unbroken run, the next morning: 2,963 trades, under a day after
/// day fifteen, at under three fifths of its volume.
#[test]
fn day_sixteen_is_its_own_unbroken_run_the_next_morning() {
    let day_fifteen = day_fifteen();
    let day_sixteen = day_sixteen();
    let ticks: Vec<&Tick> = day_sixteen.iter().flatten().collect();
    assert_eq!(ticks.len(), 2963);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_fifteen = day_fifteen.iter().flatten().map(|t| t.ts_ns).max().unwrap();
    let gap = ticks[0].ts_ns - last_day_fifteen;
    assert!(gap > 23 * 3600 * 1_000_000_000 && gap < 24 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 16.11 && volume < 16.12);
    let fifteen: f64 = day_fifteen.iter().flatten().map(|t| t.size).sum();
    assert!(volume < 0.6 * fifteen);
}

const TWELVE: [&str; 12] = [
    "d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12", "d13", "d14", "d15", "d16",
];

/// The twelfth book leans to the bid (+0.14), as the two before it did, and its
/// first-start grid never flips. Started ten ways it loses the side at some
/// starts from 2 s, and the half second still holds on every book.
#[test]
fn a_twelfth_book_leans_to_the_bid_and_takes_nothing_away() {
    let book = read_book("data/real/btc_usd_d16_book_full.ndjson").unwrap();
    let sweep = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(sweep.full.snapshots, 466);
    assert!(sweep.full.avg_queue_imbalance > 0.1);
    assert_eq!(sweep.sign_flips, 0);
    let phase = book_grid_phase(&book, "BTC-USD", &GRID, 10).unwrap();
    let kept: Vec<usize> = phase.spacings.iter().map(|s| s.sign_kept_phases).collect();
    assert_eq!(kept, [10, 10, 10, 9, 7, 6]);

    let twelve = book_grid_across(&full_rate_books(&TWELVE), "BTC-USD", &GRID).unwrap();
    assert_eq!(twelve.held_spacings, 1);
    assert_eq!(twelve.held_without, [1, 1, 4, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
}

/// Half the misses come from one book at 0.25 s and 1 s, and from four of
/// twelve at 10 s: a wide grid loses the side on many books a little, not on
/// one book a lot.
#[test]
fn half_the_wide_grid_misses_take_several_books() {
    let read = |days: &[&str]| {
        book_grid_phase_across(&full_rate_books(days), "BTC-USD", &GRID, 10).unwrap()
    };
    let half = |every: &GridPhaseAcross| -> Vec<usize> {
        every.spacings.iter().map(|s| s.half_missed_by).collect()
    };
    assert_eq!(half(&read(&ELEVEN)), [1, 0, 1, 2, 3, 3]);
    let every = read(&TWELVE);
    assert_eq!(half(&every), [1, 0, 1, 2, 3, 4]);
    assert_eq!(every.held_spacings, 1);
    assert_eq!(every.held_without, [1, 1, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
    let missed: Vec<usize> = every
        .spacings
        .iter()
        .map(|s| s.starts_run - s.kept_starts)
        .collect();
    assert_eq!(missed, [10, 0, 12, 17, 22, 28]);
    let most: Vec<usize> = every.spacings.iter().map(|s| s.most_missed).collect();
    assert_eq!(most, [10, 0, 6, 7, 4, 4]);
    // At 10 s even three books at the most anyone misses fall short of half.
    assert!(2 * 3 * every.spacings[5].most_missed < missed[5]);
}

fn day_seventeen() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d17_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A seventeenth unbroken run, earlier the next morning: 3,092 trades, under a
/// day after day sixteen, at nearly twice its volume.
#[test]
fn day_seventeen_is_its_own_unbroken_run_the_next_morning() {
    let day_sixteen = day_sixteen();
    let day_seventeen = day_seventeen();
    let ticks: Vec<&Tick> = day_seventeen.iter().flatten().collect();
    assert_eq!(ticks.len(), 3092);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_sixteen = day_sixteen.iter().flatten().map(|t| t.ts_ns).max().unwrap();
    let gap = ticks[0].ts_ns - last_day_sixteen;
    assert!(gap > 22 * 3600 * 1_000_000_000 && gap < 23 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 31.32 && volume < 31.33);
    let sixteen: f64 = day_sixteen.iter().flatten().map(|t| t.size).sum();
    assert!(volume > 1.9 * sixteen);
}

const THIRTEEN: [&str; 13] = [
    "d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12", "d13", "d14", "d15", "d16", "d17",
];

/// The thirteenth book leans hard to the bid (+0.64) and keeps that side at
/// every spacing from every start: it adds books and starts, not misses.
#[test]
fn a_thirteenth_book_leans_hard_to_the_bid_and_misses_nothing() {
    let book = read_book("data/real/btc_usd_d17_book_full.ndjson").unwrap();
    let sweep = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(sweep.full.snapshots, 345);
    assert!(sweep.full.avg_queue_imbalance > 0.6);
    assert_eq!(sweep.sign_flips, 0);
    let phase = book_grid_phase(&book, "BTC-USD", &GRID, 10).unwrap();
    assert!(phase.spacings.iter().all(|s| s.sign_kept_phases == 10));

    let thirteen = book_grid_across(&full_rate_books(&THIRTEEN), "BTC-USD", &GRID).unwrap();
    assert_eq!(thirteen.held_spacings, 1);
    assert_eq!(
        thirteen.held_without,
        [1, 1, 4, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]
    );
}

/// The clearest full-rate side any book loses at some start grows with the
/// spacing, to 0.30 at 10 s; the five books whose side is clearer than that
/// (0.43 and up) keep it at every spacing from every start.
#[test]
fn a_wide_grid_loses_only_the_fainter_sides() {
    let every = book_grid_phase_across(&full_rate_books(&THIRTEEN), "BTC-USD", &GRID, 10).unwrap();
    let strongest: Vec<f64> = every
        .spacings
        .iter()
        .map(|s| s.strongest_side_missed)
        .collect();
    assert_eq!(
        strongest,
        [0.01660519, 0.0, 0.11377945, 0.14396846, 0.17003025, 0.29748286]
    );
    assert!(strongest[1..].windows(2).all(|w| w[0] <= w[1]));
    let missed: Vec<usize> = every
        .spacings
        .iter()
        .map(|s| s.starts_run - s.kept_starts)
        .collect();
    assert_eq!(missed, [10, 0, 12, 17, 22, 28]);
    let clear: Vec<usize> = (0..THIRTEEN.len())
        .filter(|&j| every.books[j].full.avg_queue_imbalance.abs() > strongest[5])
        .collect();
    assert_eq!(clear, [4, 5, 6, 8, 12]);
    for &j in &clear {
        assert!(every.books[j].full.avg_queue_imbalance.abs() > 0.42);
        assert!(every.books[j]
            .spacings
            .iter()
            .all(|s| s.sign_kept_phases == s.phases_run));
    }
}

fn day_eighteen() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d18_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// An eighteenth unbroken run, a little earlier the next morning: 3,003 trades,
/// under a day after day seventeen, at well under half its volume.
#[test]
fn day_eighteen_is_its_own_unbroken_run_the_next_morning() {
    let day_seventeen = day_seventeen();
    let day_eighteen = day_eighteen();
    let ticks: Vec<&Tick> = day_eighteen.iter().flatten().collect();
    assert_eq!(ticks.len(), 3003);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_seventeen = day_seventeen
        .iter()
        .flatten()
        .map(|t| t.ts_ns)
        .max()
        .unwrap();
    let gap = ticks[0].ts_ns - last_day_seventeen;
    assert!(gap > 23 * 3600 * 1_000_000_000 && gap < 24 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 12.80 && volume < 12.81);
    let seventeen: f64 = day_seventeen.iter().flatten().map(|t| t.size).sum();
    assert!(volume < 0.5 * seventeen);
}

const FOURTEEN: [&str; 14] = [
    "d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12", "d13", "d14", "d15", "d16", "d17", "d18",
];

/// The fourteenth book leans to the bid too (+0.54) and, like the thirteenth,
/// keeps that side at every spacing from every start.
#[test]
fn a_fourteenth_book_leans_to_the_bid_and_misses_nothing() {
    let book = read_book("data/real/btc_usd_d18_book_full.ndjson").unwrap();
    let sweep = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(sweep.full.snapshots, 307);
    assert!(sweep.full.avg_queue_imbalance > 0.5);
    assert_eq!(sweep.sign_flips, 0);
    let phase = book_grid_phase(&book, "BTC-USD", &GRID, 10).unwrap();
    assert!(phase.spacings.iter().all(|s| s.sign_kept_phases == 10));

    let fourteen = book_grid_across(&full_rate_books(&FOURTEEN), "BTC-USD", &GRID).unwrap();
    assert_eq!(fourteen.held_spacings, 1);
    assert_eq!(
        fourteen.held_without,
        [1, 1, 4, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]
    );
}

/// At ten starts the faintest side any book keeps is clearer than the
/// strongest any book loses at every spacing but 1 s, where day fourteen
/// (0.07) holds and day eight (0.11) does not. From 2 s each spacing's
/// faintest kept side is the next one's strongest lost. Four starts blur it.
#[test]
fn side_strength_separates_held_from_missed_at_all_but_one_spacing() {
    let books = full_rate_books(&FOURTEEN);
    let every = book_grid_phase_across(&books, "BTC-USD", &GRID, 10).unwrap();
    let faintest: Vec<f64> = every
        .spacings
        .iter()
        .map(|s| s.faintest_side_held)
        .collect();
    assert_eq!(
        faintest,
        [0.03946719, 0.01660519, 0.07168341, 0.17003025, 0.29748286, 0.42541018]
    );
    let strongest: Vec<f64> = every
        .spacings
        .iter()
        .map(|s| s.strongest_side_missed)
        .collect();
    assert_eq!(
        strongest,
        [0.01660519, 0.0, 0.11377945, 0.14396846, 0.17003025, 0.29748286]
    );
    let blurred: Vec<f64> = (0..GRID.len())
        .filter(|&i| faintest[i] <= strongest[i])
        .map(|i| GRID[i])
        .collect();
    assert_eq!(blurred, [1.0]);
    assert!(every.spacings[2].split_by.contains(&3));
    assert!(!every.spacings[2].split_by.contains(&9));
    assert_eq!(faintest[3], strongest[4]);
    assert_eq!(faintest[4], strongest[5]);

    let four = book_grid_phase_across(&books, "BTC-USD", &GRID, 4).unwrap();
    let blurred_at_four: Vec<f64> = four
        .spacings
        .iter()
        .filter(|s| s.faintest_side_held <= s.strongest_side_missed)
        .map(|s| s.every_s)
        .collect();
    assert_eq!(blurred_at_four, [1.0, 2.0, 5.0]);
}

fn day_nineteen() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d19_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A nineteenth unbroken run, a little earlier again the next morning (a
/// Friday): 6,548 trades, under a day after day eighteen, at over six times its
/// volume -- 87.40 BTC, second only to day thirteen's 89.85.
#[test]
fn day_nineteen_is_its_own_unbroken_run_the_next_morning() {
    let day_eighteen = day_eighteen();
    let day_nineteen = day_nineteen();
    let ticks: Vec<&Tick> = day_nineteen.iter().flatten().collect();
    assert_eq!(ticks.len(), 6548);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_eighteen = day_eighteen
        .iter()
        .flatten()
        .map(|t| t.ts_ns)
        .max()
        .unwrap();
    let gap = ticks[0].ts_ns - last_day_eighteen;
    assert!(gap > 23 * 3600 * 1_000_000_000 && gap < 24 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 87.39 && volume < 87.40);
    let eighteen: f64 = day_eighteen.iter().flatten().map(|t| t.size).sum();
    assert!(volume > 6.0 * eighteen);
}

const FIFTEEN: [&str; 15] = [
    "d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12", "d13", "d14", "d15", "d16", "d17", "d18",
    "d19",
];

/// The fifteenth book leans to the ask (-0.33), keeps that side at every
/// spacing from every start but 10 s, where two of ten starts lose it.
#[test]
fn a_fifteenth_book_leans_to_the_ask_and_misses_only_at_ten_seconds() {
    let book = read_book("data/real/btc_usd_d19_book_full.ndjson").unwrap();
    let sweep = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(sweep.full.snapshots, 406);
    assert!(sweep.full.avg_queue_imbalance < -0.32 && sweep.full.avg_queue_imbalance > -0.33);
    assert_eq!(sweep.sign_flips, 0);
    let phase = book_grid_phase(&book, "BTC-USD", &GRID, 10).unwrap();
    let kept: Vec<usize> = phase.spacings.iter().map(|s| s.sign_kept_phases).collect();
    assert_eq!(kept, [10, 10, 10, 10, 10, 8]);

    let fifteen = book_grid_across(&full_rate_books(&FIFTEEN), "BTC-USD", &GRID).unwrap();
    assert_eq!(fifteen.held_spacings, 1);
    assert_eq!(
        fifteen.held_without,
        [1, 1, 4, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]
    );
}

/// At ten starts the 1 s overlap is one book, day fourteen; the fifteenth
/// book's 10 s miss raises the strongest side lost there to 0.33, still under
/// the faintest kept (0.43). Four starts blur three spacings by 3, 2 and 1 books.
#[test]
fn held_no_clearer_puts_one_book_in_the_one_second_overlap() {
    let books = full_rate_books(&FIFTEEN);
    let every = book_grid_phase_across(&books, "BTC-USD", &GRID, 10).unwrap();
    let no_clearer: Vec<usize> = every.spacings.iter().map(|s| s.held_no_clearer).collect();
    assert_eq!(no_clearer, [0, 0, 1, 0, 0, 0]);
    let one_s = &every.spacings[2];
    let d14 = &every.books[9];
    assert_eq!(d14.spacings[2].sign_kept_phases, d14.spacings[2].phases_run);
    assert!(d14.full.avg_queue_imbalance.abs() <= one_s.strongest_side_missed);
    assert!(every.spacings[5].split_by.contains(&14));
    assert_eq!(every.spacings[5].strongest_side_missed, 0.3276542);
    assert_eq!(every.spacings[5].faintest_side_held, 0.42541018);

    let four = book_grid_phase_across(&books, "BTC-USD", &GRID, 4).unwrap();
    let no_clearer_at_four: Vec<usize> = four.spacings.iter().map(|s| s.held_no_clearer).collect();
    assert_eq!(no_clearer_at_four, [0, 0, 3, 2, 1, 0]);
}

fn day_twenty() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d20_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A twentieth unbroken run, the Monday morning after day nineteen, across the
/// weekend: 9,128 trades and 97.46 BTC, the most of either on any day so far.
#[test]
fn day_twenty_is_its_own_unbroken_run_after_the_weekend() {
    let day_nineteen = day_nineteen();
    let day_twenty = day_twenty();
    let ticks: Vec<&Tick> = day_twenty.iter().flatten().collect();
    assert_eq!(ticks.len(), 9128);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_nineteen = day_nineteen
        .iter()
        .flatten()
        .map(|t| t.ts_ns)
        .max()
        .unwrap();
    let gap = ticks[0].ts_ns - last_day_nineteen;
    assert!(gap > 71 * 3600 * 1_000_000_000 && gap < 72 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 97.46 && volume < 97.47);
    let nineteen: f64 = day_nineteen.iter().flatten().map(|t| t.size).sum();
    assert!(volume > nineteen);
}

const SIXTEEN: [&str; 16] = [
    "d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12", "d13", "d14", "d15", "d16", "d17", "d18",
    "d19", "d20",
];

/// The sixteenth book leans hard to the ask (-0.63, the third-clearest side of
/// any book, after days eleven and seventeen) and still loses that side at one
/// of ten 10 s starts.
#[test]
fn a_sixteenth_book_leans_hard_to_the_ask_and_still_misses_at_ten_seconds() {
    let book = read_book("data/real/btc_usd_d20_book_full.ndjson").unwrap();
    let sweep = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(sweep.full.snapshots, 429);
    assert!(sweep.full.avg_queue_imbalance < -0.62 && sweep.full.avg_queue_imbalance > -0.63);
    assert_eq!(sweep.sign_flips, 0);
    let phase = book_grid_phase(&book, "BTC-USD", &GRID, 10).unwrap();
    let kept: Vec<usize> = phase.spacings.iter().map(|s| s.sign_kept_phases).collect();
    assert_eq!(kept, [10, 10, 10, 10, 10, 9]);

    let sixteen = book_grid_across(&full_rate_books(&SIXTEEN), "BTC-USD", &GRID).unwrap();
    assert_eq!(sixteen.held_spacings, 1);
    assert_eq!(
        sixteen.held_without,
        [1, 1, 4, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]
    );
}

/// Side strength no longer separates at 10 s: day twenty, lost at one start,
/// is clearer (0.63) than four books kept at every start. missed_no_fainter
/// names that side of the overlap: one lost book there, one at 1 s (day eight
/// against day fourteen). At four starts it is 4 lost books at 5 s against 1 kept.
#[test]
fn a_clear_side_lost_at_ten_seconds_outshines_four_kept_books() {
    let books = full_rate_books(&SIXTEEN);
    let every = book_grid_phase_across(&books, "BTC-USD", &GRID, 10).unwrap();
    let no_clearer: Vec<usize> = every.spacings.iter().map(|s| s.held_no_clearer).collect();
    let no_fainter: Vec<usize> = every.spacings.iter().map(|s| s.missed_no_fainter).collect();
    assert_eq!(no_clearer, [0, 0, 1, 0, 0, 4]);
    assert_eq!(no_fainter, [0, 0, 1, 0, 0, 1]);
    let ten_s = &every.spacings[5];
    assert!(ten_s.split_by.contains(&15));
    assert_eq!(ten_s.strongest_side_missed, 0.62972122);
    assert_eq!(ten_s.faintest_side_held, 0.42541018);

    let four = book_grid_phase_across(&books, "BTC-USD", &GRID, 4).unwrap();
    let no_clearer_at_four: Vec<usize> = four.spacings.iter().map(|s| s.held_no_clearer).collect();
    let no_fainter_at_four: Vec<usize> =
        four.spacings.iter().map(|s| s.missed_no_fainter).collect();
    assert_eq!(no_clearer_at_four, [0, 0, 3, 2, 1, 0]);
    assert_eq!(no_fainter_at_four, [0, 0, 1, 2, 4, 0]);
}

fn day_twenty_one() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d21_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A twenty-first unbroken run, the Tuesday evening after day twenty: 45,718
/// trades and 315.53 BTC, about five times day twenty's trades and three times
/// its volume, the most of either on any day so far by far.
#[test]
fn day_twenty_one_is_an_evening_run_far_busier_than_any_before_it() {
    let day_twenty = day_twenty();
    let day_twenty_one = day_twenty_one();
    let ticks: Vec<&Tick> = day_twenty_one.iter().flatten().collect();
    assert_eq!(ticks.len(), 45718);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_twenty = day_twenty.iter().flatten().map(|t| t.ts_ns).max().unwrap();
    let gap = ticks[0].ts_ns - last_day_twenty;
    assert!(gap > 32 * 3600 * 1_000_000_000 && gap < 33 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 315.52 && volume < 315.53);
    let twenty: f64 = day_twenty.iter().flatten().map(|t| t.size).sum();
    assert!(volume > 3.0 * twenty);
}

const SEVENTEEN: [&str; 17] = [
    "d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12", "d13", "d14", "d15", "d16", "d17", "d18",
    "d19", "d20", "d21",
];

/// The seventeenth book leans to the ask (-0.387), never flips on its first
/// start, and loses its side at one of ten 10 s starts, as day twenty does.
#[test]
fn a_seventeenth_book_leans_to_the_ask_and_misses_once_at_ten_seconds() {
    let book = read_book("data/real/btc_usd_d21_book_full.ndjson").unwrap();
    let sweep = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(sweep.full.snapshots, 478);
    assert!(sweep.full.avg_queue_imbalance < -0.38 && sweep.full.avg_queue_imbalance > -0.39);
    assert_eq!(sweep.sign_flips, 0);
    let phase = book_grid_phase(&book, "BTC-USD", &GRID, 10).unwrap();
    let kept: Vec<usize> = phase.spacings.iter().map(|s| s.sign_kept_phases).collect();
    assert_eq!(kept, [10, 10, 10, 10, 10, 9]);

    let seventeen = book_grid_across(&full_rate_books(&SEVENTEEN), "BTC-USD", &GRID).unwrap();
    assert_eq!(seventeen.held_spacings, 1);
    assert_eq!(
        seventeen.held_without,
        [1, 1, 4, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]
    );
}

/// At ten starts the clearest side lost is a different book at every spacing
/// that loses one: day seven at 0.25 s, eight at 1 s, sixteen at 2 s, five at 5 s, twenty at
/// 10 s. Day twenty-one's 10 s miss (0.387) is fainter than every kept side, so
/// the 10 s overlap is still day twenty alone.
#[test]
fn the_clearest_side_lost_is_a_different_book_at_every_spacing() {
    let books = full_rate_books(&SEVENTEEN);
    let every = book_grid_phase_across(&books, "BTC-USD", &GRID, 10).unwrap();
    let strongest: Vec<Option<usize>> = every
        .spacings
        .iter()
        .map(|s| s.strongest_missed_book)
        .collect();
    assert_eq!(
        strongest,
        [Some(2), None, Some(3), Some(11), Some(0), Some(15)]
    );
    let ten_s = &every.spacings[5];
    assert!(ten_s.split_by.contains(&16));
    assert_eq!(ten_s.held_no_clearer, 4);
    assert_eq!(ten_s.missed_no_fainter, 1);

    let four = book_grid_phase_across(&books, "BTC-USD", &GRID, 4).unwrap();
    let strongest_at_four: Vec<Option<usize>> = four
        .spacings
        .iter()
        .map(|s| s.strongest_missed_book)
        .collect();
    assert_eq!(
        strongest_at_four,
        [Some(2), None, Some(11), Some(11), Some(0), Some(14)]
    );
    // Held at all four 10 s starts, day twenty-one is now the faintest side kept.
    assert_eq!(four.spacings[5].faintest_side_held, 0.38676146);
}

fn day_twenty_two() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d22_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A twenty-second unbroken run, the Wednesday early afternoon after day
/// twenty-one: 9,193 trades and 71.73 BTC, more trades than day twenty, the
/// busiest before twenty-one, on under a quarter of twenty-one's volume.
#[test]
fn day_twenty_two_is_an_afternoon_run_back_near_the_usual_volume() {
    let day_twenty = day_twenty();
    let day_twenty_one = day_twenty_one();
    let day_twenty_two = day_twenty_two();
    let ticks: Vec<&Tick> = day_twenty_two.iter().flatten().collect();
    assert_eq!(ticks.len(), 9193);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_twenty_one = day_twenty_one
        .iter()
        .flatten()
        .map(|t| t.ts_ns)
        .max()
        .unwrap();
    let gap = ticks[0].ts_ns - last_day_twenty_one;
    assert!(gap > 20 * 3600 * 1_000_000_000 && gap < 21 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 71.72 && volume < 71.73);
    assert!(ticks.len() > day_twenty.iter().map(Vec::len).sum::<usize>());
    let twenty_one: f64 = day_twenty_one.iter().flatten().map(|t| t.size).sum();
    assert!(volume < 0.25 * twenty_one);
}

const EIGHTEEN: [&str; 18] = [
    "d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12", "d13", "d14", "d15", "d16", "d17", "d18",
    "d19", "d20", "d21", "d22",
];

/// The eighteenth book leans to the ask (-0.575), never flips at the first
/// start, and keeps its side at all ten starts of every spacing.
#[test]
fn an_eighteenth_book_leans_to_the_ask_and_keeps_every_start() {
    let book = read_book("data/real/btc_usd_d22_book_full.ndjson").unwrap();
    let sweep = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(sweep.full.snapshots, 244);
    assert!(sweep.full.avg_queue_imbalance < -0.57 && sweep.full.avg_queue_imbalance > -0.58);
    assert_eq!(sweep.sign_flips, 0);
    let phase = book_grid_phase(&book, "BTC-USD", &GRID, 10).unwrap();
    let kept: Vec<usize> = phase.spacings.iter().map(|s| s.sign_kept_phases).collect();
    assert_eq!(kept, [10; 6]);

    let eighteen = book_grid_across(&full_rate_books(&EIGHTEEN), "BTC-USD", &GRID).unwrap();
    assert_eq!(eighteen.held_spacings, 1);
    let mut without = vec![1; 18];
    without[2] = 4;
    assert_eq!(eighteen.held_without, without);
}

/// The faintest side kept is a different book at every spacing too: at ten
/// starts day twelve at 0.25 s, seven at 0.5 s, fourteen at 1 s, five at 2 s,
/// six at 5 s, thirteen at 10 s. Days seven and five sit on both edges at
/// neighbouring spacings, and day twenty-two, kept at 10 s on a side fainter
/// than the clearest lost, makes the 10 s overlap five held books, not four.
#[test]
fn the_faintest_side_kept_is_a_different_book_at_every_spacing() {
    let books = full_rate_books(&EIGHTEEN);
    let every = book_grid_phase_across(&books, "BTC-USD", &GRID, 10).unwrap();
    let faintest: Vec<Option<usize>> = every
        .spacings
        .iter()
        .map(|s| s.faintest_held_book)
        .collect();
    assert_eq!(
        faintest,
        [Some(7), Some(2), Some(9), Some(0), Some(1), Some(8)]
    );
    // Day seven: clearest lost at 0.25 s, faintest kept at 0.5 s.
    assert_eq!(every.spacings[0].strongest_missed_book, Some(2));
    // Day five: faintest kept at 2 s, clearest lost at 5 s.
    assert_eq!(every.spacings[4].strongest_missed_book, Some(0));
    let ten_s = &every.spacings[5];
    assert_eq!(ten_s.held_books, 7);
    assert_eq!(ten_s.held_no_clearer, 5);
    assert_eq!(ten_s.missed_no_fainter, 1);

    let four = book_grid_phase_across(&books, "BTC-USD", &GRID, 4).unwrap();
    let faintest_at_four: Vec<Option<usize>> =
        four.spacings.iter().map(|s| s.faintest_held_book).collect();
    assert_eq!(
        faintest_at_four,
        [Some(7), Some(2), Some(9), Some(9), Some(9), Some(16)]
    );
}

fn day_twenty_three() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d23_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A twenty-third unbroken run, the Thursday lunchtime after day twenty-two:
/// 7,636 trades and 16.81 BTC, fewer trades than day twenty-two on under a
/// quarter of its volume.
#[test]
fn day_twenty_three_is_a_lunchtime_run_on_a_quarter_of_the_volume() {
    let day_twenty_two = day_twenty_two();
    let day_twenty_three = day_twenty_three();
    let ticks: Vec<&Tick> = day_twenty_three.iter().flatten().collect();
    assert_eq!(ticks.len(), 7636);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_twenty_two = day_twenty_two
        .iter()
        .flatten()
        .map(|t| t.ts_ns)
        .max()
        .unwrap();
    let gap = ticks[0].ts_ns - last_day_twenty_two;
    assert!(gap > 22 * 3600 * 1_000_000_000 && gap < 23 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 16.80 && volume < 16.81);
    assert!(ticks.len() < day_twenty_two.iter().map(Vec::len).sum::<usize>());
    let twenty_two: f64 = day_twenty_two.iter().flatten().map(|t| t.size).sum();
    assert!(volume < 0.25 * twenty_two);
}

const NINETEEN: [&str; 19] = [
    "d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12", "d13", "d14", "d15", "d16", "d17", "d18",
    "d19", "d20", "d21", "d22", "d23",
];

/// The nineteenth book leans faintly to the bid (+0.098), flips twice at the
/// first start, and loses its side at some starts of 2, 5 and 10 s.
#[test]
fn a_nineteenth_book_leans_faintly_to_the_bid_and_splits_three_spacings() {
    let book = read_book("data/real/btc_usd_d23_book_full.ndjson").unwrap();
    let sweep = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(sweep.full.snapshots, 439);
    assert!(sweep.full.avg_queue_imbalance > 0.09 && sweep.full.avg_queue_imbalance < 0.10);
    assert_eq!(sweep.sign_flips, 2);
    let phase = book_grid_phase(&book, "BTC-USD", &GRID, 10).unwrap();
    let kept: Vec<usize> = phase.spacings.iter().map(|s| s.sign_kept_phases).collect();
    assert_eq!(kept, [10, 10, 10, 8, 8, 7]);

    let nineteen = book_grid_across(&full_rate_books(&NINETEEN), "BTC-USD", &GRID).unwrap();
    assert_eq!(nineteen.held_spacings, 1);
    let mut without = vec![1; 19];
    without[2] = 3;
    assert_eq!(nineteen.held_without, without);
}

/// The book losing the most starts is a faint one: at ten starts day seven,
/// the faintest side of all nineteen, at every split spacing to 5 s, and day
/// five (0.170) at 10 s, where the clearest side lost is 0.630. At four starts
/// day twenty-three, itself faint, is the one losing most at 10 s.
#[test]
fn the_book_losing_the_most_starts_has_one_of_the_faintest_sides() {
    let books = full_rate_books(&NINETEEN);
    let every = book_grid_phase_across(&books, "BTC-USD", &GRID, 10).unwrap();
    let side = |j: usize| every.books[j].full.avg_queue_imbalance.abs();
    let faintest = (0..books.len()).map(side).fold(f64::INFINITY, f64::min);
    assert_eq!(side(2), faintest);
    let most: Vec<Option<f64>> = every.spacings.iter().map(|s| s.most_missed_side).collect();
    assert_eq!(
        most,
        [
            Some(faintest),
            None,
            Some(faintest),
            Some(faintest),
            Some(faintest),
            Some(side(0))
        ]
    );
    let ten_s = &every.spacings[5];
    assert!(ten_s.most_missed_side.unwrap() < ten_s.strongest_side_missed / 3.0);
    // Day twenty-three, kept at 1 s on a side fainter than the clearest lost.
    assert_eq!(every.spacings[2].held_no_clearer, 2);
    assert_eq!(ten_s.half_missed_by, 5);

    let four = book_grid_phase_across(&books, "BTC-USD", &GRID, 4).unwrap();
    assert_eq!(four.spacings[5].most_missed_book, 18);
    assert_eq!(four.spacings[5].most_missed_side, Some(side(18)));
}

fn day_twenty_four() -> Vec<Vec<Tick>> {
    (1..=6)
        .map(|i| {
            read_ticks(format!("data/real/btc_usd_d24_{i}.ndjson")).expect("real session must load")
        })
        .collect()
}

/// A twenty-fourth unbroken run, the Friday morning after day twenty-three:
/// 6,272 trades and 18.72 BTC, fewer trades than day twenty-three on more
/// volume.
#[test]
fn day_twenty_four_is_a_friday_morning_run_on_fewer_trades_and_more_volume() {
    let day_twenty_three = day_twenty_three();
    let day_twenty_four = day_twenty_four();
    let ticks: Vec<&Tick> = day_twenty_four.iter().flatten().collect();
    assert_eq!(ticks.len(), 6272);
    for pair in ticks.windows(2) {
        assert_eq!(pair[1].trade_id, pair[0].trade_id + 1);
        assert!(pair[1].ts_ns >= pair[0].ts_ns);
    }
    let last_day_twenty_three = day_twenty_three
        .iter()
        .flatten()
        .map(|t| t.ts_ns)
        .max()
        .unwrap();
    let gap = ticks[0].ts_ns - last_day_twenty_three;
    assert!(gap > 20 * 3600 * 1_000_000_000 && gap < 21 * 3600 * 1_000_000_000);
    let volume: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(volume > 18.71 && volume < 18.72);
    assert!(ticks.len() < day_twenty_three.iter().map(Vec::len).sum::<usize>());
    let twenty_three: f64 = day_twenty_three.iter().flatten().map(|t| t.size).sum();
    assert!(volume > twenty_three);
}

const TWENTY: [&str; 20] = [
    "d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12", "d13", "d14", "d15", "d16", "d17", "d18",
    "d19", "d20", "d21", "d22", "d23", "d24",
];

/// The twentieth book leans to the ask (-0.442), never flips at the first
/// start, and keeps its side at every one of ten starts at every spacing.
#[test]
fn a_twentieth_book_leans_to_the_ask_and_is_kept_at_ten_starts() {
    let book = read_book("data/real/btc_usd_d24_book_full.ndjson").unwrap();
    let sweep = book_grid_sweep(&book, "BTC-USD", &GRID).unwrap();
    assert_eq!(sweep.full.snapshots, 319);
    assert!(sweep.full.avg_queue_imbalance > -0.45 && sweep.full.avg_queue_imbalance < -0.44);
    assert_eq!(sweep.sign_flips, 0);
    let phase = book_grid_phase(&book, "BTC-USD", &GRID, 10).unwrap();
    let kept: Vec<usize> = phase.spacings.iter().map(|s| s.sign_kept_phases).collect();
    assert_eq!(kept, [10; 6]);
    let phase = book_grid_phase(&book, "BTC-USD", &GRID, 4).unwrap();
    let kept: Vec<usize> = phase.spacings.iter().map(|s| s.sign_kept_phases).collect();
    assert_eq!(kept, [4, 4, 4, 4, 4, 3]);

    let twenty = book_grid_across(&full_rate_books(&TWENTY), "BTC-USD", &GRID).unwrap();
    assert_eq!(twenty.held_spacings, 1);
    let mut without = vec![1; 20];
    without[2] = 3;
    assert_eq!(twenty.held_without, without);
}

/// The faintest side lost is one book at every split spacing: day seven, the
/// faintest of all twenty. It is also the book losing most up to 5 s; at 10 s
/// the sides lost run from it to 0.630, over thirty times as clear. At four
/// starts day twenty-four's one 10 s miss is the clearest side lost there.
#[test]
fn the_faintest_side_lost_is_day_seven_at_every_split_spacing() {
    let books = full_rate_books(&TWENTY);
    let every = book_grid_phase_across(&books, "BTC-USD", &GRID, 10).unwrap();
    let side = |j: usize| every.books[j].full.avg_queue_imbalance.abs();
    let faintest = (0..books.len()).map(side).fold(f64::INFINITY, f64::min);
    assert_eq!(side(2), faintest);
    let lost: Vec<Option<f64>> = every
        .spacings
        .iter()
        .map(|s| s.faintest_side_missed)
        .collect();
    let expected = [
        Some(faintest),
        None,
        Some(faintest),
        Some(faintest),
        Some(faintest),
        Some(faintest),
    ];
    assert_eq!(lost, expected);
    for s in &every.spacings[..5] {
        assert_eq!(s.faintest_side_missed, s.most_missed_side);
    }
    let ten_s = &every.spacings[5];
    assert!(ten_s.faintest_side_missed < ten_s.most_missed_side);
    assert!(ten_s.strongest_side_missed > 30.0 * faintest);
    // Day twenty-four, kept at 10 s on a side fainter than the clearest lost.
    assert_eq!(ten_s.held_no_clearer, 6);

    let four = book_grid_phase_across(&books, "BTC-USD", &GRID, 4).unwrap();
    let lost: Vec<Option<f64>> = four
        .spacings
        .iter()
        .map(|s| s.faintest_side_missed)
        .collect();
    assert_eq!(lost, expected);
    let ten_s = &four.spacings[5];
    assert_eq!(ten_s.strongest_missed_book, Some(19));
    assert!(ten_s.faintest_side_held < ten_s.strongest_side_missed);
    assert_eq!(ten_s.held_no_clearer, 2);
}
