"""The cross-version check must pass on a recorded difference and fail on any change."""

import json
from pathlib import Path

from xexeclab import compat

BASELINE = Path(__file__).with_name("polars2_baseline.json")


def test_the_committed_baseline_is_in_the_shape_compare_reads():
    baseline = json.loads(BASELINE.read_text())
    assert isinstance(baseline["differences"], dict)
    for pair in baseline["differences"].values():
        assert len(pair) == 2


def test_the_synthetic_capture_is_deterministic():
    assert compat.synthetic_ticks(1_000).equals(compat.synthetic_ticks(1_000))
    assert not compat.synthetic_ticks(1_000).equals(compat.synthetic_ticks(1_000, seed=8))


def test_a_fingerprint_keeps_every_bit_of_a_float():
    # 0.1 + 0.2 and 0.3 print the same at 8dp; the check must still tell them apart.
    a, b = {}, {}
    compat._flatten({"x": 0.1 + 0.2}, "", a)
    compat._flatten({"x": 0.3}, "", b)
    assert compat.differences(a, b) == {"x": [(0.1 + 0.2).hex(), (0.3).hex()]}


def test_the_fingerprint_covers_the_summary_and_the_schedule():
    fp = compat.fingerprint(compat.synthetic_ticks(20_000))
    assert "summary.buy_volume" in fp
    assert any(k.startswith("summary.bars[") for k in fp)
    assert any(k.startswith("pov_schedule.") for k in fp)


def test_a_recorded_difference_passes():
    baseline = {"v": ["0x1.0p+0", "0x1.0000000000001p+0"]}
    assert compat.check({"v": "0x1.0p+0"}, {"v": "0x1.0000000000001p+0"}, baseline) == []


def test_a_new_difference_fails():
    problems = compat.check({"v": "1", "w": "2"}, {"v": "1", "w": "3"}, {})
    assert problems == ["new difference at w: 2 vs 3"]


def test_a_changed_difference_fails():
    problems = compat.check({"v": "1"}, {"v": "4"}, {"v": ["1", "3"]})
    assert problems == ["difference at v changed: ['1', '3'] -> ['1', '4']"]


def test_a_difference_that_disappears_fails_so_the_baseline_stays_honest():
    problems = compat.check({"v": "1"}, {"v": "1"}, {"v": ["1", "3"]})
    assert problems == ["recorded difference at v is gone; update the baseline"]
