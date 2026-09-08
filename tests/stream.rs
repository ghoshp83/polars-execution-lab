//! The streamed session must be the *same* session -- a different route to the
//! answer `execution::summary` already gives, computed without ever holding the
//! capture in memory. These tests pin both halves of that claim: the numbers
//! agree with the in-memory pass, and the residency stays bounded by the chunk.

use std::fs;
use std::path::PathBuf;
use xexec::execution::{order_flow, session_twap, session_vwap};
use xexec::replay::read_ticks;
use xexec::stream::stream_session;

/// Six ticks, in time order, with uneven gaps so the sample-and-hold TWAP is
/// not the plain mean and a lost interval would show up as a wrong number.
const TICKS: &str = concat!(
    r#"{"ts_ns":1000,"product":"BTC-USD","price":30000.0,"size":0.5,"side":"buy","trade_id":1}"#,
    "\n",
    r#"{"ts_ns":2000,"product":"BTC-USD","price":30010.0,"size":1.5,"side":"sell","trade_id":2}"#,
    "\n",
    r#"{"ts_ns":5000,"product":"BTC-USD","price":29990.0,"size":0.25,"side":"buy","trade_id":3}"#,
    "\n",
    r#"{"ts_ns":6000,"product":"BTC-USD","price":30020.0,"size":2.0,"side":"buy","trade_id":4}"#,
    "\n",
    r#"{"ts_ns":9000,"product":"BTC-USD","price":30005.0,"size":0.75,"side":"sell","trade_id":5}"#,
    "\n",
    r#"{"ts_ns":9500,"product":"BTC-USD","price":30015.0,"size":1.0,"side":"buy","trade_id":6}"#,
    "\n",
);

/// Write a replay into a uniquely named temp file and hand back its path.
fn replay(name: &str, body: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("xexec_stream_{name}.ndjson"));
    fs::write(&path, body).expect("writing the replay fixture");
    path
}

#[test]
fn the_streamed_session_equals_the_in_memory_session() {
    let path = replay("same", TICKS);
    let ticks = read_ticks(&path).unwrap();
    let (buy, sell, imbalance) = order_flow(&ticks).unwrap();

    let s = stream_session(&path, 2).unwrap();

    // The point of the release: folding the file changes the memory profile,
    // not the answer.
    assert_eq!(s.vwap, session_vwap(&ticks).unwrap());
    assert_eq!(s.twap, session_twap(&ticks).unwrap());
    assert_eq!(s.buy_volume, buy);
    assert_eq!(s.sell_volume, sell);
    assert_eq!(s.imbalance, imbalance);
    assert_eq!(s.rows, ticks.len());
    assert_eq!(s.product, "BTC-USD");
    assert_eq!(s.first_ts_ns, 1000);
    assert_eq!(s.last_ts_ns, 9500);
}

#[test]
fn the_chunk_size_does_not_change_the_answer() {
    let path = replay("chunks", TICKS);
    let one = stream_session(&path, 1).unwrap();

    // A chunk size is an operational knob -- how much memory to spend -- and an
    // operational knob that moves the reported VWAP is a bug, not a setting.
    for n in [2usize, 3, 4, 5, 6, 1000] {
        let s = stream_session(&path, n).unwrap();
        assert_eq!(s.vwap, one.vwap, "vwap moved at chunk_rows={n}");
        assert_eq!(s.twap, one.twap, "twap moved at chunk_rows={n}");
        assert_eq!(s.volume, one.volume, "volume moved at chunk_rows={n}");
        assert_eq!(s.notional, one.notional, "notional moved at chunk_rows={n}");
        assert_eq!(s.imbalance, one.imbalance, "imbalance moved at n={n}");
        assert_eq!(s.rows, one.rows);
    }
}

#[test]
fn the_interval_spanning_a_chunk_boundary_is_still_priced() {
    let path = replay("boundary", TICKS);
    let ticks = read_ticks(&path).unwrap();
    let whole = session_twap(&ticks).unwrap();

    // With chunk_rows = 1 every interval spans a boundary. If the carried tick
    // were dropped the TWAP would have no intervals left at all; if it were
    // dropped only sometimes the answer would drift. Either way this fails.
    assert_eq!(stream_session(&path, 1).unwrap().twap, whole);
    assert_eq!(stream_session(&path, 5).unwrap().twap, whole);
}

#[test]
fn memory_stays_bounded_by_the_chunk_not_the_file() {
    let path = replay("bounded", TICKS);

    let small = stream_session(&path, 2).unwrap();
    assert_eq!(small.chunks, 3);
    // One chunk, plus the tick carried across the boundary.
    assert_eq!(small.peak_rows_in_memory, 3);
    assert!(small.peak_rows_in_memory < small.rows);

    // A chunk wider than the file is one chunk with nothing to carry, so the
    // bound degrades to the file itself -- which is the honest report, not a
    // flattering one.
    let whole = stream_session(&path, 1000).unwrap();
    assert_eq!(whole.chunks, 1);
    assert_eq!(whole.peak_rows_in_memory, whole.rows);
}

#[test]
fn a_capture_out_of_time_order_is_refused() {
    let out_of_order = concat!(
        r#"{"ts_ns":5000,"product":"BTC-USD","price":30000.0,"size":1.0,"side":"buy","trade_id":1}"#,
        "\n",
        r#"{"ts_ns":1000,"product":"BTC-USD","price":30010.0,"size":1.0,"side":"buy","trade_id":2}"#,
        "\n",
    );
    let path = replay("unordered", out_of_order);

    // The in-memory path sorts; a streamed pass cannot sort what it has not
    // seen yet. Rather than quietly price a shuffled capture as if the gaps
    // were real, it refuses.
    let err = stream_session(&path, 8).unwrap_err().to_string();
    assert!(err.contains("not in time order"), "{err}");
}

#[test]
fn a_chunk_of_no_rows_is_rejected() {
    let path = replay("zerochunk", TICKS);
    let err = stream_session(&path, 0).unwrap_err().to_string();
    assert!(err.contains("chunk_rows must be >= 1"), "{err}");
}

#[test]
fn a_columnar_sink_is_not_a_stream() {
    // Parquet holds the same rows, but not in a form a line-oriented fold can
    // walk. Refusing by extension is cheaper and clearer than failing on the
    // first non-UTF-8 byte.
    let err = stream_session("data/sample_ticks.parquet", 4)
        .unwrap_err()
        .to_string();
    assert!(err.contains("streaming reads NDJSON only"), "{err}");
}

#[test]
fn an_empty_capture_is_refused() {
    let path = replay("empty", "\n\n");
    let err = stream_session(&path, 4).unwrap_err().to_string();
    assert!(err.contains("no ticks in"), "{err}");
}

#[test]
fn a_single_tick_cannot_make_a_twap() {
    let one = concat!(
        r#"{"ts_ns":1000,"product":"BTC-USD","price":30000.0,"size":1.0,"side":"buy","trade_id":1}"#,
        "\n",
    );
    let path = replay("single", one);

    // The same refusal `session_twap` gives, worded the same way: one trade has
    // no following interval, so there is no time to weight it by.
    let err = stream_session(&path, 4).unwrap_err().to_string();
    assert!(err.contains("need >= 2 ticks for twap"), "{err}");
}
