#!/usr/bin/env python3
"""Tests for scripts/ag_testkit.py, the Python helper of the skip mechanism (docs/architecture/test-skips.md).

    python3 scripts/test_ag_testkit.py            # also run by .github/workflows/ci.yml (docs-and-demo), on Python 3.9 too
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.dont_write_bytecode = True
import ag_testkit as kit  # noqa: E402


class Env:
    """Sets environment variables for a block and puts them back."""

    def __init__(self, **kw: str | None):
        self.kw = kw
        self.old: dict[str, str | None] = {}

    def __enter__(self) -> "Env":
        for k, v in self.kw.items():
            self.old[k] = os.environ.get(k)
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        return self

    def __exit__(self, *exc: object) -> None:
        for k, v in self.old.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v


class Required(unittest.TestCase):
    def test_a_list_names_components(self):
        self.assertTrue(kit.list_requires("postgres,s3", "s3"))
        self.assertTrue(kit.list_requires(" postgres , s3 ", "postgres"))
        self.assertTrue(kit.list_requires("postgres s3", "s3"))
        self.assertFalse(kit.list_requires("postgres,s3", "keycloak"))
        self.assertFalse(kit.list_requires("", "postgres"))
        self.assertFalse(kit.list_requires("postgres", "post"))
        self.assertTrue(kit.list_requires("ALL", "anything"))
        self.assertTrue(kit.list_requires("Postgres", "POSTGRES"))

    def test_a_runtime_refines_the_generic_name(self):
        self.assertTrue(kit.list_requires("oci", "oci:docker"))
        self.assertTrue(kit.list_requires("oci:docker", "oci"))
        self.assertFalse(kit.list_requires("oci:docker", "oci:podman"))

    def test_nothing_is_required_by_default(self):
        with Env(AG_REQUIRE_COMPONENTS=None):
            self.assertFalse(kit.required("postgres"))
        with Env(AG_REQUIRE_COMPONENTS="dash"):
            self.assertTrue(kit.required("dash"))
            self.assertFalse(kit.required("mkdocs"))


class Skipping(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.report = Path(self.tmp.name) / "skips.jsonl"

    def call(self, fn, *args):
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            fn(*args)
        return err.getvalue()

    def test_an_available_component_does_nothing(self):
        with Env(AG_SKIP_REPORT=str(self.report), AG_REQUIRE_COMPONENTS="dash"):
            self.assertEqual(self.call(kit.require_component, "dash", "t", "/bin/dash", "x"), "")
        self.assertFalse(self.report.exists())

    def test_a_missing_component_skips_and_leaves_the_marker_in_the_report(self):
        with Env(AG_SKIP_REPORT=str(self.report), AG_REQUIRE_COMPONENTS=None):
            err = io.StringIO()
            with contextlib.redirect_stderr(err), self.assertRaises(unittest.SkipTest) as cm:
                kit.require_component("dash", "test_x", None, "dash is not\ninstalled")
        self.assertEqual(str(cm.exception), "AG-SKIP component=dash test=test_x reason=dash is not installed")
        self.assertIn("AG-SKIP component=dash test=test_x", err.getvalue())
        (line,) = self.report.read_text().splitlines()
        self.assertEqual(
            json.loads(line),
            {"marker": "AG-SKIP", "component": "dash", "test": "test_x", "reason": "dash is not installed", "lang": "python"},
        )

    def test_a_missing_component_that_is_required_fails(self):
        with Env(AG_SKIP_REPORT=str(self.report), AG_REQUIRE_COMPONENTS="dash"):
            with self.assertRaises(AssertionError) as cm:
                kit.require_component("dash", "test_x", False, "dash is not installed")
        self.assertIn("dash is required", str(cm.exception))
        self.assertFalse(self.report.exists(), "a failure is not a skip")

    def test_all_requires_everything(self):
        with Env(AG_REQUIRE_COMPONENTS="all"), self.assertRaises(AssertionError):
            kit.require_component("anything", "t", False, "r")

    def test_not_applicable_skips_even_when_everything_is_required(self):
        with Env(AG_SKIP_REPORT=str(self.report), AG_REQUIRE_COMPONENTS="all"):
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(unittest.SkipTest):
                kit.not_applicable("python-version", "t", "needs 3.11")
        self.assertEqual(json.loads(self.report.read_text())["component"], "n/a:python-version")

    def test_the_marker_is_one_line(self):
        self.assertEqual(kit.marker("s3", "t", "a\n b   c"), "AG-SKIP component=s3 test=t reason=a b c")


if __name__ == "__main__":
    unittest.main()
