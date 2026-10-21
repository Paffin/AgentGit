"""The one way a Python test of this repository says "the component I need is not here".

The twin of the Rust crate `crates/ag-testkit`, the Go package `services/agent-orchestrator/internal/testutil`
(components.go) and `apps/web/e2e/support/components.ts`; the rules are in `docs/architecture/test-skips.md`.

A test that needs an external component (a shell, MkDocs, a server binary, a model, ...) calls `require_component`
instead of returning early or calling `skipTest` itself:

* the component is there: the test runs;
* the component is missing and `AG_REQUIRE_COMPONENTS` names it (or is `all`): the test FAILS;
* otherwise the test SKIPS and leaves a machine-readable marker, which `scripts/check_skips.py` compares with the
  reviewed list `tests/skip-allowlist.toml`. The marker is the skip message (`AG-SKIP component=... test=...
  reason=...`) and, when `AG_SKIP_REPORT` names a file, one JSON line appended to it.

`unittest.SkipTest` is what both unittest and pytest turn into a skip. Standard library only, and it has to run on
Python 3.9 (the lightweight scripts are tested there, see `scripts/test_py39_compat.py`).
"""

from __future__ import annotations

import json
import os
import re
import sys
import unittest

ENV_REQUIRE = "AG_REQUIRE_COMPONENTS"
ENV_REPORT = "AG_SKIP_REPORT"
MARKER = "AG-SKIP"


def list_requires(spec: str, component: str) -> bool:
    """Does the list ("a,b" / "a b" / "all") require `component`? `oci` and `oci:docker` cover each other."""
    component = component.lower()
    for token in re.split(r"[,\s]+", spec.strip().lower()):
        if not token:
            continue
        if token in ("all", component) or component.startswith(token + ":") or token.startswith(component + ":"):
            return True
    return False


def required(component: str) -> bool:
    """Is the absence of `component` a failure in this process?"""
    return list_requires(os.environ.get(ENV_REQUIRE, ""), component)


def marker(component: str, test: str, reason: str) -> str:
    """The text of a skip: the same line in every language."""
    return f"{MARKER} component={component} test={test} reason={' '.join(reason.split())}"


def _record(component: str, test: str, reason: str) -> None:
    sys.stderr.write(marker(component, test, reason) + "\n")
    path = os.environ.get(ENV_REPORT)
    if not path:
        return
    line = json.dumps(
        {"marker": MARKER, "component": component, "test": test, "reason": " ".join(reason.split()), "lang": "python"}
    )
    try:
        with open(path, "a", encoding="utf-8") as f:
            f.write(line + "\n")
    except OSError:
        pass


def require_component(component: str, test: str, available: object, reason: str = "") -> None:
    """Returns when `available` is truthy; otherwise fails the test (the component is required) or skips it."""
    if available:
        return
    reason = reason or f"{component} is not available"
    if required(component):
        raise AssertionError(f"{component} is required ({ENV_REQUIRE}) but unavailable, so {test} cannot run: {reason}")
    _record(component, test, reason)
    raise unittest.SkipTest(marker(component, test, reason))


def not_applicable(kind: str, test: str, reason: str) -> None:
    """Skips a test that does not apply under the configuration it runs in. Never a failure; the marker
    (component `n/a:<kind>`) still reaches the report."""
    _record(f"n/a:{kind}", test, reason)
    raise unittest.SkipTest(marker(f"n/a:{kind}", test, reason))
