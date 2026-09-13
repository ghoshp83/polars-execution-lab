"""Cross-version bit-identity check for the Polars engine.

The bundled fixtures are a few ticks long, which is too small for a query engine
to partition, so a suite that runs only those cannot see a change in how a
future Polars adds up a column. This module builds a capture large enough to
exercise that, fingerprints every engine result to the exact bit, and compares
two fingerprints taken under different Polars versions.

A known difference is recorded in a baseline rather than tolerated: the check
passes while the two versions differ exactly as recorded, and fails the moment
the set of differences changes in either direction.

    python -m xexeclab.compat dump OUT.json
    python -m xexeclab.compat compare A.json B.json BASELINE.json
"""

from __future__ import annotations

import argparse
import json
import sys

import polars as pl

from xexeclab import engine

PRODUCT = "BTC-USD"
ROWS = 400_000
BUCKET_NS = 5_000_000_000


def synthetic_ticks(rows: int = ROWS, seed: int = 7) -> pl.DataFrame:
    """A deterministic tick capture, identical on every platform and version.

    Drawn from a fixed linear congruential generator in plain Python rather than
    a library RNG, so the input itself can never be the thing that changed.
    """
    state = seed
    ts, price, size, side = [], [], [], []
    t, p = 1_700_000_000_000_000_000, 60_000.0
    for _ in range(rows):
        state = (state * 6364136223846793005 + 1442695040888963407) % 2**64
        u = state / 2**64
        t += 1_000_000 + int(u * 99_000_000)
        p += (u - 0.5) * 20.0
        ts.append(t)
        # Unrounded on purpose: full-precision values give every sum carry bits
        # to lose, so a change in summation order has something to show.
        price.append(p)
        size.append(0.001 + ((state >> 11) % 2_000_000_000) / 1_000_000_000 * 2.0)
        side.append("buy" if (state >> 33) & 1 else "sell")
    return pl.DataFrame(
        {
            "ts_ns": ts,
            "product": [PRODUCT] * rows,
            "price": price,
            "size": size,
            "side": side,
            "trade_id": list(range(rows)),
        }
    )


def _flatten(obj: object, path: str, out: dict[str, str]) -> None:
    if isinstance(obj, dict):
        for k, v in obj.items():
            _flatten(v, f"{path}.{k}" if path else k, out)
    elif isinstance(obj, list | tuple):
        for i, v in enumerate(obj):
            _flatten(v, f"{path}[{i}]", out)
    elif isinstance(obj, float):
        out[path] = obj.hex()
    else:
        out[path] = repr(obj)


def fingerprint(df: pl.DataFrame) -> dict[str, str]:
    """Every engine result over ``df``, keyed by path, floats as exact hex."""
    results = {
        "summary": engine.summary(df, PRODUCT, BUCKET_NS),
        "pov_schedule": engine.pov_schedule(df, PRODUCT, BUCKET_NS, 1_000.0, 0.1, 10.0),
    }
    out: dict[str, str] = {}
    _flatten(results, "", out)
    return out


def differences(a: dict[str, str], b: dict[str, str]) -> dict[str, list[str | None]]:
    """Every path whose value differs between two fingerprints, as ``[a, b]``."""
    return {k: [a.get(k), b.get(k)] for k in sorted(a.keys() | b.keys()) if a.get(k) != b.get(k)}


def check(a: dict[str, str], b: dict[str, str], baseline: dict) -> list[str]:
    """Problems with ``a`` vs ``b`` against the recorded ``baseline``; empty is a pass."""
    found = differences(a, b)
    problems = []
    for k in sorted(found.keys() | baseline.keys()):
        if k not in baseline:
            problems.append(f"new difference at {k}: {found[k][0]} vs {found[k][1]}")
        elif k not in found:
            problems.append(f"recorded difference at {k} is gone; update the baseline")
        elif found[k] != baseline[k]:
            problems.append(f"difference at {k} changed: {baseline[k]} -> {found[k]}")
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="xexeclab.compat", description=__doc__.split("\n")[0])
    sub = parser.add_subparsers(dest="cmd", required=True)
    d = sub.add_parser("dump", help="fingerprint the synthetic capture under this Polars")
    d.add_argument("out")
    c = sub.add_parser("compare", help="compare two fingerprints against a baseline")
    c.add_argument("a")
    c.add_argument("b")
    c.add_argument("baseline")
    args = parser.parse_args(argv)

    if args.cmd == "dump":
        with open(args.out, "w") as f:
            json.dump({"polars": pl.__version__, "values": fingerprint(synthetic_ticks())}, f)
        return 0

    with open(args.a) as f:
        a = json.load(f)
    with open(args.b) as f:
        b = json.load(f)
    with open(args.baseline) as f:
        baseline = json.load(f)
    problems = check(a["values"], b["values"], baseline["differences"])
    print(f"polars {a['polars']} vs {b['polars']}: {len(a['values'])} values compared")
    for p in problems:
        print(p)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
