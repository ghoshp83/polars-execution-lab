# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project adheres to
[Semantic Versioning](https://semver.org/).

## [0.2.0] - 2026-07-31

### Added
- **Order-flow imbalance** in the shared engine: `summary` now reports
  `buy_volume`, `sell_volume`, and `imbalance` = `(buy - sell) / (buy + sell)`
  in `[-1, 1]`. Computed with the same Polars expressions in the Rust crate
  (`order_flow`) and Python (`order_flow`), and covered by the cross-language
  equivalence test.
- **Parquet sink**: `read_ticks` and `write_ticks` select NDJSON or Parquet by
  file extension, and a new `xexeclab convert` subcommand moves a replay between
  the two. NDJSON stays the language-neutral contract; Parquet is a compact
  columnar option for large captures.
- **Ingest normalization tests**: `match_to_tick` / `_iso_to_ns` are now unit
  tested offline (the pure part of the live collector).

### Changed
- Test suite grows to 17 (4 Rust + 13 Python), including a Parquet round-trip
  equivalence test.

## [0.1.0] - 2026-07-30

### Added
- Initial release: one Polars execution-analytics engine run natively from both
  Rust and Python, proven byte-identical on the same replay.
- OHLCV/VWAP bars, session VWAP, session TWAP; POV/TWAP fill simulation with
  implementation-shortfall and slippage evaluation.
- Live Coinbase WebSocket ingest, deterministic synthetic generator, JSONL
  event log, and a two-job CI (Rust + Python) enforcing the equivalence test.
