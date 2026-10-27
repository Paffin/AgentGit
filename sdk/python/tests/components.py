"""The one way a test of this package says that something it needs is not here.

The twin of `scripts/ag_testkit.py` (the repository's Python helper, which this package cannot import: it is
published on its own). The rules are in docs/architecture/test-skips.md of the repository: a missing component FAILS the
test when `AG_REQUIRE_COMPONENTS` names it (or is `all`), and otherwise SKIPS it, leaving an `AG-SKIP` marker in the
file named by `AG_SKIP_REPORT` that `scripts/check_skips.py` compares with the reviewed list `tests/skip-allowlist.toml`.
"""

from __future__ import annotations

import json
import os
import re
import sys

import pytest

MARKER = "AG-SKIP"


def list_requires(spec: str, component: str) -> bool:
    component = component.lower()
    for token in re.split(r"[,\s]+", spec.strip().lower()):
        if token and (
            token in ("all", component)
            or component.startswith(token + ":")
            or token.startswith(component + ":")
        ):
            return True
    return False


def marker(component: str, test: str, reason: str) -> str:
    return f"{MARKER} component={component} test={test} reason={' '.join(reason.split())}"


def require_component(component: str, test: str, available: object, reason: str) -> None:
    """Returns when `available` is truthy; otherwise fails the test (required) or skips it with the marker."""
    if available:
        return
    if list_requires(os.environ.get("AG_REQUIRE_COMPONENTS", ""), component):
        raise AssertionError(
            f"{component} is required (AG_REQUIRE_COMPONENTS) but unavailable, so {test} cannot run: {reason}"
        )
    _skip(component, test, reason)


def not_applicable(kind: str, test: str, reason: str) -> None:
    """Skips a test that does not apply under the configuration it runs in. Never a failure."""
    _skip(f"n/a:{kind}", test, reason)


def _skip(component: str, test: str, reason: str) -> None:
    sys.stderr.write(marker(component, test, reason) + "\n")
    path = os.environ.get("AG_SKIP_REPORT")
    if path:
        line = json.dumps(
            {
                "marker": MARKER,
                "component": component,
                "test": test,
                "reason": " ".join(reason.split()),
                "lang": "python",
            }
        )
        try:
            with open(path, "a", encoding="utf-8") as f:
                f.write(line + "\n")
        except OSError:
            pass
    pytest.skip(marker(component, test, reason))
