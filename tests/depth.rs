use xexec::depth::{depth_metrics, queue_metrics};
use xexec::model::BookLevel;
use xexec::replay::read_book;

fn load() -> Vec<BookLevel> {
    read_book("data/sample_book.ndjson").expect("sample book replay must load")
}

fn bl(ts_ns: i64, side: &str, level: i64, price: f64, size: f64) -> BookLevel {
    BookLevel {
        ts_ns,
        product: "BTC-USD".into(),
        side: side.into(),
        level,
        price,
        size,
    }
}

#[test]
fn depth_and_spread_summarise_the_sample_book() {
    let levels = load();
    let m = depth_metrics(&levels, "BTC-USD").unwrap();
    // The sample is five 3-level snapshots; each metric is a per-snapshot
    // quantity averaged over the window (hand-checked against the fixture).
    assert_eq!(m.snapshots, 5);
    assert!((m.avg_bid_depth - 4.12).abs() < 1e-9);
    assert!((m.avg_ask_depth - 3.6).abs() < 1e-9);
    assert!((m.avg_spread - 1.4).abs() < 1e-9, "mean top-of-book spread");
    assert!(
        (-1.0..=1.0).contains(&m.avg_depth_imbalance),
        "depth imbalance is a ratio in [-1, 1]"
    );
}

#[test]
fn depth_imbalance_is_positive_when_the_bid_rests_heavier() {
    // One snapshot: bids total 3.0, asks total 1.0 -> imbalance (3-1)/(3+1)=0.5.
    let levels = vec![
        bl(0, "bid", 0, 100.0, 2.0),
        bl(0, "bid", 1, 99.5, 1.0),
        bl(0, "ask", 0, 101.0, 0.5),
        bl(0, "ask", 1, 101.5, 0.5),
    ];
    let m = depth_metrics(&levels, "BTC-USD").unwrap();
    assert_eq!(m.snapshots, 1);
    assert!((m.avg_depth_imbalance - 0.5).abs() < 1e-9);
    assert!((m.avg_spread - 1.0).abs() < 1e-9);
}

#[test]
fn empty_book_is_rejected() {
    assert!(depth_metrics(&[], "BTC-USD").is_err());
}

#[test]
fn queue_reports_the_touch_size_averaged_over_the_window() {
    let levels = load();
    let m = queue_metrics(&levels, "BTC-USD").unwrap();
    // The five snapshots' best-bid sizes are 1.20/0.60/2.00/0.90/1.50 (mean 1.24)
    // and best-ask 0.80/1.40/0.50/1.10/1.50 (mean 1.06); hand-checked.
    assert_eq!(m.snapshots, 5);
    assert!((m.avg_bid_queue - 1.24).abs() < 1e-9);
    assert!((m.avg_ask_queue - 1.06).abs() < 1e-9);
    // Per-snapshot imbalances 0.2/-0.4/0.6/-0.1/0.0 average to 0.06.
    assert!((m.avg_queue_imbalance - 0.06).abs() < 1e-9);
}

#[test]
fn queue_ignores_depth_below_the_touch() {
    // Queue position is the size at level 0 ALONE: a huge level-1 bid that would
    // dominate `depth_metrics` must not move the queue. Best bid 2.0 vs best ask
    // 0.5 -> imbalance (2-0.5)/(2+0.5) = 0.6, regardless of the level-1 size.
    let levels = vec![
        bl(0, "bid", 0, 100.0, 2.0),
        bl(0, "bid", 1, 99.5, 50.0),
        bl(0, "ask", 0, 101.0, 0.5),
    ];
    let m = queue_metrics(&levels, "BTC-USD").unwrap();
    assert_eq!(m.snapshots, 1);
    assert!(
        (m.avg_bid_queue - 2.0).abs() < 1e-9,
        "touch size, not depth"
    );
    assert!((m.avg_ask_queue - 0.5).abs() < 1e-9);
    assert!((m.avg_queue_imbalance - 0.6).abs() < 1e-9);
}

#[test]
fn empty_book_is_rejected_by_queue() {
    assert!(queue_metrics(&[], "BTC-USD").is_err());
}
