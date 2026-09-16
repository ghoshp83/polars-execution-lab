"""Watch the two package registries for a stable Polars 2.0.

The project adopts Polars 2.0 only when both engines can move together: the
Python package on PyPI *and* the Rust crate on crates.io must each publish a
stable 2.x release. Adopting one side alone would leave the cross-language
equivalence tests comparing two engine generations.

    python -m xexeclab.upstream

Exits 0 while at least one side is still short of a stable 2.0, and 1 once both
have one -- so a scheduled CI run goes red on the day the migration can start.
"""

from __future__ import annotations

import json
import re
import sys
import urllib.request

TARGET_MAJOR = 2
CRATES_URL = "https://crates.io/api/v1/crates/polars/versions"
PYPI_URL = "https://pypi.org/pypi/polars/json"

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


def ready(versions: list[str]) -> bool:
    """True once a stable release at or above the target major exists."""
    newest = newest_stable(versions)
    return newest is not None and stable(newest)[0] >= TARGET_MAJOR


def _get(url: str) -> dict:
    # crates.io refuses requests without an identifying User-Agent.
    req = urllib.request.Request(url, headers={"User-Agent": "polars-execution-lab upstream watch"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.load(resp)


def main() -> int:
    crate = [v["num"] for v in _get(CRATES_URL)["versions"] if not v["yanked"]]
    pypi = list(_get(PYPI_URL)["releases"])
    rust_ready, python_ready = ready(crate), ready(pypi)
    print(f"rust   crate  newest stable {newest_stable(crate)}  2.0 ready: {rust_ready}")
    print(f"python package newest stable {newest_stable(pypi)}  2.0 ready: {python_ready}")
    if rust_ready and python_ready:
        print("Both engines have a stable Polars 2.0 -- start the README migration checklist.")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
