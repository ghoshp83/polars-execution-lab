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
    depth_metrics,
    quote_metrics,
    read_book,
    read_quotes,
    read_ticks,
    summary,
)

pytestmark = pytest.mark.equivalence

SAMPLE = "data/sample_ticks.ndjson"
QUOTE_SAMPLE = "data/sample_quotes.ndjson"
BOOK_SAMPLE = "data/sample_book.ndjson"
BUCKET_MS = 1000


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
