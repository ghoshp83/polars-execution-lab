//! The six sessions under `data/real` are one unbroken ten-minute run of
//! BTC-USD trades from Coinbase's public feed, cut into consecutive 100-second
//! slices. Every other capture in `data/` was written by hand to make a point;
//! these were not, so what the engine reports on them is a measurement.

use xexec::curve::sweep_curve;
use xexec::depth::{depth_metrics, queue_metrics};
use xexec::grid::{book_grid_across, book_grid_sweep};
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
