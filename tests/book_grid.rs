use xexec::grid::{book_grid_across, book_grid_sweep};
use xexec::model::BookLevel;

const S: i64 = 1_000_000_000;

fn bl(ts_ns: i64, side: &str, price: f64, size: f64) -> BookLevel {
    BookLevel {
        ts_ns,
        product: "BTC-USD".into(),
        side: side.into(),
        level: 0,
        price,
        size,
    }
}

/// Five one-level snapshots a second apart. The ask-heavy ones (0 s, 2 s) sit
/// at a 0.1 spread and the bid-heavy ones (1 s, 3 s, 4 s) at 1.0, so at full
/// rate the queue rests on the bid (+0.1) and the spread averages 0.64.
fn book() -> Vec<BookLevel> {
    let ask_heavy = |t| [bl(t, "bid", 100.9, 1.0), bl(t, "ask", 101.0, 3.0)];
    let bid_heavy = |t| [bl(t, "bid", 100.0, 3.0), bl(t, "ask", 101.0, 1.0)];
    [
        ask_heavy(0),
        bid_heavy(S),
        ask_heavy(2 * S),
        bid_heavy(3 * S),
        bid_heavy(4 * S),
    ]
    .concat()
}

#[test]
fn a_grid_keeps_the_first_snapshot_and_each_one_a_step_after_the_last_kept() {
    let sweep = book_grid_sweep(&book(), "BTC-USD", &[1.0, 2.0, 10.0]).unwrap();
    assert_eq!(sweep.full.snapshots, 5);
    let kept: Vec<usize> = sweep.rows.iter().map(|r| r.snapshots).collect();
    assert_eq!(kept, [5, 3, 1]);
    // A one-second grid over a book recorded a second apart keeps all of it.
    let one = &sweep.rows[0];
    assert_eq!(one.avg_spread, sweep.full.avg_spread);
    assert_eq!(one.avg_queue_imbalance, sweep.full.avg_queue_imbalance);
    assert_eq!(one.spread_ratio, 1.0);
}

#[test]
fn a_grid_that_lands_on_one_kind_of_touch_moves_the_queue_to_the_other_side() {
    let sweep = book_grid_sweep(&book(), "BTC-USD", &[1.0, 2.0]).unwrap();
    assert!((sweep.full.avg_queue_imbalance - 0.1).abs() < 1e-8);
    assert!((sweep.full.avg_spread - 0.64).abs() < 1e-8);
    // Two seconds lands on 0 s, 2 s and 4 s: two ask-heavy, one bid-heavy.
    let two = &sweep.rows[1];
    assert!((two.avg_queue_imbalance + 1.0 / 6.0).abs() < 1e-8);
    assert!(!two.queue_sign_kept);
    assert!((two.spread_ratio - 0.625).abs() < 1e-8);
    // The depth summed over the one level is the same either way.
    assert!((two.avg_bid_depth + two.avg_ask_depth - 4.0).abs() < 1e-8);
    assert_eq!(sweep.sign_flips, 1);
}

#[test]
fn a_grid_spacing_must_be_positive() {
    for grid in [vec![], vec![0.0], vec![1.0, -2.0]] {
        assert!(
            book_grid_sweep(&book(), "BTC-USD", &grid).is_err(),
            "{grid:?}"
        );
    }
}

/// Five bid-heavy snapshots: the queue rests on the bid at every spacing.
fn steady_book() -> Vec<BookLevel> {
    (0..5)
        .flat_map(|i| [bl(i * S, "bid", 100.0, 3.0), bl(i * S, "ask", 101.0, 1.0)])
        .collect()
}

#[test]
fn a_spacing_holds_across_books_only_when_every_book_keeps_its_queue_side() {
    let across = book_grid_across(&[book(), steady_book()], "BTC-USD", &[1.0, 2.0]).unwrap();
    assert_eq!(across.books.len(), 2);
    let one = &across.spacings[0];
    assert_eq!(one.sign_kept_books, 2);
    assert_eq!((one.spread_ratio_min, one.spread_ratio_max), (1.0, 1.0));
    assert_eq!(one.imbalance_drift_max, 0.0);
    // At two seconds the first book flips and the steady one does not.
    let two = &across.spacings[1];
    assert_eq!(two.sign_kept_books, 1);
    assert!((two.spread_ratio_min - 0.625).abs() < 1e-8);
    assert_eq!(two.spread_ratio_max, 1.0);
    let first = &across.books[0];
    let drift = (first.rows[1].avg_depth_imbalance - first.full.avg_depth_imbalance).abs();
    assert!((two.imbalance_drift_max - drift).abs() < 1e-8);
    assert!(two.imbalance_drift_max > 0.0);
    assert_eq!(across.held_spacings, 1);
    // Leaving out the flipping book frees its spacing; leaving out the steady
    // one frees nothing.
    assert_eq!(across.held_without, vec![2, 1]);
}

#[test]
fn a_grid_across_books_needs_two_of_them() {
    assert!(book_grid_across(&[book()], "BTC-USD", &[1.0]).is_err());
    assert!(book_grid_across(&[], "BTC-USD", &[1.0]).is_err());
}
