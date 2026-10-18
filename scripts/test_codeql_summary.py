#!/usr/bin/env python3
"""Tests of scripts/codeql_summary.py.

CodeQL itself cannot run in a unit test, so the SARIF here is written by the tests following the SARIF 2.1.0 layout
CodeQL emits (runs[].tool.driver.rules[] with `security-severity`, runs[].results[] with locations). This is the one
test double of this script; the real thing is exercised by .github/workflows/codeql.yml.

Run:  python3 scripts/test_codeql_summary.py
"""
import contextlib
import io
import json
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import codeql_summary as cs  # noqa: E402


def sarif(rules, results):
    return {"version": "2.1.0", "runs": [{"tool": {"driver": {"name": "CodeQL", "rules": rules}}, "results": results}]}


def result(rule, path="src/a.rs", line=3, level=None, text="msg", suppressed=False):
    r = {
        "ruleId": rule,
        "message": {"text": text},
        "locations": [{"physicalLocation": {"artifactLocation": {"uri": path}, "region": {"startLine": line}}}],
    }
    if level:
        r["level"] = level
    if suppressed:
        r["suppressions"] = [{"kind": "inSource"}]
    return r


def run(sarifs, *args):
    with tempfile.TemporaryDirectory() as d:
        for i, doc in enumerate(sarifs):
            (pathlib.Path(d) / f"r{i}.sarif").write_text(json.dumps(doc))
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            rc = cs.main([d, *args])
    return rc, out.getvalue()


RULES = [
    {"id": "rust/sql-injection", "properties": {"security-severity": "8.8"}},
    {"id": "rust/weak-hash", "properties": {"security-severity": "5.3"}},
    {"id": "js/unused", "defaultConfiguration": {"level": "note"}},
    {"id": "go/crit", "properties": {"security-severity": "9.8"}},
]


class Severity(unittest.TestCase):
    def test_scores_map_to_names(self):
        for score, name in (("9.8", "critical"), ("9.0", "critical"), ("7.0", "high"), ("6.9", "medium"), ("4.0", "medium"), ("3.9", "low"), ("0.1", "low")):
            self.assertEqual(cs.severity({"properties": {"security-severity": score}}, {}), name, score)

    def test_without_a_score_the_level_decides(self):
        self.assertEqual(cs.severity({}, {"level": "error"}), "high")
        self.assertEqual(cs.severity({}, {"level": "warning"}), "medium")
        self.assertEqual(cs.severity({}, {"level": "note"}), "low")
        self.assertEqual(cs.severity({"defaultConfiguration": {"level": "error"}}, {}), "high")


class Summary(unittest.TestCase):
    def test_counts_and_order(self):
        doc = sarif(RULES, [result("rust/weak-hash"), result("rust/sql-injection", line=9), result("js/unused"), result("go/crit")])
        rc, out = run([doc])
        self.assertEqual(rc, 0)
        self.assertIn("4 alert(s) in 1 SARIF file(s): 1 critical, 1 high, 1 medium, 1 low", out)
        lines = [l for l in out.splitlines() if l.startswith("  ")]
        self.assertEqual([l.split()[0] for l in lines], ["critical", "high", "medium", "low"])

    def test_suppressed_results_are_not_counted(self):
        rc, out = run([sarif(RULES, [result("rust/sql-injection", suppressed=True)])])
        self.assertIn("0 alert(s)", out)

    def test_several_files_are_added_up(self):
        rc, out = run([sarif(RULES, [result("go/crit")]), sarif(RULES, [result("rust/weak-hash")])])
        self.assertIn("2 alert(s) in 2 SARIF file(s)", out)


class Gate(unittest.TestCase):
    doc = sarif(RULES, [result("rust/sql-injection"), result("rust/weak-hash")])

    def test_fail_on_high_fails_on_a_high_alert(self):
        rc, out = run([self.doc], "--fail-on", "high")
        self.assertEqual(rc, 1)
        self.assertIn("::error", out)

    def test_fail_on_critical_passes_without_one(self):
        self.assertEqual(run([self.doc], "--fail-on", "critical")[0], 0)

    def test_fail_on_medium_counts_everything_above(self):
        self.assertEqual(run([sarif(RULES, [result("rust/weak-hash")])], "--fail-on", "medium")[0], 1)
        self.assertEqual(run([sarif(RULES, [result("js/unused")])], "--fail-on", "medium")[0], 0)

    def test_no_sarif_files_is_an_error_not_a_pass(self):
        with tempfile.TemporaryDirectory() as d, contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(cs.main([d]), 2)

    def test_an_unknown_severity_is_a_usage_error(self):
        self.assertEqual(run([self.doc], "--fail-on", "urgent")[0], 2)


if __name__ == "__main__":
    unittest.main()
