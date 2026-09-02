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
    depth_metrics,
    impact_curve,
    optimal_schedule,
    queue_metrics,
    quote_metrics,
    read_book,
    read_calibration,
    read_impact,
    read_quotes,
    read_ticks,
    summary,
    sweep_cost,
    sweep_curve,
)

pytestmark = pytest.mark.equivalence

SAMPLE = "data/sample_ticks.ndjson"
QUOTE_SAMPLE = "data/sample_quotes.ndjson"
BOOK_SAMPLE = "data/sample_book.ndjson"
IMPACT_SAMPLE = "data/sample_impact.ndjson"
CALIBRATION_SAMPLE = "data/sample_calibration.ndjson"
NOISY_CALIBRATION_SAMPLE = "data/sample_calibration_noisy.ndjson"
HUBER_DELTA = 3.0
SWEEP_SIZE = 2.0
CURVE_SIZES = [0.5, 1.0, 2.0, 3.0, 10.0]
SCHEDULE = dict(
    slices=6, total_size=3.0, per_slice_volume=2.0, coef_bps=25.0, perm_coef_bps=5.0, sigma_bps=8.0
)
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
