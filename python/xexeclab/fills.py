"""Execution-algorithm fill simulation and cost analytics.

These are the quant-research layer on top of the shared engine: given a parent
order, simulate how a POV or TWAP schedule would have filled against the
observed bars, and score the result against the arrival price (implementation
shortfall) and the session VWAP benchmark.

Honest scope: this is a *simulation* over historical bars -- it does not route
orders to a venue and assumes each child fills at its bar's VWAP with no market
impact. It measures schedule quality, not live execution.
"""

from __future__ import annotations

from dataclasses import dataclass

import polars as pl

from .engine import bars


@dataclass(frozen=True)
class FillResult:
    side: str
    parent_qty: float
    filled_qty: float
    avg_price: float
    arrival_price: float
    is_bps: float  # implementation shortfall, basis points (positive = cost)
    fully_filled: bool
    schedule: list[dict]


def _shortfall_bps(avg_price: float, arrival_price: float, side: str) -> float:
    raw = (avg_price - arrival_price) / arrival_price * 1e4
    return round(raw if side == "buy" else -raw, 4)


def _arrival(df: pl.DataFrame) -> float:
    return float(df.sort("ts_ns")["price"][0])


def pov_fill(
    df: pl.DataFrame,
    *,
    side: str,
    parent_qty: float,
    participation: float,
    bucket_ns: int,
) -> FillResult:
    """Percentage-of-volume: take up to `participation` * bar_volume each bar,
    filling at that bar's VWAP, until the parent order is exhausted."""
    if not 0 < participation <= 1:
        raise ValueError("participation must be in (0, 1]")
    if parent_qty <= 0:
        raise ValueError("parent_qty must be positive")

    arrival = _arrival(df)
    remaining = parent_qty
    notional = 0.0
    filled = 0.0
    schedule: list[dict] = []
    for row in bars(df, bucket_ns).iter_rows(named=True):
        if remaining <= 0:
            break
        take = min(remaining, participation * row["volume"])
        if take <= 0:
            continue
        notional += take * row["vwap"]
        filled += take
        remaining -= take
        schedule.append(
            {"bucket_ns": row["bucket_ns"], "qty": round(take, 8), "price": round(row["vwap"], 8)}
        )

    avg_price = notional / filled if filled > 0 else 0.0
    return FillResult(
        side=side,
        parent_qty=parent_qty,
        filled_qty=round(filled, 8),
        avg_price=round(avg_price, 8),
        arrival_price=round(arrival, 8),
        is_bps=_shortfall_bps(avg_price, arrival, side),
        fully_filled=remaining <= 1e-12,
        schedule=schedule,
    )


def twap_fill(
    df: pl.DataFrame,
    *,
    side: str,
    parent_qty: float,
    bucket_ns: int,
) -> FillResult:
    """TWAP: an equal child quantity in every bar, filled at that bar's VWAP."""
    if parent_qty <= 0:
        raise ValueError("parent_qty must be positive")
    bar_rows = bars(df, bucket_ns).to_dicts()
    if not bar_rows:
        raise ValueError("no bars to schedule against")

    slice_qty = parent_qty / len(bar_rows)
    arrival = _arrival(df)
    notional = 0.0
    filled = 0.0
    schedule: list[dict] = []
    for row in bar_rows:
        notional += slice_qty * row["vwap"]
        filled += slice_qty
        schedule.append(
            {
                "bucket_ns": row["bucket_ns"],
                "qty": round(slice_qty, 8),
                "price": round(row["vwap"], 8),
            }
        )

    avg_price = notional / filled
    return FillResult(
        side=side,
        parent_qty=parent_qty,
        filled_qty=round(filled, 8),
        avg_price=round(avg_price, 8),
        arrival_price=round(arrival, 8),
        is_bps=_shortfall_bps(avg_price, arrival, side),
        fully_filled=True,
        schedule=schedule,
    )
