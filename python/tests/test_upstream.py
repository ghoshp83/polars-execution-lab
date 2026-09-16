"""The 2.0 watch must fire only on a stable 2.x, never on a pre-release."""

from xexeclab import upstream


def test_a_release_candidate_is_not_stable():
    # PyPI lists 2.0.0rc1 today; treating it as 2.0 would start the migration early.
    assert upstream.stable("2.0.0rc1") is None
    assert upstream.stable("2.0.0-beta.1") is None
    assert upstream.stable("1.44.2") == (1, 44, 2)


def test_newest_stable_compares_numerically_not_as_text():
    # As strings "0.9.0" > "0.55.2"; the crate line would look like it went backwards.
    assert upstream.newest_stable(["0.9.0", "0.55.2", "0.44.0"]) == "0.55.2"
    assert upstream.newest_stable(["2.0.0rc1"]) is None


def test_the_registries_as_they_stand_are_not_ready():
    assert not upstream.ready(["0.44.0", "0.55.2"])
    assert not upstream.ready(["1.44.2", "2.0.0rc1"])


def test_a_stable_two_is_ready():
    assert upstream.ready(["1.44.2", "2.0.0rc1", "2.0.0"])
    assert upstream.ready(["0.55.2", "2.1.0"])
