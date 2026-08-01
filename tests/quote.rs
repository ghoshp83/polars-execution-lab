use xexec::model::Quote;
use xexec::quote::quote_metrics;
use xexec::replay::read_quotes;

fn load() -> Vec<Quote> {
    read_quotes("data/sample_quotes.ndjson").expect("sample quote replay must load")
}

#[test]
fn spread_is_positive_and_mid_lies_between_bid_and_ask() {
    let quotes = load();
    let m = quote_metrics(&quotes, "BTC-USD").unwrap();
    assert_eq!(m.quotes, quotes.len());
    // A well-formed book never crosses, so the mean spread must be positive.
    assert!(m.avg_spread > 0.0, "mean spread must be positive");
    let lo = quotes.iter().map(|q| q.bid).fold(f64::MAX, f64::min);
    let hi = quotes.iter().map(|q| q.ask).fold(f64::MIN, f64::max);
    assert!(
        m.avg_mid >= lo && m.avg_mid <= hi,
        "mid within bid/ask range"
    );
}

#[test]
fn book_imbalance_is_bounded_and_tracks_resting_size() {
    let quotes = load();
    let m = quote_metrics(&quotes, "BTC-USD").unwrap();
    assert!(
        (-1.0..=1.0).contains(&m.avg_book_imbalance),
        "book imbalance is a ratio in [-1, 1]"
    );
    // Mean imbalance sign must agree with which side rests more total size --
    // that is exactly what the metric is meant to surface.
    let bid_sz: f64 = quotes.iter().map(|q| q.bid_size).sum();
    let ask_sz: f64 = quotes.iter().map(|q| q.ask_size).sum();
    if bid_sz != ask_sz {
        assert_eq!(
            m.avg_book_imbalance > 0.0,
            bid_sz > ask_sz,
            "imbalance sign tracks the heavier side of the book"
        );
    }
}

#[test]
fn microprice_leans_toward_the_thinner_side() {
    // With more size on the ask than the bid, the microprice weights toward the
    // bid and sits below the mid; the reverse holds when the bid is thinner.
    let quotes = vec![Quote {
        ts_ns: 0,
        product: "BTC-USD".into(),
        bid: 100.0,
        bid_size: 1.0,
        ask: 102.0,
        ask_size: 3.0,
    }];
    let m = quote_metrics(&quotes, "BTC-USD").unwrap();
    assert_eq!(m.avg_mid, 101.0);
    assert!(
        m.avg_microprice < m.avg_mid,
        "heavier ask should pull the microprice below the mid"
    );
}
