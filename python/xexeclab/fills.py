"""Execution-algorithm fill simulation and cost analytics.

These are the quant-research layer on top of the shared engine: given a parent
order, simulate how a POV or TWAP schedule would have filled against the
observed bars, and score the result against the arrival price (implementation
shortfall) and the session VWAP benchmark.

Honest scope: this is a *simulation* over historical bars -- it does not route
orders to a venue. Each child fills at its bar's VWAP, optionally adjusted by a
temporary market-impact model (`impact_bps`) that charges more the larger a
fraction of the bar's volume the child consumes. Two shapes are available: a
`linear` model (cost proportional to participation) and a `sqrt` model (the
concave square-root / Almgren-Chriss law, where the marginal cost of size
falls as participation grows -- the empirically observed shape). It measures
schedule quality, not live execution.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import polars as pl

from .engine import bars

IMPACT_MODELS = ("linear", "sqrt")


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


def _impact_price(
    vwap: float, qty: float, volume: float, side: str, impact_bps: float, model: str
) -> float:
    """Temporary-impact fill price under a `linear` or `sqrt` cost model.

    A child taking `qty` out of a bar of `volume` moves the price against itself
    by `impact_bps` basis points scaled by its participation load: a buy pays up,
    a sell receives less. The `linear` model scales cost by participation itself;
    the `sqrt` model scales by its square root (the concave Almgren-Chriss law),
    so a child that eats twice the volume pays only ~1.41x, not 2x. With
    `impact_bps == 0` the fill is exactly the bar VWAP, so the impact model is
    opt-in and the default preserves the pure-schedule benchmark.
    """
    if impact_bps == 0.0 or volume <= 0:
        return vwap
    participation = qty / volume
    load = participation if model == "linear" else math.sqrt(participation)
    signed = impact_bps if side == "buy" else -impact_bps
    return vwap * (1.0 + signed * 1e-4 * load)


def pov_fill(
    df: pl.DataFrame,
    *,
    side: str,
    parent_qty: float,
    participation: float,
    bucket_ns: int,
    impact_bps: float = 0.0,
    impact_model: str = "linear",
) -> FillResult:
    """Percentage-of-volume: take up to `participation` * bar_volume each bar,
    filling at that bar's VWAP (adjusted by `impact_bps` under the `impact_model`
    cost shape), until the parent order is exhausted."""
    if not 0 < participation <= 1:
        raise ValueError("participation must be in (0, 1]")
    if parent_qty <= 0:
        raise ValueError("parent_qty must be positive")
    if impact_model not in IMPACT_MODELS:
        raise ValueError(f"impact_model must be one of {IMPACT_MODELS}")

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
        price = _impact_price(row["vwap"], take, row["volume"], side, impact_bps, impact_model)
        notional += take * price
        filled += take
        remaining -= take
        schedule.append(
            {"bucket_ns": row["bucket_ns"], "qty": round(take, 8), "price": round(price, 8)}
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
    impact_bps: float = 0.0,
    impact_model: str = "linear",
) -> FillResult:
    """TWAP: an equal child quantity in every bar, filled at that bar's VWAP
    (adjusted by `impact_bps` under the `impact_model` cost shape)."""
    if parent_qty <= 0:
        raise ValueError("parent_qty must be positive")
    if impact_model not in IMPACT_MODELS:
        raise ValueError(f"impact_model must be one of {IMPACT_MODELS}")
    bar_rows = bars(df, bucket_ns).to_dicts()
    if not bar_rows:
        raise ValueError("no bars to schedule against")

    slice_qty = parent_qty / len(bar_rows)
    arrival = _arrival(df)
    notional = 0.0
    filled = 0.0
    schedule: list[dict] = []
    for row in bar_rows:
        price = _impact_price(row["vwap"], slice_qty, row["volume"], side, impact_bps, impact_model)
        notional += slice_qty * price
        filled += slice_qty
        schedule.append(
            {
                "bucket_ns": row["bucket_ns"],
                "qty": round(slice_qty, 8),
                "price": round(price, 8),
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
