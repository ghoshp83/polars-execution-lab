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
