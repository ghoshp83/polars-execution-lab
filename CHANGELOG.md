# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project adheres to
[Semantic Versioning](https://semver.org/).

## [0.31.0] - 2026-09-23

### Added
- **`pov-sweep`: the breakeven rate as a shape, not a number.** v0.30.0 derived
  `breakeven_bps` — what a missed unit would have to be worth for the verdict to
  flip — but reported it at the one cap the run was given. The cap is a desk's
  own risk parameter, not a property of the market, so a threshold quoted at a
  single cap is quoted at an arbitrary point: two desks asking the same question
  of the same session get answers that do not compare. The new command re-runs
  `pov-forecast` across a grid of caps and lines the answers up, exactly as
  `sensitivity` does for the impact coefficient. On the bundled thin capture at
  `--cap-grid 0.05,0.1,0.15,0.2,0.25` the rate runs **7.72908423 bps at a 10% cap
  down to 3.56522873 at 25%** — the v0.30.0 figure turns out to be the loosest
  point of the ladder, and a desk reading only that understates its own bar by
  more than a factor of two. New module `src/capsweep.rs` and `cap_sweep` in
  `python/xexeclab/engine.py`, wired to both CLIs as `pov-sweep --cap-grid`.
  It deliberately accepts no `--shortfall-bps`: a caller who has a rate wants
  `net_bps` at their own cap, not a ladder.
- **The dominance finding, promoted from a comment to a reported field.**
  v0.30.0 recorded, in prose, that under the forward carry a plan which gets more
  away early both misses less *and* pays less per unit filled — so where only a
  thin tail binds, one plan dominates outright and no non-negative rate can
  reverse it. That is now `dominated_points` on the sweep, beside
  `like_for_like_points` and `traded_off_points`. The three counts **partition**
  the grid, which is a property a test pins rather than a claim the docs make:
  a reader can see at a glance how much of a ladder a rate actually decides.
  `like_for_like_from_cap` reports the threshold above which the two plans stop
  differing at all, scanned from the top of the grid down so it is a threshold
  and not the first of several disconnected stretches.
- Eight tests each side, and a twenty-first cross-language equivalence test. The
  sweep is the first report whose shape depends on a comparison *across* runs
  rather than one run's arithmetic, so the equivalence test pins the three counts
  and not only the figures beneath them: a rounding difference too small to move
  any single number could still move a point from one count to another.

### Changed
- README documents `pov-sweep` with a fourth usage block and extends the
  `pov-forecast` limitation to say plainly that the v0.30.0 threshold was quoted
  at one arbitrary cap.

### Notes
- Polars 2.0 is still not adoptable and this release does not change that. Both
  tripwires were re-measured on 2026-09-23: the Rust crate's newest stable is
  0.55.2 and the Python package's is 1.44.2, neither 2.0-ready. The weekly
  advisory job remains the thing that will say when it changes.

## [0.30.0] - 2026-09-22

### Added
- **`breakeven_bps`: the rate the engine can derive, instead of the one it
  cannot.** v0.29.0 could net a gain against a shortfall only when the caller
  supplied a price for the missed quantity, and nothing derives that price — so
  with no rate, nothing ranked the two plans. Each `CappedGain` now also reports
  `improvement_bps * parent_qty / shortfall_qty`: the rate at which the two
  cancel. It is arithmetic on figures already reported, not a judgement about the
  order, and it turns an unanswerable question ("what is a missed unit worth?")
  into one a desk can answer about its own mandate ("is it worth more or less
  than this?"). `null` when nothing was missed, and `null` when one plan is both
  cheaper per unit filled and missed less, since no non-negative rate reverses
  that.
- **`data/sample_ticks_thin.ndjson`: the first bundled capture that leaves a
  remainder.** Every other sample session has the volume to absorb the parent, so
  `shortfall_qty` was zero throughout and the netting fields were exercised only
  by tests — a gap v0.28.0 opened and v0.29.0 confirmed. This session's volume
  collapses after the first second, so the forward carry has nowhere later to put
  what the cap deferred. At `--cap 0.25` against the pooled history the pooled
  plan is **0.40579986 bps cheaper per unit filled and misses 0.02276431 more**
  of a 0.2 parent; the two tie at **3.56522873 bps**. Under the reshape the
  comparison is exactly flat: water-filling pins both thin buckets at the cap
  whatever shape the plan had, so the plans coincide and `improvement_bps` is 0.
- Four tests each side and a twentieth cross-language equivalence test, the first
  run on a capture where the two engines have a non-zero shortfall to divide.

### Changed
- The `pov-forecast` limitation in the README no longer says the bundled history
  cannot exercise the shortfall — it now names the capture that does, and reports
  the measured numbers above.

### Notes
- **Polars 2.0 is still not adoptable** and this release does not change that:
  the Rust crate remains the binding constraint. The weekly `upstream` tripwire
  is live and will go red the week both crates.io and PyPI ship a stable 2.0.

## [0.29.0] - 2026-09-21

### Added
- **A price for the quantity a plan misses, supplied by the caller.** v0.28.0
  reported `improvement_bps` and `shortfall_qty` side by side and refused to net
  them, because basis points and unfilled quantity do not convert. They convert
  once someone says what missing a unit of the parent costs, and that is a
  property of the order and its mandate, not of any session the engine has seen.
  `pov-forecast --shortfall-bps` takes that rate; each `CappedGain` then also
  reports `net_bps` — `improvement_bps - shortfall_qty / parent_qty *
  shortfall_bps` — and the rate is echoed back as `shortfall_bps` on the report.
  Omitted, both fields are `null` and nothing is netted. A rate of `0` is a
  different statement from no rate at all and the report keeps them apart. This
  is the first field in the engine that can be absent.
- Four tests each side and a nineteenth cross-language equivalence test — the
  first over an optional field, so serde's `null` and `json.dumps(None)` are
  checked to agree as well as the arithmetic does.

### Changed
- The `pov-forecast` limitation in the README now says why the engine will not
  rank the two capped executions for you, what supplying a rate does and does not
  buy, and that **the bundled history does not exercise it**: at every cap the
  pooled and naive plans miss the same quantity, so `shortfall_qty` is zero and
  `net_bps` equals `improvement_bps` there. The tests carry the non-zero case.
- 135 Rust tests and 200 Python (19 of them cross-language equivalence).

## [0.28.0] - 2026-09-20

### Added
- **A capped twin for `improvement_bps`.** v0.27.0 reported both capped
  executions on `pov-forecast` but left the headline comparison uncapped, because
  the executions had no figure that could be subtracted: their `impact_bps` is
  per unit of *parent*, so an execution the cap left short reports a smaller
  number for having traded less, and the difference would have rewarded missing
  the order. `filled_impact_bps` on `CappedReplay` and `SpreadPlan` re-bases that
  impact onto the quantity that actually filled, and `capped_improvement` uses it
  to ask the pooling question inside the cap, once per execution: an
  `improvement_bps` per unit traded, a `shortfall_qty` for what the pooled plan
  missed that the naive one did not, and `like_for_like` when that shortfall is
  zero. The two are reported side by side rather than netted, because basis
  points and unfilled quantity are different currencies.
- Four tests each side (131 Rust, 195 Python, 18 of them cross-language
  equivalence): that `filled_impact_bps` does not fall with the shortfall, that
  the capped comparison reports rather than absorbs a plan that filled less, that
  a slack cap gives the uncapped answer back, and that a session too thin for the
  parent leaves the forecasts nothing to win.

### Changed
- The `pov-forecast` limitation in the README now carries the capped comparison
  and its measured numbers. On the bundled history at `--cap 0.14` the 0.39 bps
  uncapped `improvement_bps` is worth 0.02 bps under the replay and 0.11 under
  the reshape, both `like_for_like` — the two executions disagree by a factor of
  five about what the same pooling bought, which is why neither is reported alone.

## [0.27.0] - 2026-09-19

### Added
- **`pov-forecast` reports both capped executions, for both plans it prices.**
  Since v0.26.0 `pov-backtest` has shown what a desk that respects the cap would
  have got, two ways: a `capped` replay that defers the excess and a `spread`
  that re-shapes the plan to fit. `pov-forecast` showed neither, so a pooled plan
  could come back `feasible: false` with nothing said about what staying inside
  the cap would have cost. `forecast_capped` and `naive_capped` now carry both,
  computed by the same two functions the backtest uses, so no command can charge
  the same allocation differently.
- **Four tests each side.** The single-history collapse now has to hold with the
  cap binding. The reshape completes for two forecasts that disagree about every
  slot, so under it the shortfall is a property of the session; a cap the session
  cannot fill misses the same half either way; and the forward carry is what
  still charges a forecast for its shape. 127 Rust, 191 Python.

### Changed
- **The README says what `improvement_bps` does not price.** It compares the
  *uncapped* allocations, so it can credit a pool for volume the cap would never
  have let it take. On the bundled history the cap is slack and both executions
  are inert; at `--cap 0.14` the naive plan breaches where the pooled one does
  not, and the 0.39 bps improvement becomes 0.02 bps under the replay.
- **GitHub Actions off deprecated Node 20.** `actions/checkout` v4 → v5 and
  `actions/setup-python` v5 → v6 in both workflows; GitHub was already
  force-running them on Node 24 and warning on every run.

## [0.26.0] - 2026-09-18

### Added
- **`pov-backtest` reports a `spread` plan beside the `capped` replay.** The
  capped replay only carries quantity forward, so a cap that binds near the
  close leaves a remainder an earlier bucket had the volume to absorb — its
  `unfilled_qty` is an upper bound, not the least a capped desk must miss. The
  `spread` water-fills the same plan instead: sizes stay proportional to the
  plan wherever the cap is slack, every binding bucket is pinned at `cap` times
  its traded volume, and what those buckets cannot take is re-spread over the
  rest in the same proportions. Feasible by construction, and it fills the whole
  parent whenever `parent_qty <= cap * exec_volume`. Same fields as the capped
  replay, identical in both engines, covered by the `pov-backtest` equivalence
  test.
- Tests on both sides: the reshape splits the pinned bucket's excess by planned
  proportion rather than into the next bucket, it completes a parent the forward
  carry leaves short, it misses exactly the quantity the session had no volume
  for, and a cap that never binds collapses back to the uncapped backtest.

### Changed
- The README states what each of the two capped executions bounds, and that
  neither is the cheap plan — pinning a bucket at the cap moves participation
  away from the oracle.

## [0.25.0] - 2026-09-17

### Added
- **`pov-backtest` reports a `capped` replay.** An infeasible backtest said the
  forecast breached the cap, but not what a desk that respects the cap would
  have got. The same plan is now also executed slot by slot, taking at most
  `cap` of each slot's traded volume and carrying the rest forward. It reports
  `filled_qty`, `unfilled_qty`, `completed`, `capped_slots`, the price,
  tracking, impact and per-slot sizes. Identical in both engines and covered by
  the existing `pov-backtest` equivalence test.
- Tests on both sides: the deferral lands in the next slot, a cap too tight
  for the close leaves a reported remainder, and a cap that never binds
  collapses back to the uncapped backtest.

### Changed
- README: the `pov-backtest` limitation now states that the capped replay
  knows each bucket's volume in advance and only carries forward.

## [0.24.0] - 2026-09-16

### Added
- **`python/xexeclab/upstream.py` watches for a stable Polars 2.0** on both
  crates.io and PyPI, using only the standard library. Release candidates never
  count, and versions compare numerically, not as text. It exits 1 only when
  both engines can move.
- A weekly `upstream` workflow runs it, so CI goes red the week migration
  becomes possible. Today: crate 0.55.2, PyPI 1.44.2 stable (2.0.0rc1 ignored).
- Tests for the watch's version logic.

### Changed
- The README's Polars 2.0 note now carries the ordered migration checklist.

## [0.23.0] - 2026-09-15

### Added
- A sweep test over two identical snapshots pins that the running size total is
  windowed per snapshot -- the one expression the upgrade had to touch.

### Changed
- **The Rust engine moves from `polars` 0.44 to 0.55.2**, the latest crate on
  crates.io, keeping the same minimal feature set (`lazy`, `abs`, `cum_agg`).
  The one source change: a window expression's `.over(...)` now returns a
  `Result`, so the order-book sweep propagates it. The full Rust suite and all
  18 cross-language equivalence tests pass unchanged against the new binary --
  eleven minor versions of the engine moved and not one reported value did.
- The README's Polars 2.0 disclaimer now states the current crate pin.

## [0.22.0] - 2026-09-14

### Added
- **`python/tests/test_precision.py` pins the `r8` magnitude ceiling.** Every
  reported value is rounded to 8 absolute decimals, but a double's spacing grows
  with magnitude: below 2^26 it is finer than 1e-8 and every 8th decimal is
  resolved; above it a 1e-8 step can vanish, and near 1e9 the resolution is
  about 1.2e-7. Past 2^53 / 1e8 the rounding is the identity.

### Changed
- The README's byte-identical claim now says it concerns 8dp-rounded values, and
  the honest disclaimer states the ceiling: prices, sizes, basis points and
  ratios sit far below it, volume and notional totals can cross it. The engines
  still agree bit for bit above it -- the equivalence tests prove agreement, not
  resolution.

## [0.21.0] - 2026-09-13

### Added
- **A cross-version bit-identity check -- `python -m xexeclab.compat`.** The
  advisory Polars 2.0 CI job ran the test suite only, and its fixtures are a few
  ticks long: too small for the streaming engine to partition, so a change in how
  a column is summed could never show up there. The module builds a
  deterministic 400,000-tick capture, fingerprints every `summary` and
  `pov_schedule` value as an exact hex float, and compares a fingerprint taken
  under Polars 1.x with one taken under 2.0.

  Known differences are recorded in `python/tests/polars2_baseline.json`; the
  check passes while the versions differ exactly as recorded and fails on any
  new, changed, or vanished difference, so it never settles into a permanently
  red job that people learn to ignore. On 1.43.2 vs 2.0.0-rc.1 all 60,507 values
  match, and the baseline is empty.

### Changed
- The `polars2` CI job now runs the check after the test suite, building a
  second environment on Polars 1.x to compare against.

## [0.20.0] - 2026-09-12

### Added
- **Recency-weighted pooling -- `--half-life` / `half_life` on `pov-forecast`.**
  v0.19.0 pooled the history sessions with equal weight, which is the right
  estimator for a profile that is merely noisy and the wrong one for a profile
  that is drifting: a week-old session votes as loudly as yesterday's. The
  forecast is now a **weighted** mean of the history share profiles, with a
  session `age` sessions back weighted `0.5 ** (age / half_life)` and the weights
  normalised over the history.

  The parameter is the dial between the two plans `pov-forecast` already prices:
  `--half-life 0` (the default, and the v0.19.0 behaviour) pools every session
  equally, and a half-life far below one session drives the weight onto the last
  session, where the forecast *is* the naive baseline. Both ends are pinned by
  tests. The result reports `half_life` and the normalised `weights` it used, so
  the plan can be re-derived from the output.

  Which half-life is right is a property of the market, not of the engine:
  `improvement_bps` is how a desk would choose it, session by session.

### Fixed
- **`_measured` totalled bucket volume with the built-in `sum`.** From Python
  3.12 that uses compensated summation and Rust's plain running sum does not, so
  the two engines' totals could differ in the last bit and diverge after
  rounding. It is an explicit loop now, as the rest of the mirrored host-side
  arithmetic already was. CI pins Python 3.11, where the two agree, so this could
  only ever have been hit by a user on 3.12 or newer -- the package supports
  `>=3.11`.

## [0.19.0] - 2026-09-11

### Added
- **Pooled volume forecasts -- `pov-forecast` / `pov_forecast`.** `pov-backtest`
  (v0.18.0) judges the naive forecast: one earlier session's profile, used
  unchanged, with every burst and lull of that one session baked into the plan.
  This builds the plan from the **mean share profile of several history
  sessions** (`--history a.ndjson,b.ndjson`, oldest first) and prices it against
  the execution session beside two references -- the naive forecast (the most
  recent history session alone) and the oracle.

  Sessions are pooled by **share, not by volume**, so a session that traded ten
  times as much cannot outvote the others. The result carries a `PlanScore` for
  each plan (`profile_distance`, `price`, `tracking_bps`, `impact_bps`,
  `max_participation`, `feasible`, `forecast_cost_bps`) and `improvement_bps`,
  the naive plan's impact minus the pooled plan's.

  `forecast_cost_bps` is still never negative -- the oracle is the minimum under
  this cost model. `improvement_bps` has **no such guarantee**: when a session
  repeats the most recent one exactly, pooling only adds error and the number
  goes negative. A test pins that case, so the claim that averaging helps is
  checked per session rather than assumed.

- **A third bundled session, `data/sample_ticks_third.ndjson`**, whose shape sits
  between the first two. Pooling `sample_ticks` and `sample_ticks_next` forecasts
  it more closely than `sample_ticks_next` alone does.

- **Seventeenth cross-language equivalence test.** The first over an estimate
  built from several captures: the engines must agree on every session's
  profile, on the order the shares are averaged in, and on two plans priced
  against a third session.

### Changed
- The out-of-sample pricing is factored into one path (`price_plan`, with the
  oracle and the rounded `PlanScore` beside it) on both engines, so the backtest
  and the forecast cannot charge the same allocation differently. `pov_backtest`
  output is unchanged.

### Notes
- The mean is summed in an explicit loop, session by session, on both engines.
  Python's built-in `sum` uses compensated float summation from 3.12, which Rust
  does not, so it would not be bit-identical.
- Still no new Polars feature flags.

## [0.18.0] - 2026-09-10

### Added
- **Out-of-sample volume plans -- `pov-backtest` / `pov_backtest`.** `pov-plan`
  (v0.17.0) allocates against the volume a capture *already* traded, so both of
  its properties -- constant participation and exact VWAP tracking -- hold only
  inside the capture the plan was built from. Used as a forward schedule, the
  plan meets a different session. This replays the allocation derived from one
  capture (`--plan-input`) against another (`--input`), lining the buckets up by
  time into the session rather than by wall clock, and reports what the forecast
  error did: per-bucket participation against the volume that actually arrived,
  whether any bucket breached the cap, how far the tracking drifted from the
  session VWAP, and the total-variation `profile_distance` between the shapes.

  The comparison point is the **oracle** -- the volume-following plan built on
  the execution session itself, as if its volume had been known. Under the
  two-term law, equal participation in every bucket minimises impact per unit of
  parent, so the oracle is the cheapest allocation the cost model admits and
  `forecast_cost_bps` (`impact_bps - oracle_impact_bps`) can never be negative.
  It is reported rather than assumed, so a regression surfaces as a negative
  number.

  A cap breach is **reported** (`feasible: false`) rather than refused: the
  backtest exists to show what the forecast did, and a breach is the most
  important thing it can do.

- **A second bundled session, `data/sample_ticks_next.ndjson`.** The same product
  a minute later, over the same three seconds of the clock, with a thin middle
  second. Planned on `sample_ticks.ndjson` with `--parent-qty 0.2 --cap 0.25`,
  the plan that took a constant 11.6% of every bucket in sample takes 34% of
  that thin second -- over the cap -- while the oracle stays inside it.

- **Sixteenth cross-language equivalence test.** The first over two captures:
  the engines must agree on both profiles, on their alignment, and on the price
  of the difference. It also checks `forecast_cost_bps >= 0` against the Rust
  output.

### Changed
- `pov_schedule`'s argument checks and capture measurement are factored into
  helpers shared with `pov_backtest` on both engines. Refusals and their
  wordings are unchanged.

### Notes
- Still no new Polars feature flags: the two profiles are aligned by slot index
  and assembled into one frame, not joined, and `abs` was already enabled.

## [0.17.0] - 2026-09-09

### Added
- **Volume-following execution plans -- `pov-plan` / `pov_schedule`.** `schedule`
  (v0.12.0) chooses a trajectory against a single assumed `per_slice_volume`:
  one number, held to be true of every interval. Real volume is not flat, and a
  clock-uniform slice dropped into a thin interval is a large share of a small
  market -- which is precisely what the square-root impact law charges most for.

  This plan is derived from measured volume instead. The capture is bucketed
  (the same buckets `bars` builds), and each bucket receives the parent order in
  proportion to the volume it actually traded. Two properties follow from that
  allocation, and both are reported rather than assumed:

  1. **Participation is constant by construction.** Allocating
     `parent_qty * volume_i / total_volume` into a bucket holding `volume_i`
     leaves `parent_qty / total_volume` in every bucket, whatever the profile
     looks like. So the participation cap is one scalar check -- there is
     nothing to redistribute, and no water-filling loop for the two engines to
     disagree about.
  2. **It tracks the session VWAP exactly.** The achieved price is the
     volume-weighted mean of the bucket VWAPs, which is the session VWAP itself.
     `pov_tracking_bps` is therefore zero, and it is reported so that a
     regression shows up as a number rather than as silence.

  The clock-uniform allocation of the same quantity over the same buckets is
  priced alongside it with the same two-term impact law. It has neither
  property, and where it breaches the participation cap the output says so
  (`twap_feasible: false`) instead of clipping it and calling the result a
  benchmark.

- **Fifteenth cross-language equivalence test.** The first over a plan derived
  from the replay rather than from parameters alone: the engines have to agree
  on the volume profile before they can agree on the allocation. Alongside the
  exact equality it checks constant participation and exact VWAP tracking
  against the Rust output, not only the Python one.

### Notes
- No new Polars feature flags. The profile is a `group_by` over an integer
  bucket column, which the crate's minimal feature set already covers.
- Named `pov-plan`, not `pov`, because `simulate --algo pov` already fills a POV
  order; this one plans one from measured volume rather than simulating it.

## [0.16.0] - 2026-09-08

### Added
- **Streamed sessions -- `stream` / `stream_session`.** Every other command in
  this project starts by reading the whole replay into memory: Python calls
  `pl.read_ndjson`, Rust deserialises the file into a `Vec`. That is fine for a
  fixture and wrong for a real capture -- a day of trades on a liquid product
  does not fit the machine that wants the VWAP of it.

  So the capture is folded in chunks of `chunk_rows`. Each chunk is aggregated
  with the Polars engine -- the same `sum` expressions `session_vwap` and
  `order_flow` use over a whole frame -- and reduced into a fixed-size
  accumulator. Only one chunk plus one carried tick is ever resident, and
  `peak_rows_in_memory` reports that bound in the output, so the claim is a
  number rather than a sentence in a README. On the bundled 13-row sample:
  4 chunks, 5 rows resident.

  The carried tick is the subtle part. A sample-and-hold TWAP weights each price
  by the gap to the *next* trade, and at a chunk boundary that next trade is in
  the following chunk. Dropping it would silently lose one interval per boundary
  and drift the answer as a function of the chunk size, so the last tick of each
  chunk is carried across. A test folds the same file at `chunk_rows` 1 through
  1000 and requires every reported number to be identical: a chunk size is a
  memory setting, and a memory setting that moves the VWAP is a bug.

  A streamed pass cannot sort what it has not seen, so unlike the in-memory
  readers -- which sort by `ts_ns` -- `stream` requires a capture already in time
  order and refuses one that is not, rather than pricing shuffled gaps as real.
  Parquet is refused by extension for the same honesty: it is a columnar sink,
  not a line-oriented stream.

- **Fourteenth cross-language equivalence test.** Both engines fold
  `data/sample_ticks.ndjson` in the same chunks and must return identical
  reports -- chunk count and residency bound included, so an engine that
  quietly read the whole file would fail even if its VWAP agreed.

- **Polars 2.0 forward-compatibility job in CI (advisory).** Polars 2.0rc1 is
  out, and its headline change is that the streaming engine becomes the default
  for every `LazyFrame` query. The Python engine here already runs on it: the
  full suite passes unchanged under `2.0.0-rc.1`, since none of 2.0's breaking
  changes (stricter type coercion, stricter `concat` height checks, removed
  ambiguous casts) touch the expressions this project uses. A new
  `continue-on-error` CI job keeps checking that, so the claim stays verified
  rather than becoming a note about one afternoon.

### Notes
- **The project is deliberately not pinned to Polars 2.0.** Two reasons. It is a
  release candidate, and the stable is still weeks out. More importantly the
  Rust `polars` crate has not gone 2.0 -- crates.io tops out at 0.55.2, and this
  crate pins 0.44 -- so pinning the Python half to 2.0 would put the two engines
  on different generations and leave the cross-language equivalence tests
  comparing across a version boundary rather than proving one engine. The
  dependency stays `polars>=1.0`; the advisory job watches the gap.
- The Python test suite validates this release's fold against Polars' own
  out-of-core engine (`scan_ndjson(...).collect(engine="streaming")`) on the
  session sums. The fold exists because that engine has no notion of the
  sample-and-hold carry across a chunk boundary; where it *can* answer, it is
  used as the reference.
- The Rust crate's Polars feature set is unchanged (`lazy`, `abs`, `cum_agg`).
  No `json`, `parquet` or `streaming` features were added.

## [0.15.0] - 2026-09-07

### Added
- **Coefficient sensitivity -- `sensitivity` / `sensitivity`.** v0.14.0 reports
  `edge_bps` as though the temporary-impact coefficient were known. It is not: it
  is fitted, by `calibrate` or `curve`, from noisy data. So the same comparison is
  re-run across a grid of `coef_bps` and the answers are lined up, which lets a
  desk ask whether a verdict is a property of the execution or of the number that
  was fed into it.

  `verdict_stable` is the headline, and it is deliberately strict: true only when
  every point on the grid picks the same best alternative *and* agrees on the sign
  of the edge. A constant winner is not enough -- a comparison that shrinks
  through zero has changed its answer even if the same benchmark keeps winning.
  An edge of exactly zero is a tie, and a tie is not a stable verdict either.

  When the sign does flip, `breakeven_coef_bps` is the coefficient at which it
  does. That number is exact rather than approximate: the edge is affine in
  `coef_bps` (drift does not depend on it, and both impact terms are linear in
  their coefficients), so interpolating between the two bracketing grid points
  lands on the true crossing. A test asserts this by re-running `counterfactual`
  at the reported breakeven and requiring the edge back to be zero.

  `perm_coef_bps` is held fixed: this sweeps one axis, not the plane.

  On the bundled sample the verdict is stable -- the realised schedule loses to
  volume-following at every coefficient from 10bps to 30bps, by between 0.73 and
  0.94bps. That result is kept as it is: the loss reported in v0.14.0 is a
  property of the schedule, not of the calibration.
- `sensitivity` on both CLIs, reading the same realised-fill replay as
  `counterfactual` and taking the grid as `--coef-grid 10,20,30`.

### Changed
- The cross-language equivalence suite gains a thirteenth test. It compares the
  whole grid point by point, not just the summary, because a disagreement at a
  single coefficient can flip `verdict_stable` and invent or erase a breakeven.

## [0.14.0] - 2026-09-05

### Added
- **Counterfactual scheduling -- `counterfactual` / `counterfactual`.** v0.13.0
  split a realised execution into the cost its size always implied and the cost
  it did not, but that residual is a bucket. This release asks how much of it the
  *schedule* earned. The quantity that actually filled is re-allocated across the
  same intervals as a plain TWAP and as a volume-following participation, priced
  on the same prices, the same traded volumes and the same two-term impact law,
  and the three are ranked. `edge_bps` is the headline -- the gap to the best
  benchmark, so a positive number means the realised schedule was the better one.

  Holding quantity fixed is what makes the comparison fair: the unfilled
  remainder is identical under every strategy, so its opportunity cost cancels
  and is deliberately not reported here (`shortfall` is where that lives). Each
  strategy reports `drift_bps` and `impact_bps` separately, so a desk can see
  whether it lost on *when* it traded or on *how much* it took at once, with the
  per-interval `legs` carrying the same split.

  An allocation that would take more than an interval ever traded is refused, not
  priced: the impact law is undefined above full participation, and a benchmark
  the market could not have filled is fiction, not a comparison.
- `counterfactual` on both CLIs, reading the same realised-fill replay as
  `shortfall` and needing only the arrival price and the impact coefficients --
  the benchmark quantity comes from what actually filled.

### Changed
- The cross-language equivalence suite gains its twelfth test. Three strategies
  are priced and then *ranked*, so a last-place disagreement in any one of them
  can flip `best_alternative` outright; the test pins the ordering as well as the
  numbers.

## [0.13.0] - 2026-09-03

### Added
- **Post-trade attribution -- `shortfall` / `shortfall`.** v0.12.0 chose an
  execution trajectory; this release scores one after the fills come back, and
  closes the loop the repo has been building: measure the book, fit the
  coefficients, price a schedule, choose a schedule, then judge what actually
  happened against the model that chose it. `shortfall` reads realised child
  fills (`ts_ns`, `side`, `qty`, `price`, `interval_volume`), measures the
  implementation shortfall of the filled quantity against the arrival (decision)
  price, then prices those same fills again through the *same* two-term law the
  rest of the repo uses -- `coef_bps * sqrt(participation)` temporary plus
  `perm_coef_bps * participation` permanent -- and differences the two.

  The difference is the point. `realised_bps` on its own is not a measure of
  execution quality: it rewards whoever happened to be handed the small orders.
  `modelled_bps` is the cost the parent's size was always going to pay and that
  no algorithm avoids; `residual_bps` is what is left over -- timing, venue
  selection, spread capture, adverse selection, luck -- and it is the only part
  of the number a desk can be held to. The per-fill `slices` carry the same three
  columns, so a single bad child is visible rather than averaged away.

  Quantity that never filled is charged, not dropped. `opportunity_bps` prices
  the unfilled remainder at the drift from arrival to the last fill, weighted by
  its share of the parent, so an algorithm cannot improve its average price by
  simply not finishing. `realised_bps` is quoted on the filled notional and
  `total_bps` on the parent; `total_bps` is the honest headline.

  Reconciliation errors are refused rather than reported: fills that mix sides in
  one parent, a child that took more than its interval held, and fills totalling
  more than the parent all raise, because a participation or a fill rate above 1
  is a data problem that a summary statistic would bury.
- `read_fills` on both engines and a `Fill` record type, plus the
  `data/sample_fills.ndjson` replay the tests and the README run against.
- `xexec shortfall` and `xexeclab shortfall` on both CLIs, each requiring
  `--parent-qty` and `--arrival` explicitly.

### Changed
- The cross-language equivalence suite gains its **eleventh** test, and the most
  numerically fragile one: an attribution is a difference of two numbers of
  similar size, so a last-place disagreement in either leg surfaces whole in
  `residual_bps`. Both legs are asserted non-zero so the two engines cannot pass
  by agreeing on a trivial attribution.

## [0.12.0] - 2026-09-02

### Added
- **The execution schedule itself -- `schedule` / `optimal_schedule`.** Every
  release so far *measured* an execution: `sweep` prices the book, `calibrate`
  and `curve` recover the impact coefficients, `impact` prices a participation
  schedule someone else chose. None of them chose one. This release closes the
  loop. Trading a parent order quickly concentrates size into few intervals and
  pays more impact; trading it slowly leaves inventory exposed to the mid
  wandering away. Almgren-Chriss is the statement that those two costs trade off
  and the total has a minimum, and `optimal_schedule` finds it: a candidate
  trajectory is an exponential front-load with urgency `k` -- slice `i` of `n`
  weighted by `exp(-k * i / n)`, normalised, so `k = 0` is exactly a TWAP -- and
  each candidate on a fixed grid is priced with the *same* two-term law the rest
  of the repo uses (`coef_bps * sqrt(participation)` temporary,
  `perm_coef_bps * participation` permanent, each weighted by the fraction of the
  parent that slice trades so the totals are in bps of the parent) plus the
  timing-risk term `sigma_bps * sqrt(mean(remaining^2))`. The summary reports the
  chosen `urgency`, the `impact_bps` / `risk_bps` split, the TWAP it was measured
  against and the `saving_bps` between them, and the whole per-slice trajectory:
  `weight`, `size`, `participation`, `temp_bps`, `perm_bps`, and the `remaining`
  inventory the risk term prices.

  The **grid search is deliberate**, not a shortcut. The classic closed form is
  derived under *linear* temporary impact -- an assumption this repo's own
  measured cost curve contradicts -- so the trajectory is searched under the
  concave law the book actually charges rather than inherited from a solution
  that does not apply. A candidate that overruns the volume available in an
  interval is infeasible, not free: it is skipped, and if even the uniform
  schedule overruns, the call is **refused** rather than reporting a cheap
  schedule that cannot be traded. With `sigma_bps = 0` the concave law makes
  uniform trading strictly cheapest and the optimiser returns the TWAP, so a
  spurious front-load is impossible.

  Implemented in the Rust crate (`src/schedule.rs`) and Python (`engine.py`),
  held identical by a **tenth** cross-language equivalence test -- the first over
  a *decision* rather than a measurement: both engines price 41 candidates and
  pick one, so agreeing means agreeing on every candidate to 8dp and on the
  tie-break, since a single candidate off in the last place returns a wholly
  different schedule. New `xexec schedule` / `xexeclab schedule` subcommands
  (`--slices`, `--total-size`, `--slice-volume`, `--coef-bps`,
  `--perm-coef-bps`, `--sigma-bps`) -- the first command that plans rather than
  measures, and so the only one that reads no replay file.

### Changed
- README architecture diagram and value prop now carry the schedule stage.
- Honest disclaimer records what the optimiser is not: a grid search over a fixed
  urgency ladder under this repo's own cost model, only as good as the three
  coefficients fed to it, with the per-interval volume taken as a constant rather
  than forecast -- and a reported urgency of 4.0 means the optimum sits at the
  edge of the grid.

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
