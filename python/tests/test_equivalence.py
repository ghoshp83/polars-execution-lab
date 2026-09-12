"""The signature test: the Rust engine and the Python engine must produce
identical execution summaries on the same replay. This is what turns "one
engine, two languages" from a claim into a verified property.
"""

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

from xexeclab.engine import (
    calibrate_impact,
    calibrate_impact_robust,
    counterfactual,
    depth_metrics,
    impact_curve,
    optimal_schedule,
    pov_backtest,
    pov_forecast,
    pov_schedule,
    queue_metrics,
    quote_metrics,
    read_book,
    read_calibration,
    read_fills,
    read_impact,
    read_quotes,
    read_ticks,
    sensitivity,
    session_twap,
    session_vwap,
    shortfall,
    stream_session,
    summary,
    sweep_cost,
    sweep_curve,
)

pytestmark = pytest.mark.equivalence

SAMPLE = "data/sample_ticks.ndjson"
# A later session of the same product, over the same three seconds of the clock
# but with a thin middle second: the session a plan built on SAMPLE has to meet.
NEXT_SAMPLE = "data/sample_ticks_next.ndjson"
# A third session whose shape sits between the first two: the one a forecast
# pooled from both has to meet.
THIRD_SAMPLE = "data/sample_ticks_third.ndjson"
QUOTE_SAMPLE = "data/sample_quotes.ndjson"
BOOK_SAMPLE = "data/sample_book.ndjson"
IMPACT_SAMPLE = "data/sample_impact.ndjson"
CALIBRATION_SAMPLE = "data/sample_calibration.ndjson"
FILL_SAMPLE = "data/sample_fills.ndjson"
NOISY_CALIBRATION_SAMPLE = "data/sample_calibration_noisy.ndjson"
HUBER_DELTA = 3.0
SWEEP_SIZE = 2.0
CURVE_SIZES = [0.5, 1.0, 2.0, 3.0, 10.0]
SCHEDULE = dict(
    slices=6, total_size=3.0, per_slice_volume=2.0, coef_bps=25.0, perm_coef_bps=5.0, sigma_bps=8.0
)
SHORTFALL = dict(parent_qty=3.5, arrival_price=30000.0, coef_bps=25.0, perm_coef_bps=5.0)
COEF_GRID = [10.0, 15.0, 20.0, 25.0, 30.0]
# Deliberately smaller than the 13-row sample, so the fold really chunks.
STREAM_CHUNK_ROWS = 4
# 0.2 against the sample's 1.73 of traded volume: an eighth of the capture, well
# inside the cap, and small enough that the clock-uniform benchmark is feasible
# too -- so the comparison is between two tradeable plans, not against a fiction.
POV_PLAN = dict(parent_qty=0.2, cap=0.25, coef_bps=25.0, perm_coef_bps=5.0)
BUCKET_MS = 1000
COEF_BPS = 12.5
PERM_COEF_BPS = 7.5


def _find_binary() -> str | None:
    env = os.environ.get("XEXEC_BIN")
    if env and Path(env).exists():
        return env
    for candidate in ("target/release/xexec", "target/debug/xexec"):
        if Path(candidate).exists():
            return candidate
    return shutil.which("xexec")


