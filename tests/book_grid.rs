use xexec::grid::{book_grid_across, book_grid_phase, book_grid_phase_across, book_grid_sweep};
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
fn each_spacing_names_the_books_that_lose_their_queue_side() {
    let across = book_grid_across(&[book(), steady_book()], "BTC-USD", &[1.0, 2.0]).unwrap();
    assert!(across.spacings[0].sign_lost_by.is_empty());
    assert_eq!(across.spacings[1].sign_lost_by, vec![0]);
    // The names follow input order, not the book: swap them and the index moves.
    let swapped = book_grid_across(&[steady_book(), book()], "BTC-USD", &[1.0, 2.0]).unwrap();
    assert_eq!(swapped.spacings[1].sign_lost_by, vec![1]);
    // Kept and lost partition the books at every spacing.
    for s in across.spacings.iter().chain(&swapped.spacings) {
        assert_eq!(s.sign_kept_books + s.sign_lost_by.len(), 2);
    }
}

#[test]
fn a_grid_across_books_needs_two_of_them() {
    assert!(book_grid_across(&[book()], "BTC-USD", &[1.0]).is_err());
    assert!(book_grid_across(&[], "BTC-USD", &[1.0]).is_err());
}

#[test]
fn a_grid_spacing_must_be_finite_and_listed_once() {
    // Infinity used to pass here and saturate, while Python overflowed on it.
    for grid in [vec![f64::INFINITY], vec![f64::NAN], vec![1.0, 2.0, 1.0]] {
        assert!(
            book_grid_sweep(&book(), "BTC-USD", &grid).is_err(),
            "{grid:?}"
        );
    }
    // A repeat would be counted twice as held.
    assert!(book_grid_across(&[book(), steady_book()], "BTC-USD", &[1.0, 1.0]).is_err());
}

#[test]
fn where_a_grid_starts_can_decide_which_side_the_queue_reports() {
    let phase = book_grid_phase(&book(), "BTC-USD", &[2.0], 2).unwrap();
    let two = &phase.spacings[0];
    // From 0 s it lands on 0, 2, 4 (-1/6); from 1 s on 1, 3 (+0.5). Same book.
    assert_eq!(two.phases_run, 2);
    assert_eq!((two.snapshots_min, two.snapshots_max), (2, 3));
    assert_eq!(two.sign_kept_phases, 1);
    assert!(!two.first_phase_kept);
    assert!((two.queue_imbalance_min + 1.0 / 6.0).abs() < 1e-8);
    assert!((two.queue_imbalance_max - 0.5).abs() < 1e-8);
    assert_eq!(
        (
            phase.held_spacings,
            phase.lost_spacings,
            phase.split_spacings
        ),
        (0, 0, 1)
    );
}

#[test]
fn the_first_start_is_the_grid_book_grid_runs() {
    let grid = [1.0, 2.0, 3.0];
    let phase = book_grid_phase(&book(), "BTC-USD", &grid, 3).unwrap();
    let sweep = book_grid_sweep(&book(), "BTC-USD", &grid).unwrap();
    for (p, r) in phase.spacings.iter().zip(&sweep.rows) {
        assert_eq!(p.first_phase_kept, r.queue_sign_kept);
    }
    assert_eq!(phase.full.avg_spread, sweep.full.avg_spread);
}

#[test]
fn a_start_past_the_end_of_the_book_is_left_out_and_the_counts_partition() {
    // At 10 s and five starts, offsets 2 s and 4 s still land on a 4 s book; 6 s
    // and 8 s fall past its end and keep nothing.
    let phase = book_grid_phase(&book(), "BTC-USD", &[1.0, 10.0], 5).unwrap();
    assert_eq!(phase.spacings[1].phases_run, 3);
    assert_eq!(phase.spacings[1].snapshots_max, 1);
    assert_eq!(
        phase.held_spacings + phase.lost_spacings + phase.split_spacings,
        2
    );
    let steady = book_grid_phase(&steady_book(), "BTC-USD", &[1.0, 2.0], 4).unwrap();
    assert_eq!(steady.held_spacings, 2);
    assert_eq!((steady.lost_spacings, steady.split_spacings), (0, 0));
}

