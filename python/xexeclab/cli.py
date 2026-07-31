"""xexeclab command line: ingest, analytics, and execution-quality evaluation."""

from __future__ import annotations

import argparse
import asyncio
import json

from . import __version__
from .engine import bars, read_ticks, session_twap, session_vwap, summary, write_ticks
from .events import EventLog
from .fills import pov_fill, twap_fill


def cmd_ingest(a: argparse.Namespace) -> None:
    from .ingest import stream_coinbase

    log = EventLog(a.event_log)
    n = asyncio.run(stream_coinbase(a.product, a.out, a.max_trades, log))
    print(f"captured {n} trades -> {a.out}")


def cmd_synth(a: argparse.Namespace) -> None:
    from .ingest import synthetic_ticks

    n = synthetic_ticks(a.out, n=a.n, seed=a.seed, product=a.product)
    print(f"wrote {n} synthetic ticks -> {a.out}")


def cmd_convert(a: argparse.Namespace) -> None:
    n = write_ticks(read_ticks(a.input), a.out)
    print(f"converted {n} ticks {a.input} -> {a.out}")


def cmd_summary(a: argparse.Namespace) -> None:
    df = read_ticks(a.input)
    product = df["product"][0] if df.height else a.product
    print(json.dumps(summary(df, product, a.bucket_ms * 1_000_000)))


def cmd_bars(a: argparse.Namespace) -> None:
    df = read_ticks(a.input)
    for row in bars(df, a.bucket_ms * 1_000_000).iter_rows(named=True):
        print(json.dumps(row))


def cmd_vwap(a: argparse.Namespace) -> None:
    print(session_vwap(read_ticks(a.input)))


def cmd_twap(a: argparse.Namespace) -> None:
    print(session_twap(read_ticks(a.input)))


def _run_algo(a: argparse.Namespace):
    df = read_ticks(a.input)
    bucket_ns = a.bucket_ms * 1_000_000
    if a.algo == "pov":
        return df, pov_fill(
            df, side=a.side, parent_qty=a.qty, participation=a.participation, bucket_ns=bucket_ns
        )
    return df, twap_fill(df, side=a.side, parent_qty=a.qty, bucket_ns=bucket_ns)


def cmd_simulate(a: argparse.Namespace) -> None:
    _, result = _run_algo(a)
    print(json.dumps(result.__dict__))


def cmd_eval(a: argparse.Namespace) -> None:
    df = read_ticks(a.input)
    bucket_ns = a.bucket_ms * 1_000_000
    benchmark = session_vwap(df)
    pov = pov_fill(
        df, side=a.side, parent_qty=a.qty, participation=a.participation, bucket_ns=bucket_ns
    )
    twap = twap_fill(df, side=a.side, parent_qty=a.qty, bucket_ns=bucket_ns)

    def slip(px: float) -> float:
        raw = (px - benchmark) / benchmark * 1e4
        return round(raw if a.side == "buy" else -raw, 4)

    report = {
        "benchmark_vwap": benchmark,
        "side": a.side,
        "parent_qty": a.qty,
        "pov": {
            "avg_price": pov.avg_price,
            "slippage_vs_vwap_bps": slip(pov.avg_price),
            "is_bps": pov.is_bps,
            "fully_filled": pov.fully_filled,
        },
        "twap": {
            "avg_price": twap.avg_price,
            "slippage_vs_vwap_bps": slip(twap.avg_price),
            "is_bps": twap.is_bps,
        },
    }
    print(json.dumps(report, indent=2))


def _add_input(p: argparse.ArgumentParser) -> None:
    p.add_argument("--input", required=True, help="NDJSON replay file")
    p.add_argument("--bucket-ms", type=int, default=1000, help="bar width in milliseconds")
    p.add_argument("--product", default="BTC-USD")


def _add_algo(p: argparse.ArgumentParser) -> None:
    p.add_argument("--input", required=True)
    p.add_argument("--bucket-ms", type=int, default=1000)
    p.add_argument("--algo", choices=["pov", "twap"], default="pov")
    p.add_argument("--side", choices=["buy", "sell"], default="buy")
    p.add_argument("--qty", type=float, default=1.0, help="parent order quantity")
    p.add_argument("--participation", type=float, default=0.2, help="POV rate in (0, 1]")


def main(argv: list[str] | None = None) -> None:
    p = argparse.ArgumentParser(
        prog="xexeclab", description="Execution analytics over crypto tick data (Polars)."
    )
    p.add_argument("--version", action="version", version=f"xexeclab {__version__}")
    sub = p.add_subparsers(dest="cmd", required=True)

    pi = sub.add_parser("ingest", help="capture live Coinbase trades into an NDJSON replay")
    pi.add_argument("--product", default="BTC-USD")
    pi.add_argument("--out", required=True)
    pi.add_argument("--max-trades", type=int, default=500)
    pi.add_argument("--event-log", default=None)
    pi.set_defaults(fn=cmd_ingest)

    ps = sub.add_parser("synth", help="write a deterministic synthetic replay")
    ps.add_argument("--out", required=True)
    ps.add_argument("--n", type=int, default=200)
    ps.add_argument("--seed", type=int, default=7)
    ps.add_argument("--product", default="BTC-USD")
    ps.set_defaults(fn=cmd_synth)

    pc = sub.add_parser("convert", help="convert a replay between NDJSON and Parquet")
    pc.add_argument("--input", required=True, help="source replay (.ndjson/.jsonl/.parquet)")
    pc.add_argument("--out", required=True, help="destination (.ndjson/.jsonl/.parquet)")
    pc.set_defaults(fn=cmd_convert)

    for name, fn in (
        ("summary", cmd_summary),
        ("bars", cmd_bars),
        ("vwap", cmd_vwap),
        ("twap", cmd_twap),
    ):
        sp = sub.add_parser(name, help=f"compute {name}")
        _add_input(sp)
        sp.set_defaults(fn=fn)

    for name, fn in (("simulate", cmd_simulate), ("eval", cmd_eval)):
        sp = sub.add_parser(name, help=f"{name} an execution schedule")
        _add_algo(sp)
        sp.set_defaults(fn=fn)

    args = p.parse_args(argv)
    args.fn(args)


if __name__ == "__main__":
    main()
