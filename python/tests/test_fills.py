import pytest

from xexeclab.engine import bars, read_ticks
from xexeclab.fills import pov_fill, twap_fill

SAMPLE = "data/sample_ticks.ndjson"
BUCKET_1S = 1_000_000_000


def test_pov_respects_participation_cap_per_bar():
    df = read_ticks(SAMPLE)
    result = pov_fill(df, side="buy", parent_qty=100.0, participation=0.2, bucket_ns=BUCKET_1S)
    # A parent far larger than available liquidity cannot fully fill under a cap.
    assert not result.fully_filled
    vol = {r["bucket_ns"]: r["volume"] for r in bars(df, BUCKET_1S).iter_rows(named=True)}
    for child in result.schedule:
        assert child["qty"] <= 0.2 * vol[child["bucket_ns"]] + 1e-9


def test_pov_fills_small_order_fully():
    df = read_ticks(SAMPLE)
    result = pov_fill(df, side="buy", parent_qty=0.05, participation=0.5, bucket_ns=BUCKET_1S)
    assert result.fully_filled
    assert abs(result.filled_qty - 0.05) < 1e-9


def test_implementation_shortfall_sign_is_side_aware():
    df = read_ticks(SAMPLE)
    buy = pov_fill(df, side="buy", parent_qty=0.05, participation=0.5, bucket_ns=BUCKET_1S)
    sell = pov_fill(df, side="sell", parent_qty=0.05, participation=0.5, bucket_ns=BUCKET_1S)
    # Same fills, opposite side: shortfall convention flips sign.
    assert buy.is_bps == -sell.is_bps


def test_twap_fills_fully_with_equal_slices():
    df = read_ticks(SAMPLE)
    result = twap_fill(df, side="buy", parent_qty=3.0, bucket_ns=BUCKET_1S)
    assert result.fully_filled
    qtys = [c["qty"] for c in result.schedule]
    assert max(qtys) - min(qtys) < 1e-9


def test_participation_is_validated():
    df = read_ticks(SAMPLE)
    with pytest.raises(ValueError):
        pov_fill(df, side="buy", parent_qty=1.0, participation=0, bucket_ns=BUCKET_1S)


def test_market_impact_moves_the_fill_against_the_order():
    df = read_ticks(SAMPLE)
    base = pov_fill(df, side="buy", parent_qty=1.0, participation=0.5, bucket_ns=BUCKET_1S)
    impacted = pov_fill(
        df, side="buy", parent_qty=1.0, participation=0.5, bucket_ns=BUCKET_1S, impact_bps=50.0
    )
    # A buy pays up under impact: higher average price and worse shortfall.
    assert impacted.avg_price > base.avg_price
    assert impacted.is_bps > base.is_bps
    # A sell of the same schedule is filled worse in the opposite direction.
    sell = twap_fill(df, side="sell", parent_qty=1.0, bucket_ns=BUCKET_1S, impact_bps=50.0)
    sell_base = twap_fill(df, side="sell", parent_qty=1.0, bucket_ns=BUCKET_1S)
    assert sell.avg_price < sell_base.avg_price


def test_zero_impact_is_the_pure_vwap_benchmark():
    df = read_ticks(SAMPLE)
    a = twap_fill(df, side="buy", parent_qty=1.0, bucket_ns=BUCKET_1S)
    b = twap_fill(df, side="buy", parent_qty=1.0, bucket_ns=BUCKET_1S, impact_bps=0.0)
    assert a.avg_price == b.avg_price


def test_sqrt_model_charges_more_than_linear_at_low_participation():
    df = read_ticks(SAMPLE)
    one_bar = 10**18  # a bucket wide enough to hold the whole sample in one bar
    # A single sub-1 participation slice: sqrt(p) > p, so the concave law is
    # steeper than linear near zero and marks the buy up further.
    linear = twap_fill(
        df, side="buy", parent_qty=0.5, bucket_ns=one_bar, impact_bps=100.0, impact_model="linear"
    )
    sqrt = twap_fill(
        df, side="buy", parent_qty=0.5, bucket_ns=one_bar, impact_bps=100.0, impact_model="sqrt"
    )
    assert sqrt.avg_price > linear.avg_price > 0


def test_invalid_impact_model_is_rejected():
    df = read_ticks(SAMPLE)
    with pytest.raises(ValueError):
        twap_fill(df, side="buy", parent_qty=1.0, bucket_ns=BUCKET_1S, impact_model="cubic")