#[test]
fn a_phase_sweep_needs_two_starts_and_a_clean_grid() {
    for phases in [0, 1] {
        assert!(book_grid_phase(&book(), "BTC-USD", &[1.0], phases).is_err());
    }
    assert!(book_grid_phase(&book(), "BTC-USD", &[], 2).is_err());
    assert!(book_grid_phase(&book(), "BTC-USD", &[f64::INFINITY], 2).is_err());
}

/// Bid-heavy at 0 s, 3 s and 4 s, ask-heavy at 1 s and 2 s: +0.1 at full rate.
/// A two-second grid from 0 s lands on 0, 2, 4 (+1/6) and keeps the bid; from
/// 1 s it lands on 1, 3 (exactly 0) and keeps neither side.
fn kept_at_start_book() -> Vec<BookLevel> {
    let ask_heavy = |t| [bl(t, "bid", 100.9, 1.0), bl(t, "ask", 101.0, 3.0)];
    let bid_heavy = |t| [bl(t, "bid", 100.0, 3.0), bl(t, "ask", 101.0, 1.0)];
    [
        bid_heavy(0),
        ask_heavy(S),
        ask_heavy(2 * S),
        bid_heavy(3 * S),
        bid_heavy(4 * S),
    ]
    .concat()
}

#[test]
fn a_spacing_holds_across_books_only_when_every_start_on_every_book_keeps_it() {
    let across =
        book_grid_phase_across(&[book(), steady_book()], "BTC-USD", &[1.0, 2.0], 2).unwrap();
    assert_eq!(across.books.len(), 2);
    let one = &across.spacings[0];
    assert_eq!(one.held_books, 2);
    assert!(one.lost_by.is_empty() && one.split_by.is_empty());
    // At two seconds the first book splits: lost from 0 s, kept from 1 s.
    let two = &across.spacings[1];
    assert_eq!((two.held_books, two.first_phase_books), (1, 1));
    assert_eq!(two.split_by, vec![0]);
    assert!(two.lost_by.is_empty());
    assert_eq!(across.held_spacings, 1);
    assert_eq!(across.held_without, vec![2, 1]);
    // Held, lost and split partition the books at every spacing.
    for s in &across.spacings {
        assert_eq!(s.held_books + s.lost_by.len() + s.split_by.len(), 2);
    }
}

#[test]
fn a_spacing_held_at_the_first_start_on_every_book_can_still_split() {
    let books = [kept_at_start_book(), steady_book()];
    let across = book_grid_phase_across(&books, "BTC-USD", &[2.0], 2).unwrap();
    // book-grid-across reads only the first start and calls the spacing held.
    let first = book_grid_across(&books, "BTC-USD", &[2.0]).unwrap();
    assert_eq!(first.held_spacings, 1);
    assert_eq!(across.first_phase_held_spacings, first.held_spacings);
    // Started from 1 s, the grid loses the first book's side.
    assert_eq!(across.held_spacings, 0);
    assert_eq!(across.spacings[0].split_by, vec![0]);
}

#[test]
fn a_split_spacing_counts_the_starts_that_keep_it_over_every_book() {
    let books = [kept_at_start_book(), steady_book()];
    let across = book_grid_phase_across(&books, "BTC-USD", &[2.0], 2).unwrap();
    let s = &across.spacings[0];
    // Split, not lost: the first book keeps one start of two, the steady book both.
    assert_eq!(s.held_books, 1);
    assert_eq!((s.kept_starts, s.starts_run), (3, 4));
    let per_book: usize = across
        .books
        .iter()
        .map(|b| b.spacings[0].sign_kept_phases)
        .sum();
    assert_eq!(s.kept_starts, per_book);
}

