use xexec::depth::depth_metrics;
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
