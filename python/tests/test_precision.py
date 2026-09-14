"""Where `_r8` stops resolving its 8th decimal.

`_r8` rounds to 8 *absolute* decimals, but a double's spacing grows with
magnitude. Below 2**26 the spacing is at most 2**-27 (~7.45e-9), finer than
1e-8, so distinct 8-decimal values stay distinct. Above it they can collapse
onto the same double. The README states this ceiling; these tests pin it.
"""

import math

from xexeclab.engine import _r8

CEILING = 2.0**26


def test_below_the_ceiling_every_8th_decimal_is_resolved():
    for x in (1.0, 65_000.0, 1e6, CEILING / 2, CEILING - 1.0):
        assert math.ulp(x) < 1e-8
        assert _r8(x + 1e-8) != _r8(x)
        assert _r8(-x - 1e-8) != _r8(-x)


def test_above_the_ceiling_an_8th_decimal_step_can_vanish():
    x = 2.0**27
    assert math.ulp(x) > 1e-8
    assert _r8(x + 1e-8) == _r8(x)


def test_at_session_notional_scale_the_resolution_is_about_1e_7():
    # A day's notional on a busy product is ~1e9; the 8dp claim is an overstatement there.
    assert 1e-7 < math.ulp(1e9) < 2e-7


def test_past_2_pow_53_over_1e8_rounding_is_the_identity():
    # x * 1e8 is already an integer, so `_r8` returns its input unchanged.
    x = 2.0**53 / 1e8 * 1.5
    assert _r8(x) == x
