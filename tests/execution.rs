use xexec::execution::{bars, order_flow, session_twap, session_vwap, summary};
use xexec::model::Tick;
use xexec::replay::read_ticks;

const BUCKET_1S: i64 = 1_000_000_000;

fn load() -> Vec<Tick> {
    read_ticks("data/sample_ticks.ndjson").expect("sample replay must load")
}

#[test]
fn bars_conserve_volume_and_are_valid_ohlc() {
    let ticks = load();
    let bs = bars(&ticks, BUCKET_1S).unwrap();
    // The sample spans three one-second buckets.
    assert_eq!(bs.len(), 3, "expected three 1s bars");

    let bar_vol: f64 = bs.iter().map(|b| b.volume).sum();
    let tick_vol: f64 = ticks.iter().map(|t| t.size).sum();
    assert!(
        (bar_vol - tick_vol).abs() < 1e-6,
        "bars must conserve total traded volume"
    );

    for b in &bs {
        assert!(b.high >= b.low, "high >= low");
        assert!(
            b.high >= b.open && b.high >= b.close,
            "high bounds open/close"
        );
        assert!(b.low <= b.open && b.low <= b.close, "low bounds open/close");
        assert!(
            b.vwap >= b.low && b.vwap <= b.high,
            "bar vwap within its range"
        );
    }
}

#[test]
fn vwap_weights_by_size_not_count() {
    let ticks = load();
    let vwap = session_vwap(&ticks).unwrap();
    let mean: f64 = ticks.iter().map(|t| t.price).sum::<f64>() / ticks.len() as f64;
    // With non-uniform sizes, the size-weighted VWAP must not collapse to the
    // naive arithmetic mean -- that is the whole point of using VWAP.
    assert!(
        (vwap - mean).abs() > 1e-9,
        "vwap must differ from the unweighted mean"
    );
    let hi = ticks.iter().map(|t| t.price).fold(f64::MIN, f64::max);
    let lo = ticks.iter().map(|t| t.price).fold(f64::MAX, f64::min);
    assert!(vwap >= lo && vwap <= hi, "vwap within observed price range");
}

#[test]
fn twap_differs_from_vwap_and_summary_is_consistent() {
    let ticks = load();
    let vwap = session_vwap(&ticks).unwrap();
    let twap = session_twap(&ticks).unwrap();
    assert!(
        twap != vwap,
        "time-weighting and size-weighting should not coincide here"
    );
    let s = summary(&ticks, "BTC-USD", BUCKET_1S).unwrap();
    assert_eq!(s.ticks, ticks.len());
    assert_eq!(s.vwap, vwap);
    assert_eq!(s.twap, twap);
    assert_eq!(s.bars.len(), 3);
}

#[test]
fn order_flow_splits_volume_and_bounds_imbalance() {
    let ticks = load();
    let (buy, sell, imbalance) = order_flow(&ticks).unwrap();
    let total: f64 = ticks.iter().map(|t| t.size).sum();
    // Buy and sell volume must partition the total traded volume.
    assert!(
        (buy + sell - total).abs() < 1e-6,
        "buy + sell must equal total volume"
    );
    assert!(
        (-1.0..=1.0).contains(&imbalance),
        "imbalance is a ratio in [-1, 1]"
    );
    // The imbalance sign must agree with which side traded more.
    let expected_sign = (buy - sell).signum();
    assert!(
        imbalance == 0.0 || imbalance.signum() == expected_sign,
        "imbalance sign tracks the heavier side"
    );
    let s = summary(&ticks, "BTC-USD", BUCKET_1S).unwrap();
    assert_eq!(s.buy_volume, buy);
    assert_eq!(s.sell_volume, sell);
    assert_eq!(s.imbalance, imbalance);
}
