"""xexeclab command line: ingest, analytics, and execution-quality evaluation."""

from __future__ import annotations

import argparse
import asyncio
import json

from . import __version__
from .engine import (
    bars,
    calibrate_impact,
    calibrate_impact_robust,
    depth_metrics,
    impact_curve,
    quote_metrics,
    read_book,
    read_calibration,
    read_impact,
    read_quotes,
    read_ticks,
    session_twap,
    session_vwap,
    summary,
    write_ticks,
)
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


def cmd_ingest_quotes(a: argparse.Namespace) -> None:
    from .ingest import stream_coinbase_quotes

    log = EventLog(a.event_log)
    n = asyncio.run(stream_coinbase_quotes(a.product, a.out, a.max_quotes, log))
    print(f"captured {n} quotes -> {a.out}")


def cmd_synth_quotes(a: argparse.Namespace) -> None:
    from .ingest import synthetic_quotes

    n = synthetic_quotes(a.out, n=a.n, seed=a.seed, product=a.product)
    print(f"wrote {n} synthetic quotes -> {a.out}")


def cmd_ingest_book(a: argparse.Namespace) -> None:
    from .ingest import stream_coinbase_book

    log = EventLog(a.event_log)
    n = asyncio.run(stream_coinbase_book(a.product, a.out, a.max_snapshots, a.levels, log))
    print(f"captured {n} book snapshots -> {a.out}")


def cmd_synth_book(a: argparse.Namespace) -> None:
    from .ingest import synthetic_book

    n = synthetic_book(a.out, n_snapshots=a.n, levels=a.levels, seed=a.seed, product=a.product)
    print(f"wrote {n} synthetic book rows -> {a.out}")


def cmd_convert(a: argparse.Namespace) -> None:
    n = write_ticks(read_ticks(a.input), a.out)
    print(f"converted {n} ticks {a.input} -> {a.out}")


def cmd_summary(a: argparse.Namespace) -> None:
    df = read_ticks(a.input)
    product = df["product"][0] if df.height else a.product
    print(json.dumps(summary(df, product, a.bucket_ms * 1_000_000)))


def cmd_book(a: argparse.Namespace) -> None:
    df = read_quotes(a.input)
    product = df["product"][0] if df.height else a.product
    print(json.dumps(quote_metrics(df, product)))


def cmd_depth(a: argparse.Namespace) -> None:
    df = read_book(a.input)
    product = df["product"][0] if df.height else a.product
    print(json.dumps(depth_metrics(df, product)))


def cmd_impact(a: argparse.Namespace) -> None:
    df = read_impact(a.input)
    product = df["product"][0] if df.height else a.product
    print(json.dumps(impact_curve(df, product, a.coef_bps, a.perm_coef_bps)))


def cmd_calibrate(a: argparse.Namespace) -> None:
    df = read_calibration(a.input)
    product = df["product"][0] if df.height else a.product
    if a.huber_delta is not None or a.ridge_lambda != 0.0:
        fit = calibrate_impact_robust(
            df,
            product,
            huber_delta=a.huber_delta,
            ridge_lambda=a.ridge_lambda,
            max_iters=a.max_iters,
        )
    else:
        fit = calibrate_impact(df, product)
    print(json.dumps(fit))


def cmd_synth_calibration(a: argparse.Namespace) -> None:
    from .ingest import synthetic_calibration

    n = synthetic_calibration(
        a.out,
        n=a.n,
        coef_bps=a.coef_bps,
        perm_coef_bps=a.perm_coef_bps,
        noise_bps=a.noise_bps,
        outlier_frac=a.outlier_frac,
        outlier_bps=a.outlier_bps,
        seed=a.seed,
        product=a.product,
    )
    print(f"wrote {n} synthetic calibration samples -> {a.out}")


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
            df,
            side=a.side,
            parent_qty=a.qty,
            participation=a.participation,
            bucket_ns=bucket_ns,
            impact_bps=a.impact_bps,
            impact_model=a.impact_model,
            perm_impact_bps=a.perm_impact_bps,
        )
    return df, twap_fill(
        df,
        side=a.side,
        parent_qty=a.qty,
        bucket_ns=bucket_ns,
        impact_bps=a.impact_bps,
        impact_model=a.impact_model,
        perm_impact_bps=a.perm_impact_bps,
    )


def cmd_simulate(a: argparse.Namespace) -> None:
    _, result = _run_algo(a)
    print(json.dumps(result.__dict__))


