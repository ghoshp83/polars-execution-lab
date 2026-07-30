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

from xexeclab.engine import read_ticks, summary

pytestmark = pytest.mark.equivalence

SAMPLE = "data/sample_ticks.ndjson"
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
    assert rust["bars"] == py["bars"]
