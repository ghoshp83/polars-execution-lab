"""Watch that both engines are built from the same Polars crate line.

The Python package and the Rust crate are numbered independently. Every Python
release is built from the Rust workspace in the same repository, and that
workspace carries a crate version of its own: the `py-1.43.2` tag declares
0.54.4, and `py-1.44.2` and `py-2.0.0` both declare 0.55.1. So "Polars 2.0" is
a Python version number, and the crate line it is built from is the one to
match. This reads the newest stable Python release, looks up the crate version
its tag declares, and compares that line with the one `Cargo.toml` pins.

    python -m xexeclab.upstream

Exits 0 while the two lines match and 1 once the newest Python release is built
from a different crate line -- so a scheduled CI run goes red on the day the
Rust pin has fallen behind the Python engine.
"""

from __future__ import annotations

import json
import re
import sys
import tomllib
import urllib.request
from pathlib import Path

CRATES_URL = "https://crates.io/api/v1/crates/polars/versions"
PYPI_URL = "https://pypi.org/pypi/polars/json"
TAG_MANIFEST_URL = "https://raw.githubusercontent.com/pola-rs/polars/py-{version}/Cargo.toml"
OWN_MANIFEST = Path(__file__).resolve().parents[2] / "Cargo.toml"

_STABLE = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")


def stable(version: str) -> tuple[int, int, int] | None:
    """The version as a tuple if it is a plain release, else None.

    Release candidates, betas and dev builds (`2.0.0rc1`, `2.0.0-beta.1`) are not
    stable and return None, whatever their major number.
    """
    m = _STABLE.match(version)
    return (int(m[1]), int(m[2]), int(m[3])) if m else None


def newest_stable(versions: list[str]) -> str | None:
    """The highest plain release in `versions`, or None if there is none."""
    releases = [(v, stable(v)) for v in versions]
    releases = [(v, t) for v, t in releases if t is not None]
    return max(releases, key=lambda r: r[1])[0] if releases else None


def line(version: str) -> tuple[int, int]:
    """The crate line a version or a requirement belongs to: `0.55.2` and the
    requirement `0.55` are both the 0.55 line. Below 1.0 a minor bump is the
    breaking one, so the line is what two builds must share."""
    major, minor = version.split(".")[:2]
    return int(major), int(minor)


def built_from(tag_manifest: str) -> str:
    """The crate version a Python release tag declares for its Rust workspace."""
    return tomllib.loads(tag_manifest)["workspace"]["package"]["version"]


def pinned(own_manifest: str) -> str:
    """The `polars` requirement in this project's `Cargo.toml`."""
    return tomllib.loads(own_manifest)["dependencies"]["polars"]["version"]


def _fetch(url: str) -> str:
    # crates.io refuses requests without an identifying User-Agent.
    req = urllib.request.Request(url, headers={"User-Agent": "polars-execution-lab upstream watch"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        return resp.read().decode()


def main() -> int:
    crate = [v["num"] for v in json.loads(_fetch(CRATES_URL))["versions"] if not v["yanked"]]
    python = newest_stable(list(json.loads(_fetch(PYPI_URL))["releases"]))
    source = built_from(_fetch(TAG_MANIFEST_URL.format(version=python)))
    pin = pinned(OWN_MANIFEST.read_text())
    print(f"python package newest stable {python}, built from crate {source}")
    print(f"rust   crate   newest stable {newest_stable(crate)}, Cargo.toml pins {pin}")
    if line(source) != line(pin):
        print("The engines are on different crate lines -- move the Rust pin to match.")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
