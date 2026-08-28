use xexec::model::BookLevel;
use xexec::replay::read_book;
use xexec::sweep::sweep_cost;

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
fn sweep_prices_a_buy_that_walks_the_sample_book() {
    let levels = load();
    let m = sweep_cost(&levels, "BTC-USD", "buy", 2.0).unwrap();
    // A 2.0 buy clears the touch in every snapshot and eats into deeper levels
    // (2, 2, 3, 2, 2 -> mean 2.2); values hand-checked against the fixture.
    assert_eq!(m.snapshots, 5);
    assert_eq!(m.filled_snapshots, 5);
    assert!((m.avg_sweep_vwap - 60011.96).abs() < 1e-8);
    assert!((m.avg_slippage_bps - 0.04332343).abs() < 1e-8);
    assert!((m.avg_levels_consumed - 2.2).abs() < 1e-9);
    assert!((m.avg_fill_ratio - 1.0).abs() < 1e-9);
}

#[test]
fn an_order_inside_the_touch_pays_no_slippage() {
    // The whole order rests at the best level, so the realised price *is* the
    // touch: the sweep costs nothing beyond crossing the spread. This is the
    // boundary that separates liquidity cost from spread cost.
    let m = sweep_cost(&load(), "BTC-USD", "buy", 0.4).unwrap();
    assert!(m.avg_slippage_bps.abs() < 1e-12);
    assert!((m.avg_levels_consumed - 1.0).abs() < 1e-9);
    assert!((m.avg_fill_ratio - 1.0).abs() < 1e-9);
}

#[test]
fn slippage_grows_with_order_size() {
    // The economic claim of the whole module: a bigger order reaches worse
    // prices. If this ever stopped holding the book walk would be wrong.
    let levels = load();
    let costs: Vec<f64> = [0.4, 1.0, 2.0, 3.0]
        .iter()
        .map(|q| sweep_cost(&levels, "BTC-USD", "buy", *q).unwrap().avg_slippage_bps)
        .collect();
    assert!(costs.windows(2).all(|w| w[0] <= w[1]), "{costs:?}");
    assert!(costs[3] > costs[0]);
}

#[test]
fn a_thin_book_reports_a_short_fill_not_a_silent_full_one() {
    // 10.0 exceeds the captured depth of every snapshot: the sweep must report
    // what it could actually fill rather than pretending the order completed.
    let m = sweep_cost(&load(), "BTC-USD", "buy", 10.0).unwrap();
    assert_eq!(m.filled_snapshots, 0);
    assert!((m.avg_fill_ratio - 0.36).abs() < 1e-9);
    assert!((m.avg_levels_consumed - 3.0).abs() < 1e-9);
}

#[test]
fn a_sell_walks_the_bids_downward() {
    // Bids 2.0 @ 100 then 1.0 @ 99; a 3.0 sell realises (2*100 + 99) / 3, i.e.
    // 33.33 bps below the touch. Signed so a larger number is worse either side.
    let levels = vec![
        bl(0, "bid", 0, 100.0, 2.0),
        bl(0, "bid", 1, 99.0, 1.0),
        bl(0, "ask", 0, 101.0, 5.0),
    ];
    let m = sweep_cost(&levels, "BTC-USD", "sell", 3.0).unwrap();
    assert!((m.avg_sweep_vwap - 299.0 / 3.0).abs() < 1e-8);
    assert!((m.avg_slippage_bps - 33.33333333).abs() < 1e-8);
    assert!((m.avg_levels_consumed - 2.0).abs() < 1e-9);
}

#[test]
fn a_buy_and_a_sell_of_the_same_size_price_different_sides() {
    // The same order size must not read the same book: a buy crosses the asks,
    // a sell crosses the bids.
    let levels = load();
    let buy = sweep_cost(&levels, "BTC-USD", "buy", 2.0).unwrap();
    let sell = sweep_cost(&levels, "BTC-USD", "sell", 2.0).unwrap();
    assert!(buy.avg_sweep_vwap > sell.avg_sweep_vwap);
}

#[test]
fn bad_inputs_are_rejected() {
    let levels = load();
    assert!(sweep_cost(&[], "BTC-USD", "buy", 1.0).is_err());
    assert!(sweep_cost(&levels, "BTC-USD", "buy", 0.0).is_err());
    assert!(sweep_cost(&levels, "BTC-USD", "sideways", 1.0).is_err());
}
