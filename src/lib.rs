//! xexec -- execution-analytics engine over crypto tick data.
//!
//! The analytical core (OHLCV/VWAP bars and session benchmarks) is computed
//! with the Polars query engine, the same engine the Python side of this
//! project uses. A cross-language equivalence test asserts both produce
//! identical results on the same deterministic replay, so the "one engine,
//! two languages" claim is verified, not asserted.

pub mod depth;
pub mod execution;
pub mod model;
pub mod quote;
pub mod replay;
