# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project adheres to
[Semantic Versioning](https://semver.org/).

## [0.6.0] - 2026-08-04

### Added
- **Permanent-impact term — the impact model is now the full two-term
  Almgren-Chriss cost.** `impact_curve` gains a `perm_coef_bps` argument that
  prices each slice's *permanent* impact, a lasting shift of the mid linear in
  size (`perm_bps = perm_coef_bps * participation`), alongside the existing
  *temporary* square-root term. The summary now reports `perm_coef_bps`,
  `avg_perm_impact_bps`, `total_perm_impact_bps`, and `total_cost_bps` (temporary
  + permanent round-trip cost); with `perm_coef_bps = 0` the permanent fields are
  zero and `total_cost_bps` collapses to `total_impact_bps`, so the pure
  square-root curve stays the default. Computed with the same Polars expressions
  in the Rust crate and Python, and held identical by the **fifth** field group
  of the cross-language equivalence test.
- **`--perm-coef-bps`** on both `impact` CLIs (`xexec impact` / `xexeclab impact`).
- **Permanent price drift in the fill simulator**: `pov_fill` / `twap_fill` accept
  `perm_impact_bps`, so each child order permanently shifts the working price in
  its own direction and every later child fills off the drifted price — the
  schedule pays for the mid it walks away for good. `perm_impact_bps = 0`
  preserves the prior behaviour. Exposed as `xexeclab simulate|eval
  --perm-impact-bps`.

### Changed
- Test suite grows to 55 (16 Rust + 39 Python): the equivalence test now also
  asserts the permanent and total-cost fields under a non-zero `perm_coef_bps`.

## [0.5.0] - 2026-08-03

### Added
- **Calibrated square-root market-impact cost curve** in the shared engine: a new
  canonical `ImpactSlice` schema (`ts_ns`/`product`/`participation`) and
  `impact_curve`, which prices each slice of an execution schedule under the
  concave square-root (Almgren-Chriss-style) law
  `impact_bps = coef_bps * sqrt(participation)` — where `coef_bps` is the
  calibration constant (impact in bps of taking the entire available volume) — and
  summarises the schedule as `avg_impact_bps`, `max_impact_bps`, and
  `total_impact_bps`. Computed with the same Polars expressions in the Rust crate
  (`impact::impact_curve`) and Python, and held identical by a **fourth**
  cross-language equivalence test.
- **`impact` subcommand** in both CLIs (`xexec impact --coef-bps` /
  `xexeclab impact --coef-bps`) plus a checked-in `data/sample_impact.ndjson`
  replay.
- **Selectable impact shape in the fill simulator**: `pov_fill` / `twap_fill` now
  take `impact_model` = `linear` (cost proportional to participation, the prior
  behaviour and default) or `sqrt` (the concave Almgren-Chriss law, so a child
  eating twice the volume pays ~1.41x rather than 2x). Exposed as
  `xexeclab simulate|eval --impact-model {linear,sqrt}`.

### Changed
- Test suite grows to 49 (14 Rust + 35 Python), including a fourth equivalence
  test over the impact-curve engine.

## [0.4.0] - 2026-08-02

### Added
- **L2 order-book depth microstructure** in the shared engine: a new canonical
  `BookLevel` schema (`side`/`level`/`price`/`size` per snapshot) and
  `depth_metrics`, a two-stage Polars pipeline that reduces each snapshot to its
  per-side resting depth and top-of-book spread, then averages over the window —
  reporting `avg_bid_depth`, `avg_ask_depth`, `avg_depth_imbalance` =
  `(bid_depth - ask_depth) / (bid_depth + ask_depth)` in `[-1, 1]`, and
  `avg_spread`. Computed with the same expressions in the Rust crate
  (`depth::depth_metrics`) and Python, and held identical by a **third**
  cross-language equivalence test.
- **`depth` subcommand** in both CLIs (`xexec depth` / `xexeclab depth`) plus a
  checked-in `data/sample_book.ndjson` replay.
- **Live L2 book reconstruction with backfill-on-reconnect**: `xexeclab
  ingest-book` subscribes to Coinbase's `level2_batch` channel, seeds the book
  from the `snapshot`, applies each `l2update` statefully, and writes the top-N
  levels per side; on a dropped connection it reconnects and the fresh snapshot
  re-seeds the book (a clean backfill, logged per reconnect via
  `book_ingest_reconnect`). `xexeclab synth-book` writes a deterministic depth
  replay for offline runs and CI.
- **Linear market-impact model** in the fill simulator: `pov_fill` / `twap_fill`
  accept `impact_bps`, charging a child order a bps cost proportional to the
  fraction of the bar's volume it consumes (a buy pays up, a sell receives less);
  `impact_bps=0` preserves the pure-VWAP benchmark. Exposed as
  `xexeclab simulate|eval --impact-bps`.

### Changed
- Test suite grows to 38 (10 Rust + 28 Python), including a third equivalence
  test over the depth engine.

## [0.3.0] - 2026-08-01

### Added
- **Top-of-book quote microstructure** in the shared engine: a new canonical
  `Quote` schema (`bid`/`bid_size`/`ask`/`ask_size`) and `quote_metrics`
  reporting mean spread, mid, size-weighted **microprice**, and **book
  imbalance** = `(bid_size - ask_size) / (bid_size + ask_size)` in `[-1, 1]`.
  Computed with the same Polars expressions in the Rust crate (`quote::quote_metrics`)
  and Python (`quote_metrics`), and held identical by a second cross-language
  equivalence test.
- **`book` subcommand** in both CLIs (`xexec book` / `xexeclab book`) plus a
  checked-in `data/sample_quotes.ndjson` replay.
- **Live top-of-book ingest with auto-reconnect**: `xexeclab ingest-quotes`
  subscribes to Coinbase's `ticker` channel and resumes the capture across
  dropped connections (logged per reconnect); `xexeclab synth-quotes` writes a
  deterministic quote replay for offline runs and CI.

### Changed
- Test suite grows to 26 (7 Rust + 19 Python), including a second equivalence
  test over the quote engine.

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
