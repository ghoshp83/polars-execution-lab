"""The watch compares crate lines, and reads a pre-release as no release at all."""

from pathlib import Path

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


def test_a_requirement_and_a_patch_release_share_a_line():
    # `Cargo.toml` says "0.55" and resolves to 0.55.2; the Python 2.0.0 tag
    # declares 0.55.1. All three are one engine, and a patch must not go red.
    assert upstream.line("0.55") == upstream.line("0.55.1") == upstream.line("0.55.2")
    assert upstream.line("0.54.4") != upstream.line("0.55.1")
    assert upstream.line("0.9.0") < upstream.line("0.55.0")


def test_the_crate_version_is_read_from_the_workspace_table_of_a_tag():
    manifest = """
[workspace]
members = ["crates/*"]

[workspace.package]
version = "0.55.1"
edition = "2024"

[workspace.dependencies]
polars = { version = "0.99.0", path = "crates/polars" }
"""
    assert upstream.built_from(manifest) == "0.55.1"


def test_this_project_pins_the_line_python_two_is_built_from():
    # The fact the 2.x pin rests on: the `py-2.0.0` tag declares crate 0.55.1.
    # If the Rust pin moves, the Python pin has to be re-read against it.
    pin = upstream.pinned((Path(__file__).parents[2] / "Cargo.toml").read_text())
    assert upstream.line(pin) == upstream.line("0.55.1")