def cmd_eval(a: argparse.Namespace) -> None:
    df = read_ticks(a.input)
    bucket_ns = a.bucket_ms * 1_000_000
    benchmark = session_vwap(df)
    pov = pov_fill(
        df,
        side=a.side,
        parent_qty=a.qty,
        participation=a.participation,
        bucket_ns=bucket_ns,
        impact_bps=a.impact_bps,
        impact_model=a.impact_model,
        perm_impact_bps=a.perm_impact_bps,
    )
    twap = twap_fill(
        df,
        side=a.side,
        parent_qty=a.qty,
        bucket_ns=bucket_ns,
        impact_bps=a.impact_bps,
        impact_model=a.impact_model,
        perm_impact_bps=a.perm_impact_bps,
    )

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
    p.add_argument(
        "--impact-bps",
        type=float,
        default=0.0,
        help="market impact in bps per unit of participation load (0 = pure VWAP)",
    )
    p.add_argument(
        "--impact-model",
        choices=["linear", "sqrt"],
        default="linear",
        help="temporary impact cost shape: linear in participation, or the concave sqrt law",
    )
    p.add_argument(
        "--perm-impact-bps",
        type=float,
        default=0.0,
        help="permanent impact in bps at full participation; drifts every later fill (0 = off)",
    )


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

    piq = sub.add_parser(
        "ingest-quotes", help="capture live Coinbase top-of-book quotes (auto-reconnect)"
    )
    piq.add_argument("--product", default="BTC-USD")
    piq.add_argument("--out", required=True)
    piq.add_argument("--max-quotes", type=int, default=500)
    piq.add_argument("--event-log", default=None)
    piq.set_defaults(fn=cmd_ingest_quotes)

    psq = sub.add_parser("synth-quotes", help="write a deterministic synthetic quote replay")
    psq.add_argument("--out", required=True)
    psq.add_argument("--n", type=int, default=200)
    psq.add_argument("--seed", type=int, default=7)
    psq.add_argument("--product", default="BTC-USD")
    psq.set_defaults(fn=cmd_synth_quotes)

    pc = sub.add_parser("convert", help="convert a replay between NDJSON and Parquet")
    pc.add_argument("--input", required=True, help="source replay (.ndjson/.jsonl/.parquet)")
    pc.add_argument("--out", required=True, help="destination (.ndjson/.jsonl/.parquet)")
    pc.set_defaults(fn=cmd_convert)

    pb = sub.add_parser("book", help="top-of-book microstructure metrics over a quote replay")
    pb.add_argument("--input", required=True, help="quote replay (.ndjson/.jsonl/.parquet)")
    pb.add_argument("--product", default="BTC-USD")
    pb.set_defaults(fn=cmd_book)

    pd = sub.add_parser("depth", help="L2 depth microstructure metrics over a book replay")
    pd.add_argument("--input", required=True, help="book replay (.ndjson/.jsonl/.parquet)")
    pd.add_argument("--product", default="BTC-USD")
    pd.set_defaults(fn=cmd_depth)

    pim = sub.add_parser(
        "impact", help="square-root market-impact cost curve over a participation schedule"
    )
    pim.add_argument("--input", required=True, help="impact replay (.ndjson/.jsonl/.parquet)")
    pim.add_argument(
        "--coef-bps", type=float, default=10.0, help="temporary impact in bps at full participation"
    )
    pim.add_argument(
        "--perm-coef-bps",
        type=float,
        default=0.0,
        help="permanent (linear) impact in bps at full participation (0 = temporary-only)",
    )
    pim.add_argument("--product", default="BTC-USD")
    pim.set_defaults(fn=cmd_impact)

    pcal = sub.add_parser("calibrate", help="fit impact coefficients from a realised-fill replay")
    pcal.add_argument("--input", required=True, help="calibration replay (.ndjson/.jsonl/.parquet)")
    pcal.add_argument("--product", default="BTC-USD")
    pcal.add_argument(
        "--huber-delta",
        type=float,
        default=None,
        help="robust IRLS: down-weight fills whose residual exceeds this many bps",
    )
    pcal.add_argument(
        "--ridge-lambda",
        type=float,
        default=0.0,
        help="ridge penalty added to the normal-matrix diagonal (0 = off)",
    )
    pcal.add_argument(
        "--max-iters", type=int, default=8, help="max IRLS passes when --huber-delta is set"
    )
    pcal.set_defaults(fn=cmd_calibrate)

    psc = sub.add_parser(
        "synth-calibration", help="write a deterministic synthetic realised-fill replay"
    )
    psc.add_argument("--out", required=True)
    psc.add_argument("--n", type=int, default=200, help="number of samples")
    psc.add_argument("--coef-bps", type=float, default=10.0, help="true temporary coefficient")
    psc.add_argument("--perm-coef-bps", type=float, default=20.0, help="true permanent coefficient")
    psc.add_argument(
        "--noise-bps", type=float, default=0.0, help="Gaussian noise stddev in bps (0 = exact fit)"
    )
    psc.add_argument(
        "--outlier-frac",
        type=float,
        default=0.0,
        help="fraction of samples corrupted by a shock (0 = none)",
    )
    psc.add_argument(
        "--outlier-bps", type=float, default=0.0, help="bps shock added to each corrupted sample"
    )
    psc.add_argument("--seed", type=int, default=7)
    psc.add_argument("--product", default="BTC-USD")
    psc.set_defaults(fn=cmd_synth_calibration)

    pib = sub.add_parser(
        "ingest-book", help="reconstruct the live Coinbase L2 book (auto-reconnect backfill)"
    )
    pib.add_argument("--product", default="BTC-USD")
    pib.add_argument("--out", required=True)
    pib.add_argument("--max-snapshots", type=int, default=500)
    pib.add_argument("--levels", type=int, default=10, help="top levels per side to record")
    pib.add_argument("--event-log", default=None)
    pib.set_defaults(fn=cmd_ingest_book)

    psb = sub.add_parser("synth-book", help="write a deterministic synthetic L2 book replay")
    psb.add_argument("--out", required=True)
    psb.add_argument("--n", type=int, default=100, help="number of snapshots")
    psb.add_argument("--levels", type=int, default=5, help="levels per side")
    psb.add_argument("--seed", type=int, default=7)
    psb.add_argument("--product", default="BTC-USD")
    psb.set_defaults(fn=cmd_synth_book)

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
