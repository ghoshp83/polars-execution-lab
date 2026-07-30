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
