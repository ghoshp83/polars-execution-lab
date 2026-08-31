# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project adheres to
[Semantic Versioning](https://semver.org/).

## [0.11.0] - 2026-08-31

### Added
- **Impact calibration from the book alone -- `curve` / `sweep_curve`.**
  `calibrate` fits the Almgren-Chriss coefficients from a desk's own realised
  fills. A new venue, a new product, or a pre-trade "what would this cost us
  here?" question has no fills to fit. This release recovers the temporary
  coefficient from an L2 capture alone: a ladder of order sizes is swept through
  the book by `sweep_cost`, each cost is expressed against the mean resting depth
  on the swept side as a participation rate, and the concave law
  `measured_bps = coef_bps * sqrt(participation)` is fitted through the origin
  over those points. The summary reports `coef_bps`, `rmse_bps`, `r_squared`,
  `avg_depth`, and the whole `curve` -- every rung with its `participation`,
  `measured_bps`, the fitted `modelled_bps`, and the `residual_bps` between them.
  So the fit **tests** the square-root law rather than assuming it: a book that
  charges a different shape shows up as a low `r_squared` and a visible residual
  pattern instead of being averaged away. A size the captured book cannot fill
  paid only for the liquidity that was there, so it is reported with its short
  `fill_ratio` and **excluded from the fit**, never quietly regressed. Implemented
  in the Rust crate (`src/curve.rs`) and Python (`engine.py`), held identical by a
  **ninth** cross-language equivalence test -- the deepest yet: a ladder of full
  book walks, a separate depth aggregation, a filter that drops the short fills,
  and a least-squares solve on top, any of which would move the coefficient if the
  two engines diverged. New `xexec curve` / `xexeclab curve` subcommands
  (`--side`, `--sizes`) over the existing `data/sample_book.ndjson` fixture.

### Changed
- README architecture diagram now shows the `sweep` and `curve` stages, which the
  book-sweep release left out.
- Honest disclaimer updated: the fitted coefficient inherits the sweep's static
  book -- it measures what liquidity is *showing*, not what would refill during a
  real execution -- and a low `r_squared` means the single coefficient is a poor
  summary of that book, not that the book is wrong.

## [0.10.0] - 2026-08-28

### Added
- **Book-sweep cost -- `sweep` / `sweep_cost`.** `depth` answers "how much size is
  standing" and `queue` answers "how long is the passive line"; neither prices a
  **taker**. An order larger than the touch eats level 0, then level 1, then level
  2, and its realised price is the size-weighted average of the levels it
  consumed. That consumption cost is exactly what the Almgren-Chriss impact model
  parameterises -- this release measures it **directly off the book** instead of
  modelling it, so the two can be compared. `sweep_cost` walks each snapshot in
  the order the taker meets it (asks cheapest first for a `buy`, bids dearest
  first for a `sell`) using a Polars `cum_sum` window over `ts_ns` for the size
  resting ahead of each level, allocates the order across levels, then prices the
  fill against the touch it started from: `avg_sweep_vwap`, `avg_slippage_bps`
  (signed so a larger number is always worse on either side), `avg_levels_consumed`,
  `avg_fill_ratio`, and `filled_snapshots`. A book too thin to complete the order
  reports the **short fill**, never a silent full one. Implemented in the Rust
  crate (`src/sweep.rs`) and Python (`engine.py`), held identical by an **eighth**
  cross-language equivalence test -- the first over a metric that depends on a
  *within-snapshot* order, so a divergence in sort order or in the running total
  would change the allocation itself. New `xexec sweep` / `xexeclab sweep`
  subcommands (`--side`, `--size`) over the existing `data/sample_book.ndjson`
  fixture.

### Changed
- Honest disclaimer updated: the sweep prices a **static** book -- it does not
  model replenishment or other participants reacting while the order executes, so
  it is the cost of taking the visible liquidity, not a full execution simulation.

## [0.9.0] - 2026-08-19

### Added
- **Top-of-book queue-position metrics — `queue` / `queue_metrics`.** The `depth`
  command sums resting size across *all* captured levels, which answers "how much
  liquidity is standing" but not "how long is the line I'd join." Passive fill
  priority is governed by the size at the **touch** alone: a maker joins the back
  of the best-level queue and only fills once the size ahead of it trades through.
  This release adds a distinct session summary — `avg_bid_queue` / `avg_ask_queue`
  (mean resting size at the best bid / ask) and `avg_queue_imbalance`
  (`(bid_queue - ask_queue) / (bid_queue + ask_queue)` in `[-1, 1]`, the
  fill-priority signal: positive means a longer queue on the bid, so a passive bid
  waits behind more size than a passive ask). Same two-stage Polars reduction as
  `depth` — collapse each snapshot to its level-0 size per side, then average over
  the window — in the Rust crate (`src/depth.rs`) and Python (`engine.py`), held
  identical by a **seventh** cross-language equivalence test. A dedicated test in
  both languages asserts the intent that separates queue from depth: a large
  level-1 order that would dominate `depth` must leave the queue metrics
  untouched. New `xexec queue` / `xexeclab queue` subcommands over the existing
  `data/sample_book.ndjson` fixture.

### Changed
- Honest disclaimer updated: the reconstructed book now backs **top-of-book
  queue-size** analytics, framed as a session average of touch size — explicitly
  **not** an order-by-order queue-position simulation.

## [0.8.0] - 2026-08-06

### Added
- **Robust / regularised calibration — `calibrate_impact_robust`.** The v0.7 fit
  is plain least squares, so a single bad print (a fat-finger fill, a mis-tagged
  participation) drags both coefficients toward itself, and a design clustered at
  one participation level barely separates the two basis functions. This variant
  adds the two standard defences: **Huber** (`huber_delta`) runs iteratively
  reweighted least squares, re-weighting every sample by `min(1, delta/|residual|)`
  so a gross outlier's pull decays as `1/|residual|`; **ridge** (`ridge_lambda`)
  adds an L2 penalty to the normal-matrix diagonal, shrinking toward zero and
  making a single-participation-level design solvable rather than rejected. With
  `huber_delta=None` and `ridge_lambda=0` it reproduces `calibrate_impact`
  bit-for-bit. Both defences are the same weighted Polars sums plus 2x2 solve in
  the Rust crate (`src/calibrate.rs`) and Python (`engine.py`), held identical by
  a **sixth** cross-language equivalence test — the iteratively reweighted fit,
  every reweight and every scalar step, matches to the bit. New checked-in
  `data/sample_calibration_noisy.ndjson` (the clean design plus one gross outlier)
  on which plain OLS blows out to `coef≈80 / perm≈-24` while the Huber fit holds
  near the true `10 / 20`.
- **`--huber-delta` / `--ridge-lambda` / `--max-iters`** on both `calibrate` CLIs
  (`xexec` / `xexeclab`): either robustness flag switches to the robust fit.
- **Outlier injection in `synthetic_calibration`** (`--outlier-frac` /
  `--outlier-bps`): corrupt a share of generated fills with a shock, so the
  synthetic replay exercises the robust fit end to end. `outlier_frac=0` leaves the
  draw sequence unchanged (back-compat).

### Changed
- Test suite grows to 77 (24 Rust + 53 Python): a sixth cross-language
  equivalence test plus robust-calibration unit tests (Huber recovers the sign OLS
  flips, ridge makes a thin design solvable, and the no-option path reproduces OLS
  exactly) and the outlier-generator round trips.
- **Honest disclaimer** updated: the robust fit **bounds** an outlier's influence
  rather than rejecting it, so under one-sided contamination it lands much closer
  to the truth than OLS but not exactly on it — stated plainly.
- Rust `polars` dependency gains the `abs` feature (used by the Huber residual
  weighting); no new crates.

## [0.7.0] - 2026-08-05

### Added
- **Calibration harness — fit the impact coefficients from realised fills.**
  `impact_curve` consumes the two Almgren-Chriss coefficients; `calibrate_impact`
  is the other half that recovers them. Given realised fills, each tagged with the
  fraction of volume it took (`participation`) and the cost it actually paid
  (`realised_bps`), it fits `realised_bps ~ coef_bps * sqrt(participation) +
  perm_coef_bps * participation` as an ordinary-least-squares regression through
  the origin, and reports the fitted `coef_bps` and `perm_coef_bps` plus
  `rmse_bps` and `r_squared` diagnostics. Every quantity the normal equations need
  is a sum, so the fit is a shared Polars aggregation plus a 2x2 solve, computed
  with the same expressions in the Rust crate (`src/calibrate.rs`) and Python
  (`engine.py`) and held identical by a **fifth** cross-language equivalence test
  — the recovered coefficients *and* the diagnostics match bit-for-bit. New
  `CalibrationSample` schema and a checked-in `data/sample_calibration.ndjson`
  (costs are exactly `10*sqrt(p) + 20*p`, so the fit recovers 10 and 20). A
  near-singular design (a single participation level, which cannot separate the
  two terms) is refused rather than returning a blown-up coefficient.
- **`calibrate`** subcommand on both CLIs (`xexec calibrate` / `xexeclab
  calibrate`): read a realised-fill replay, print the fitted `CalibrationSummary`.
- **`synth-calibration`** CLI + `synthetic_calibration` generator: write a
  deterministic realised-fill replay from known coefficients (plus optional
  Gaussian noise), so the harness runs end to end — with zero noise the fit
  recovers the generating coefficients to floating-point precision.

### Changed
- Test suite grows to 66 (20 Rust + 46 Python): a fifth cross-language
  equivalence test plus calibration unit and round-trip tests.
- **Honest disclaimer** updated: the impact coefficients are **no longer
  external-only** — `calibrate` fits them from a desk's own realised fills. The
  remaining limitation is that the fit is only as good as the realised costs fed
  in, and it calibrates the two-coefficient Almgren-Chriss model rather than
  discovering the model.

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