def test_rust_and_python_summaries_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    proc = subprocess.run(
        [binary, "summary", "--input", SAMPLE, "--bucket-ms", str(BUCKET_MS)],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    df = read_ticks(SAMPLE)
    py = summary(df, df["product"][0], BUCKET_MS * 1_000_000)

    assert rust["ticks"] == py["ticks"]
    assert rust["vwap"] == py["vwap"]
    assert rust["twap"] == py["twap"]
    assert rust["buy_volume"] == py["buy_volume"]
    assert rust["sell_volume"] == py["sell_volume"]
    assert rust["imbalance"] == py["imbalance"]
    assert rust["bars"] == py["bars"]


def test_rust_and_python_quote_metrics_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    proc = subprocess.run(
        [binary, "book", "--input", QUOTE_SAMPLE],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    df = read_quotes(QUOTE_SAMPLE)
    py = quote_metrics(df, df["product"][0])

    assert rust["quotes"] == py["quotes"]
    assert rust["avg_spread"] == py["avg_spread"]
    assert rust["avg_mid"] == py["avg_mid"]
    assert rust["avg_microprice"] == py["avg_microprice"]
    assert rust["avg_book_imbalance"] == py["avg_book_imbalance"]


def test_rust_and_python_depth_metrics_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    proc = subprocess.run(
        [binary, "depth", "--input", BOOK_SAMPLE],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    df = read_book(BOOK_SAMPLE)
    py = depth_metrics(df, df["product"][0])

    assert rust["snapshots"] == py["snapshots"]
    assert rust["avg_bid_depth"] == py["avg_bid_depth"]
    assert rust["avg_ask_depth"] == py["avg_ask_depth"]
    assert rust["avg_depth_imbalance"] == py["avg_depth_imbalance"]
    assert rust["avg_spread"] == py["avg_spread"]


def test_rust_and_python_queue_metrics_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    proc = subprocess.run(
        [binary, "queue", "--input", BOOK_SAMPLE],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    df = read_book(BOOK_SAMPLE)
    py = queue_metrics(df, df["product"][0])

    assert rust["snapshots"] == py["snapshots"]
    assert rust["avg_bid_queue"] == py["avg_bid_queue"]
    assert rust["avg_ask_queue"] == py["avg_ask_queue"]
    assert rust["avg_queue_imbalance"] == py["avg_queue_imbalance"]


def test_rust_and_python_sweep_costs_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    # The sweep is the first metric that depends on a *within-snapshot* order:
    # a cumulative sum walking the levels the taker meets, so a divergence in
    # sort order or in the running total would change the allocation and the
    # realised price. Run a size that eats past the touch in every snapshot,
    # so the walk is actually exercised rather than stopping at level 0.
    proc = subprocess.run(
        [binary, "sweep", "--input", BOOK_SAMPLE, "--side", "buy", "--size", str(SWEEP_SIZE)],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    df = read_book(BOOK_SAMPLE)
    py = sweep_cost(df, df["product"][0], "buy", SWEEP_SIZE)

    assert rust["snapshots"] == py["snapshots"]
    assert rust["filled_snapshots"] == py["filled_snapshots"]
    assert rust["avg_sweep_vwap"] == py["avg_sweep_vwap"]
    assert rust["avg_slippage_bps"] == py["avg_slippage_bps"]
    assert rust["avg_levels_consumed"] == py["avg_levels_consumed"]
    assert rust["avg_fill_ratio"] == py["avg_fill_ratio"]

    # And the sell side too: it sorts the book the other way, so agreeing on a
    # buy alone would leave half the walk unverified.
    sell = json.loads(
        subprocess.run(
            [binary, "sweep", "--input", BOOK_SAMPLE, "--side", "sell", "--size", str(SWEEP_SIZE)],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    )
    py_sell = sweep_cost(df, df["product"][0], "sell", SWEEP_SIZE)
    assert sell["avg_sweep_vwap"] == py_sell["avg_sweep_vwap"]
    assert sell["avg_slippage_bps"] == py_sell["avg_slippage_bps"]


def test_rust_and_python_sweep_curves_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    # The deepest test of the shared engine so far: a ladder of sweeps (each a
    # full book walk), a participation denominator from a separate aggregation,
    # a filter that drops the short fills, and a least-squares solve on top. A
    # divergence anywhere in that chain -- one differently-ordered sum, one
    # point admitted to or dropped from the fit -- moves the coefficient. The
    # ladder deliberately includes sizes the fixture book cannot fill, so the
    # exclusion rule itself is compared, not just the arithmetic.
    proc = subprocess.run(
        [
            binary,
            "curve",
            "--input",
            BOOK_SAMPLE,
            "--side",
            "buy",
            "--sizes",
            ",".join(str(q) for q in CURVE_SIZES),
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    df = read_book(BOOK_SAMPLE)
    py = sweep_curve(df, df["product"][0], "buy", CURVE_SIZES)

    assert rust["snapshots"] == py["snapshots"]
    assert rust["avg_depth"] == py["avg_depth"]
    assert rust["points"] == py["points"]
    assert rust["fitted_points"] == py["fitted_points"]
    assert rust["coef_bps"] == py["coef_bps"]
    assert rust["rmse_bps"] == py["rmse_bps"]
    assert rust["r_squared"] == py["r_squared"]
    assert rust["curve"] == py["curve"]

    # Some point in the ladder must have been excluded, or the exclusion rule
    # agreed only vacuously.
    assert rust["fitted_points"] < rust["points"]


def test_rust_and_python_schedules_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    # The first equivalence test over a *decision*, not a measurement. Both
    # engines price 41 candidate trajectories and pick one; agreeing means they
    # agree on every candidate's cost to 8dp and on the tie-break, because a
    # single candidate mispriced in the last place would hand back a different
    # urgency and a wholly different schedule.
    proc = subprocess.run(
        [
            binary,
            "schedule",
            "--slices",
            str(SCHEDULE["slices"]),
            "--total-size",
            str(SCHEDULE["total_size"]),
            "--slice-volume",
            str(SCHEDULE["per_slice_volume"]),
            "--coef-bps",
            str(SCHEDULE["coef_bps"]),
            "--perm-coef-bps",
            str(SCHEDULE["perm_coef_bps"]),
            "--sigma-bps",
            str(SCHEDULE["sigma_bps"]),
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    py = optimal_schedule("BTC-USD", **SCHEDULE)

    assert rust["urgency"] == py["urgency"]
    assert rust["impact_bps"] == py["impact_bps"]
    assert rust["risk_bps"] == py["risk_bps"]
    assert rust["total_bps"] == py["total_bps"]
    assert rust["twap_impact_bps"] == py["twap_impact_bps"]
    assert rust["twap_risk_bps"] == py["twap_risk_bps"]
    assert rust["twap_total_bps"] == py["twap_total_bps"]
    assert rust["saving_bps"] == py["saving_bps"]
    assert rust["schedule"] == py["schedule"]

    # The optimiser must have moved off the TWAP, or the two engines agreed
    # only on the trivial candidate and the search was never compared.
    assert rust["urgency"] > 0.0


def test_rust_and_python_shortfall_attributions_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    # The attribution is a difference of two numbers of similar size, so it is
    # the most fragile equivalence in the repo: a last-place disagreement in
    # either the realised or the modelled leg shows up whole in residual_bps.
    proc = subprocess.run(
        [
            binary,
            "shortfall",
            "--input",
            FILL_SAMPLE,
            "--parent-qty",
            str(SHORTFALL["parent_qty"]),
            "--arrival",
            str(SHORTFALL["arrival_price"]),
            "--coef-bps",
            str(SHORTFALL["coef_bps"]),
            "--perm-coef-bps",
            str(SHORTFALL["perm_coef_bps"]),
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    py = shortfall(read_fills(FILL_SAMPLE), "BTC-USD", **SHORTFALL)

    for field in (
        "side",
        "fills",
        "filled_qty",
        "unfilled_qty",
        "fill_rate",
        "avg_price",
        "final_price",
        "realised_bps",
        "modelled_bps",
        "residual_bps",
        "opportunity_bps",
        "total_bps",
    ):
        assert rust[field] == py[field], field
    assert rust["slices"] == py["slices"]

    # Both legs must be non-trivial, or the engines agreed only on zero.
    assert rust["modelled_bps"] != 0.0
    assert rust["opportunity_bps"] != 0.0


def test_rust_and_python_counterfactual_comparisons_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    # Three strategies priced and then ranked against each other. A last-place
    # disagreement in any one of them can flip best_alternative outright, so
    # this test pins the ordering as well as the numbers.
    proc = subprocess.run(
        [
            binary,
            "counterfactual",
            "--input",
            FILL_SAMPLE,
            "--arrival",
            str(SHORTFALL["arrival_price"]),
            "--coef-bps",
            str(SHORTFALL["coef_bps"]),
            "--perm-coef-bps",
            str(SHORTFALL["perm_coef_bps"]),
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    py = counterfactual(
        read_fills(FILL_SAMPLE),
        "BTC-USD",
        SHORTFALL["arrival_price"],
        SHORTFALL["coef_bps"],
        SHORTFALL["perm_coef_bps"],
    )

    for field in ("side", "intervals", "filled_qty", "best_alternative", "edge_bps"):
        assert rust[field] == py[field], field
    assert rust["realised"] == py["realised"]
    assert rust["alternatives"] == py["alternatives"]

    # The benchmarks must actually differ from the realised schedule, or the
    # engines agreed only that everything cost the same.
    assert rust["edge_bps"] != 0.0
    assert rust["realised"]["impact_bps"] != 0.0


def test_rust_and_python_sensitivity_sweeps_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    # The whole grid is compared point by point, not just the summary: a
    # disagreement at a single coefficient can flip verdict_stable, change where
    # the sign flips, and invent or erase a breakeven the other engine never saw.
    proc = subprocess.run(
        [
            binary,
            "sensitivity",
            "--input",
            FILL_SAMPLE,
            "--arrival",
            str(SHORTFALL["arrival_price"]),
            "--coef-grid",
            ",".join(str(c) for c in COEF_GRID),
            "--perm-coef-bps",
            str(SHORTFALL["perm_coef_bps"]),
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    py = sensitivity(
        read_fills(FILL_SAMPLE),
        "BTC-USD",
        SHORTFALL["arrival_price"],
        COEF_GRID,
        SHORTFALL["perm_coef_bps"],
    )

    for field in (
        "side",
        "intervals",
        "edge_min_bps",
        "edge_max_bps",
        "sign_flips",
        "verdict_stable",
        "breakeven_coef_bps",
    ):
        assert rust[field] == py[field], field
    assert rust["points"] == py["points"]

    # The sweep must have moved something, or the engines agreed only that the
    # coefficient makes no difference.
    assert len(rust["points"]) == len(COEF_GRID)
    assert rust["edge_min_bps"] != rust["edge_max_bps"]


def test_rust_and_python_volume_following_plans_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    # The fifteenth equivalence test, and the first over a plan derived from the
    # replay rather than from parameters alone: `schedule` chooses a trajectory
    # from numbers the caller supplies, whereas this allocation is read out of
    # the capture's own volume profile. So the engines must agree on the
    # bucketing before they can agree on anything else.
    proc = subprocess.run(
        [
            binary,
            "pov-plan",
            "--input",
            SAMPLE,
            "--bucket-ms",
            str(BUCKET_MS),
            "--parent-qty",
            str(POV_PLAN["parent_qty"]),
            "--cap",
            str(POV_PLAN["cap"]),
            "--coef-bps",
            str(POV_PLAN["coef_bps"]),
            "--perm-coef-bps",
            str(POV_PLAN["perm_coef_bps"]),
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    df = read_ticks(SAMPLE)
    py = pov_schedule(
        df,
        df["product"][0],
        BUCKET_MS * 1_000_000,
        POV_PLAN["parent_qty"],
        POV_PLAN["cap"],
        POV_PLAN["coef_bps"],
        POV_PLAN["perm_coef_bps"],
    )

    assert rust == py

    # The profile must have more than one bucket, or "follows the volume" is
    # vacuous -- a single bucket takes the whole parent whatever the rule.
    assert rust["buckets"] > 1

    # The two claims the module makes, checked against the Rust output rather
    # than only the Python one: constant participation, and exact VWAP tracking.
    assert {s["participation"] for s in rust["schedule"]} == {rust["participation"]}
    assert rust["pov_price"] == session_vwap(df)
    assert rust["pov_tracking_bps"] == 0.0

    # The benchmark is a real alternative here, not an infeasible strawman.
    assert rust["twap_feasible"]


def test_rust_and_python_out_of_sample_volume_plans_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    # The sixteenth equivalence test, and the first over two captures: the
    # engines must agree on both volume profiles, on how they line up, and on
    # the price of the difference between them.
    proc = subprocess.run(
        [
            binary,
            "pov-backtest",
            "--plan-input",
            SAMPLE,
            "--input",
            NEXT_SAMPLE,
            "--bucket-ms",
            str(BUCKET_MS),
            "--parent-qty",
            str(POV_PLAN["parent_qty"]),
            "--cap",
            str(POV_PLAN["cap"]),
            "--coef-bps",
            str(POV_PLAN["coef_bps"]),
            "--perm-coef-bps",
            str(POV_PLAN["perm_coef_bps"]),
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    py = pov_backtest(
        read_ticks(SAMPLE),
        read_ticks(NEXT_SAMPLE),
        BUCKET_MS * 1_000_000,
        POV_PLAN["parent_qty"],
        POV_PLAN["cap"],
        POV_PLAN["coef_bps"],
        POV_PLAN["perm_coef_bps"],
    )

    assert rust == py

    # The profiles genuinely differ, or the backtest is the in-sample plan again.
    assert rust["profile_distance"] > 0.0
    assert len({s["participation"] for s in rust["schedule"]}) > 1

    # The oracle is the cheapest allocation the cost model admits, so the price
    # of the forecast is never negative -- checked on the Rust output too.
    assert rust["forecast_cost_bps"] >= 0.0
    assert rust["impact_bps"] >= rust["oracle_impact_bps"]


def test_rust_and_python_pooled_volume_forecasts_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    # The seventeenth equivalence test, and the first over an estimate built
    # from several captures: the engines must agree on every session's profile,
    # on the order the shares are averaged in, and on two plans priced against
    # a third session -- a last-place disagreement in the mean moves both.
    proc = subprocess.run(
        [
            binary,
            "pov-forecast",
            "--history",
            f"{SAMPLE},{NEXT_SAMPLE}",
            "--input",
            THIRD_SAMPLE,
            "--bucket-ms",
            str(BUCKET_MS),
            "--parent-qty",
            str(POV_PLAN["parent_qty"]),
            "--cap",
            str(POV_PLAN["cap"]),
            "--coef-bps",
            str(POV_PLAN["coef_bps"]),
            "--perm-coef-bps",
            str(POV_PLAN["perm_coef_bps"]),
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    py = pov_forecast(
        [read_ticks(SAMPLE), read_ticks(NEXT_SAMPLE)],
        read_ticks(THIRD_SAMPLE),
        BUCKET_MS * 1_000_000,
        POV_PLAN["parent_qty"],
        POV_PLAN["cap"],
        POV_PLAN["coef_bps"],
        POV_PLAN["perm_coef_bps"],
    )

    assert rust == py

    # The pooled plan must differ from the naive one, or the engines agreed
    # only on a single session copied twice.
    assert rust["sessions"] == 2
    assert rust["forecast"]["impact_bps"] != rust["naive"]["impact_bps"]
    assert rust["forecast"]["forecast_cost_bps"] >= 0.0
    assert rust["naive"]["forecast_cost_bps"] >= 0.0


def test_rust_and_python_recency_weighted_forecasts_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    # The eighteenth equivalence test. The weights are the one quantity in the
    # engine neither Polars nor a plain sum produces: each is a `pow` of 0.5
    # called in the host language, then divided by a total the two engines must
    # accumulate in the same order. A half-life of 1.5 sessions gives both
    # weights an infinite binary expansion, so any disagreement shows up.
    half_life = "1.5"
    proc = subprocess.run(
        [
            binary,
            "pov-forecast",
            "--history",
            f"{SAMPLE},{NEXT_SAMPLE}",
            "--input",
            THIRD_SAMPLE,
            "--bucket-ms",
            str(BUCKET_MS),
            "--parent-qty",
            str(POV_PLAN["parent_qty"]),
            "--cap",
            str(POV_PLAN["cap"]),
            "--coef-bps",
            str(POV_PLAN["coef_bps"]),
            "--perm-coef-bps",
            str(POV_PLAN["perm_coef_bps"]),
            "--half-life",
            half_life,
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    py = pov_forecast(
        [read_ticks(SAMPLE), read_ticks(NEXT_SAMPLE)],
        read_ticks(THIRD_SAMPLE),
        BUCKET_MS * 1_000_000,
        POV_PLAN["parent_qty"],
        POV_PLAN["cap"],
        POV_PLAN["coef_bps"],
        POV_PLAN["perm_coef_bps"],
        float(half_life),
    )

    assert rust == py

    # The decay must actually have tilted the pool, or the test is the flat
    # case again under a different name.
    assert rust["half_life"] == 1.5
    assert rust["weights"][0] < rust["weights"][1]
    assert rust["weights"] != [0.5, 0.5]
    assert rust["forecast"]["forecast_cost_bps"] >= 0.0


def test_rust_and_python_streamed_sessions_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    # Both engines fold the same capture in the same chunks. The chunk count and
    # the residency bound are compared alongside the numbers: two engines that
    # agreed on the VWAP while one of them quietly read the whole file would not
    # be running the same algorithm, and this is the release that claims they do.
    proc = subprocess.run(
        [binary, "stream", "--input", SAMPLE, "--chunk-rows", str(STREAM_CHUNK_ROWS)],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    py = stream_session(SAMPLE, STREAM_CHUNK_ROWS)

    assert rust == py

    # The fold must have actually chunked: with one chunk this would only prove
    # the two engines agree on a single in-memory pass, which is the property
    # the other thirteen tests already cover.
    assert rust["chunks"] > 1
    assert rust["peak_rows_in_memory"] < rust["rows"]

    # ...and it must be the same session the in-memory pass reports.
    df = read_ticks(SAMPLE)
    assert rust["vwap"] == session_vwap(df)
    assert rust["twap"] == session_twap(df)


def test_rust_and_python_impact_curves_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    # Exercise both terms of the Almgren-Chriss model: a non-zero permanent
    # coefficient means the permanent and total-cost fields are non-trivial, so
    # the two engines must agree on the linear term and the round-trip sum too.
    proc = subprocess.run(
        [
            binary,
            "impact",
            "--input",
            IMPACT_SAMPLE,
            "--coef-bps",
            str(COEF_BPS),
            "--perm-coef-bps",
            str(PERM_COEF_BPS),
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    df = read_impact(IMPACT_SAMPLE)
    py = impact_curve(df, df["product"][0], COEF_BPS, PERM_COEF_BPS)

    assert rust["slices"] == py["slices"]
    assert rust["coef_bps"] == py["coef_bps"]
    assert rust["perm_coef_bps"] == py["perm_coef_bps"]
    assert rust["avg_impact_bps"] == py["avg_impact_bps"]
    assert rust["max_impact_bps"] == py["max_impact_bps"]
    assert rust["total_impact_bps"] == py["total_impact_bps"]
    assert rust["avg_perm_impact_bps"] == py["avg_perm_impact_bps"]
    assert rust["total_perm_impact_bps"] == py["total_perm_impact_bps"]
    assert rust["total_cost_bps"] == py["total_cost_bps"]


def test_rust_and_python_calibration_fits_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    # The fit is a Polars aggregation (sufficient statistics) plus a 2x2 solve.
    # Both engines must agree on the recovered coefficients *and* the fit-quality
    # diagnostics -- the scalar linear algebra has to be bit-identical too, not
    # just the sums.
    proc = subprocess.run(
        [binary, "calibrate", "--input", CALIBRATION_SAMPLE],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    df = read_calibration(CALIBRATION_SAMPLE)
    py = calibrate_impact(df, df["product"][0])

    assert rust["samples"] == py["samples"]
    assert rust["coef_bps"] == py["coef_bps"]
    assert rust["perm_coef_bps"] == py["perm_coef_bps"]
    assert rust["rmse_bps"] == py["rmse_bps"]
    assert rust["r_squared"] == py["r_squared"]


def test_rust_and_python_robust_calibration_fits_are_identical():
    binary = _find_binary()
    if not binary:
        pytest.skip("xexec Rust binary not built; run `cargo build --release`")

    # The robust fit is iteratively reweighted least squares: several weighted
    # Polars aggregations, each feeding a 2x2 solve whose coefficients drive the
    # next round of Huber weights. Every sum, every scalar step, and every
    # reweight has to be bit-identical across the two engines for the recovered
    # coefficients to match -- a much tighter test of the shared engine than the
    # single-pass OLS fit. Run it on the noisy replay (one gross outlier) so the
    # weights actually move.
    proc = subprocess.run(
        [
            binary,
            "calibrate",
            "--input",
            NOISY_CALIBRATION_SAMPLE,
            "--huber-delta",
            str(HUBER_DELTA),
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    rust = json.loads(proc.stdout)

    df = read_calibration(NOISY_CALIBRATION_SAMPLE)
    py = calibrate_impact_robust(df, df["product"][0], huber_delta=HUBER_DELTA)

    assert rust["samples"] == py["samples"]
    assert rust["coef_bps"] == py["coef_bps"]
    assert rust["perm_coef_bps"] == py["perm_coef_bps"]
    assert rust["rmse_bps"] == py["rmse_bps"]
    assert rust["r_squared"] == py["r_squared"]

    # And the robust fit must actually differ from the plain OLS fit on the same
    # replay -- otherwise the flag did nothing and the identity above is vacuous.
    ols = subprocess.run(
        [binary, "calibrate", "--input", NOISY_CALIBRATION_SAMPLE],
        capture_output=True,
        text=True,
        check=True,
    )
    assert json.loads(ols.stdout)["coef_bps"] != rust["coef_bps"]