#[test]
fn the_weakest_book_is_named_and_a_tie_names_the_first() {
    // The steady book misses nothing, so the miss belongs to the second book.
    let books = [steady_book(), kept_at_start_book()];
    let s = &book_grid_phase_across(&books, "BTC-USD", &[2.0], 2)
        .unwrap()
        .spacings[0];
    assert_eq!((s.most_missed, s.most_missed_book), (1, 1));
    // Two equally weak books: input order decides, not the last one seen.
    let books = [kept_at_start_book(), kept_at_start_book()];
    let s = &book_grid_phase_across(&books, "BTC-USD", &[2.0], 2)
        .unwrap()
        .spacings[0];
    assert_eq!((s.most_missed, s.most_missed_book), (1, 0));
}

#[test]
fn a_lone_weak_book_leaves_a_gap_and_two_weak_books_tie() {
    // Only the second book misses, so nothing is left for the rest.
    let books = [steady_book(), kept_at_start_book()];
    let s = &book_grid_phase_across(&books, "BTC-USD", &[2.0], 2)
        .unwrap()
        .spacings[0];
    assert_eq!((s.most_missed, s.next_most_missed), (1, 0));
    // Two equally weak books: the runner-up misses as often as the weakest.
    let books = [kept_at_start_book(), kept_at_start_book()];
    let s = &book_grid_phase_across(&books, "BTC-USD", &[2.0], 2)
        .unwrap()
        .spacings[0];
    assert_eq!((s.most_missed, s.next_most_missed), (1, 1));
}

#[test]
fn setting_the_weakest_book_aside_takes_only_its_own_misses() {
    // A lone weak book: the rest keep every start they run.
    let books = [steady_book(), kept_at_start_book()];
    let s = &book_grid_phase_across(&books, "BTC-USD", &[2.0], 2)
        .unwrap()
        .spacings[0];
    assert_eq!((s.kept_without_weakest, s.run_without_weakest), (2, 2));
    assert_eq!(s.starts_run - s.run_without_weakest, 2);
    // A tie: the first book is set aside, the second's miss stays behind.
    let books = [kept_at_start_book(), kept_at_start_book()];
    let s = &book_grid_phase_across(&books, "BTC-USD", &[2.0], 2)
        .unwrap()
        .spacings[0];
    assert_eq!((s.kept_without_weakest, s.run_without_weakest), (1, 2));
    assert_eq!(
        s.run_without_weakest - s.kept_without_weakest,
        s.next_most_missed
    );
}

#[test]
fn half_the_misses_take_one_weak_book_or_several_even_ones() {
    let half = |books: &[Vec<xexec::model::BookLevel>]| {
        book_grid_phase_across(books, "BTC-USD", &[2.0], 2)
            .unwrap()
            .spacings[0]
            .half_missed_by
    };
    // Nothing missed: no book is needed to account for it.
    assert_eq!(half(&[steady_book(), steady_book()]), 0);
    // One weak book carries every miss.
    assert_eq!(half(&[steady_book(), kept_at_start_book()]), 1);
    // Three books missing one start each: one is a third, two are past half.
    let even = [
        kept_at_start_book(),
        kept_at_start_book(),
        kept_at_start_book(),
    ];
    assert_eq!(half(&even), 2);
}

#[test]
fn the_strongest_side_missed_is_the_clearest_full_rate_side_a_grid_loses() {
    // Nothing missed: no side was lost, however clear.
    let books = [steady_book(), steady_book()];
    let across = book_grid_phase_across(&books, "BTC-USD", &[2.0], 2).unwrap();
    assert_eq!(across.spacings[0].strongest_side_missed, 0.0);
    // Only the second book misses, so only its full-rate side counts, not the
    // steady book's, however clear that one is.
    let books = [steady_book(), kept_at_start_book()];
    let across = book_grid_phase_across(&books, "BTC-USD", &[2.0], 2).unwrap();
    let lost = across.books[1].full.avg_queue_imbalance.abs();
    assert!(lost > 0.0);
    assert_eq!(across.spacings[0].strongest_side_missed, lost);
}

