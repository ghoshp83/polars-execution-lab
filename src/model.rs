use serde::{Deserialize, Serialize};

/// A canonical trade tick. This is the on-the-wire schema of a replay file
/// (one JSON object per line) and the normalized form every exchange adapter
/// maps into.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tick {
    /// Trade time as nanoseconds since the Unix epoch (UTC).
    pub ts_ns: i64,
    /// Product / symbol, e.g. "BTC-USD".
    pub product: String,
    /// Executed price.
    pub price: f64,
    /// Executed base-asset size.
    pub size: f64,
    /// Aggressor side: "buy" or "sell".
    pub side: String,
    /// Exchange trade id.
    pub trade_id: i64,
}

/// A canonical top-of-book quote. This is the on-the-wire schema of a quote
/// replay file (one JSON object per line) and the normalized form every
/// exchange best-bid/offer adapter maps into.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Quote {
    /// Quote time as nanoseconds since the Unix epoch (UTC).
    pub ts_ns: i64,
    /// Product / symbol, e.g. "BTC-USD".
    pub product: String,
    /// Best bid price.
    pub bid: f64,
    /// Resting size at the best bid.
    pub bid_size: f64,
    /// Best ask price.
    pub ask: f64,
    /// Resting size at the best ask.
    pub ask_size: f64,
}

/// One price level of an L2 order-book snapshot. A full snapshot is the set of
/// rows sharing a `ts_ns`: several `bid` levels and several `ask` levels, each
/// tagged with its `level` (0 = best). This is the on-the-wire schema of a
/// depth replay file and the normalized form the `level2` book reconstruction
/// emits, one JSON object per line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BookLevel {
    /// Snapshot time as nanoseconds since the Unix epoch (UTC).
    pub ts_ns: i64,
    /// Product / symbol, e.g. "BTC-USD".
    pub product: String,
    /// Book side: "bid" or "ask".
    pub side: String,
    /// Depth rank on this side: 0 is the best (top of book).
    pub level: i64,
    /// Price at this level.
    pub price: f64,
    /// Resting size at this level.
    pub size: f64,
}

/// One slice of an execution schedule, tagged with the fraction of the
/// available volume it consumes. This is the on-the-wire schema of an impact
/// replay file (one JSON object per line): the input to the square-root
/// market-impact cost curve.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImpactSlice {
    /// Slice time as nanoseconds since the Unix epoch (UTC).
    pub ts_ns: i64,
    /// Product / symbol, e.g. "BTC-USD".
    pub product: String,
    /// Fraction of the available volume this slice takes, in `(0, 1]`.
    pub participation: f64,
}