#[test]
fn the_faintest_side_held_is_the_weakest_full_rate_side_a_grid_keeps() {
    // Only the steady book keeps every start, so only its side counts, not the
    // missing book's, however faint that one is.
    let books = [steady_book(), kept_at_start_book()];
    let across = book_grid_phase_across(&books, "BTC-USD", &[2.0], 2).unwrap();
    let kept = across.books[0].full.avg_queue_imbalance.abs();
    assert!(kept > 0.0);
    assert_eq!(across.spacings[0].held_books, 1);
    assert_eq!(across.spacings[0].faintest_side_held, kept);
    // No book held: there is no side kept to report.
    let books = [kept_at_start_book(), kept_at_start_book()];
    let across = book_grid_phase_across(&books, "BTC-USD", &[2.0], 2).unwrap();
    assert_eq!(across.spacings[0].held_books, 0);
    assert_eq!(across.spacings[0].faintest_side_held, 0.0);
}

#[test]
fn held_no_clearer_counts_the_kept_books_a_lost_side_outshines() {
    // Nothing missed: no lost side to outshine a kept one.
    let books = [steady_book(), steady_book()];
    let across = book_grid_phase_across(&books, "BTC-USD", &[2.0], 2).unwrap();
    assert_eq!(across.spacings[0].held_books, 2);
    assert_eq!(across.spacings[0].strongest_side_missed, 0.0);
    assert_eq!(across.spacings[0].held_no_clearer, 0);
    // One kept, one missed: the kept book counts only if its side is no clearer
    // than the missed one's, and then side strength does not separate them.
    let books = [steady_book(), kept_at_start_book()];
    let across = book_grid_phase_across(&books, "BTC-USD", &[2.0], 2).unwrap();
    let s = &across.spacings[0];
    let kept = across.books[0].full.avg_queue_imbalance.abs();
    let lost = across.books[1].full.avg_queue_imbalance.abs();
    assert_eq!(s.held_no_clearer, usize::from(kept <= lost));
    assert_eq!(
        s.held_no_clearer == 0,
        s.faintest_side_held > s.strongest_side_missed
    );
    // No book held: nothing to count.
    let books = [kept_at_start_book(), kept_at_start_book()];
    let across = book_grid_phase_across(&books, "BTC-USD", &[2.0], 2).unwrap();
    assert_eq!(across.spacings[0].held_no_clearer, 0);
}

#[test]
fn missed_no_fainter_counts_the_lost_books_a_kept_side_does_not_outshine() {
    // Nothing missed: no lost book to count.
    let books = [steady_book(), steady_book()];
    let across = book_grid_phase_across(&books, "BTC-USD", &[2.0], 2).unwrap();
    assert_eq!(across.spacings[0].missed_no_fainter, 0);
    // One kept, one missed: the missed book counts only if its side is at least
    // as clear as the kept one's -- the same overlap held_no_clearer counts.
    let books = [steady_book(), kept_at_start_book()];
    let across = book_grid_phase_across(&books, "BTC-USD", &[2.0], 2).unwrap();
    let s = &across.spacings[0];
    let kept = across.books[0].full.avg_queue_imbalance.abs();
    let lost = across.books[1].full.avg_queue_imbalance.abs();
    assert_eq!(s.missed_no_fainter, usize::from(lost >= kept));
    assert_eq!(s.missed_no_fainter == 0, s.held_no_clearer == 0);
    // No book held: no kept side to compare with, so 0 rather than every book.
    let books = [kept_at_start_book(), kept_at_start_book()];
    let across = book_grid_phase_across(&books, "BTC-USD", &[2.0], 2).unwrap();
    assert_eq!(across.spacings[0].held_books, 0);
    assert_eq!(across.spacings[0].missed_no_fainter, 0);
}

#[test]
fn a_phase_sweep_across_books_needs_two_books_two_starts_and_a_clean_grid() {
    assert!(book_grid_phase_across(&[book()], "BTC-USD", &[1.0], 2).is_err());
    assert!(book_grid_phase_across(&[book(), steady_book()], "BTC-USD", &[1.0], 1).is_err());
    assert!(book_grid_phase_across(&[book(), steady_book()], "BTC-USD", &[1.0, 1.0], 2).is_err());
}
