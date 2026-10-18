#!/usr/bin/env python3
"""Tests of scripts/coverage_summary.py and scripts/mutants_summary.py.

scripts/testdata/coverage/llvm-cov-core.json is a real `cargo llvm-cov --json --summary-only` export (cargo-llvm-cov
0.9.1, llvm.coverage.json.export 3.1.0) of nine core crates run with their own test suites; only the paths were made
machine-independent. The cargo-mutants output directory in the mutants tests is written by the test in the layout the
tool documents (one mutant per line in caught.txt, missed.txt, timeout.txt, unviable.txt).

Run:  python3 scripts/test_coverage_summary.py
"""
import contextlib
import io
import json
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import coverage_summary as cov  # noqa: E402
import mutants_summary as mut  # noqa: E402

REPORT = pathlib.Path(__file__).resolve().parent / "testdata" / "coverage" / "llvm-cov-core.json"


def run(*args):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        sys.argv = ["coverage_summary.py", *map(str, args)]
        rc = cov.main()
    return rc, out.getvalue(), err.getvalue()


class PerCrate(unittest.TestCase):
    def setUp(self):
        self.crates = cov.per_crate(json.loads(REPORT.read_text()))

    def test_files_are_attributed_to_the_crate_they_live_in(self):
        self.assertEqual(
            sorted(self.crates),
            ["ag-changeset", "ag-crypto", "ag-dag", "ag-diff", "ag-merge", "ag-object", "ag-policy", "ag-tree", "ag-types"],
        )

    def test_counts_are_sums_over_the_crates_files(self):
        raw = json.loads(REPORT.read_text())["data"][0]["files"]
        want = sum(f["summary"]["lines"]["count"] for f in raw if "/crates/ag-tree/src/" in f["filename"])
        self.assertEqual(self.crates["ag-tree"]["lines"][1], want)
        covered, total = self.crates["ag-tree"]["lines"]
        self.assertTrue(0 < covered <= total)

    def test_tests_of_a_crate_are_not_counted(self):
        report = json.loads(REPORT.read_text())
        report["data"][0]["files"].append(
            {"filename": "/workspace/crates/ag-tree/tests/x.rs", "summary": {k: {"count": 1000, "covered": 0, "percent": 0} for k in ("lines", "functions", "regions")}}
        )
        self.assertEqual(cov.per_crate(report)["ag-tree"], self.crates["ag-tree"])

    def test_percentages(self):
        self.assertEqual(cov.pct((1, 4)), 25.0)
        self.assertEqual(cov.pct((0, 0)), 100.0)  # nothing to cover is not a failure

    def test_the_table_has_a_row_per_crate_and_a_total(self):
        text = cov.table(self.crates)
        self.assertEqual(text.count("\n"), len(self.crates) + 2)
        self.assertIn("| ag-merge |", text)
        self.assertIn("**all crates**", text)


class Floors(unittest.TestCase):
    def floors_file(self, body):
        d = tempfile.mkdtemp()
        p = pathlib.Path(d) / "floors.toml"
        p.write_text(body)
        return p

    def test_floors_that_hold_pass(self):
        rc, out, _ = run(REPORT, "--floors", self.floors_file("[lines]\nag-tree = 90\nag-types = 70\n"))
        self.assertEqual(rc, 0)
        self.assertIn("all 2 floors hold", out)

    def test_a_crate_below_its_floor_fails_and_is_named(self):
        rc, _, err = run(REPORT, "--floors", self.floors_file("[lines]\nag-types = 99\n"))
        self.assertEqual(rc, 1)
        self.assertIn("ag-types: line coverage", err)
        self.assertIn("below its floor of 99", err)

    def test_a_floor_for_a_crate_the_report_lacks_fails(self):
        rc, _, err = run(REPORT, "--floors", self.floors_file("[lines]\nag-nonexistent = 10\n"))
        self.assertEqual(rc, 1)
        self.assertIn("no source file", err)

    def test_a_report_without_crates_is_an_error_not_a_pass(self):
        d = pathlib.Path(tempfile.mkdtemp()) / "empty.json"
        d.write_text(json.dumps({"data": [{"files": []}]}))
        self.assertEqual(run(d)[0], 2)

    def test_the_markdown_table_is_appended_to_a_file(self):
        target = pathlib.Path(tempfile.mkdtemp()) / "summary.md"
        target.write_text("existing\n")
        rc, _, _ = run(REPORT, "--markdown", target)
        self.assertEqual(rc, 0)
        text = target.read_text()
        self.assertTrue(text.startswith("existing\n### Coverage by crate"))
        self.assertIn("| ag-diff |", text)

    def test_the_floors_of_the_repository_are_well_formed(self):
        # What the CI run enforces is checked there, against the whole workspace; here: the file names crates that
        # exist, in whole multiples of five, and every crate of the deterministic core has one.
        root = pathlib.Path(__file__).parent.parent
        floors = cov.tomllib.loads((root / "scripts" / "coverage_floors.toml").read_text())["lines"]
        crates = {p.parent.name for p in (root / "crates").glob("*/Cargo.toml")}
        self.assertEqual(sorted(set(floors) - crates), [], "a floor for a crate that does not exist")
        self.assertTrue(all(isinstance(v, int) and 0 < v <= 100 and v % 5 == 0 for v in floors.values()), floors)
        core = ["ag-types", "ag-crypto", "ag-object", "ag-changeset", "ag-dag", "ag-diff", "ag-tree", "ag-merge", "ag-policy"]
        self.assertEqual([c for c in core if c not in floors], [])

    def test_the_recorded_report_has_the_core_crates_with_their_figures(self):
        crates = self.crates()
        self.assertIn("ag-merge", crates)
        self.assertEqual(crates["ag-diff"]["lines"], (183, 189))
        self.assertEqual(round(cov.pct(crates["ag-diff"]["lines"]), 1), 96.8)

    def crates(self):
        return cov.per_crate(json.loads(REPORT.read_text()))


class Mutants(unittest.TestCase):
    def out_dir(self, **files):
        d = pathlib.Path(tempfile.mkdtemp())
        for name, lines in files.items():
            (d / f"{name}.txt").write_text("\n".join(lines) + ("\n" if lines else ""))
        return d

    def test_score_and_survivors(self):
        d = self.out_dir(
            caught=["a.rs:1: replace f -> u32 with 0", "a.rs:2: replace + with -", "b.rs:3: delete !"],
            missed=["b.rs:9: replace g -> bool with true"],
            timeout=["c.rs:5: replace < with >"],
            unviable=["c.rs:6: replace h -> String with String::new()"],
        )
        text = mut.summary(d, "ag-merge")
        self.assertIn("3 caught, 1 timed out", text)
        self.assertIn("**1 missed**", text)
        self.assertIn("1 did not compile", text)
        self.assertIn("Score 80.0% of 5 mutants that built", text)
        self.assertIn("b.rs:9: replace g -> bool with true", text)

    def test_a_clean_run_lists_no_survivors(self):
        text = mut.summary(self.out_dir(caught=["x"], missed=[]), "ag-tree")
        self.assertIn("**0 missed**", text)
        self.assertNotIn("Survivors", text)

    def test_unviable_mutants_do_not_count_in_the_score(self):
        text = mut.summary(self.out_dir(caught=["x"], unviable=["u1", "u2", "u3"]), "ag-tree")
        self.assertIn("Score 100.0% of 1 mutants", text)

    def test_a_missing_directory_is_an_error(self):
        sys.argv = ["mutants_summary.py", "/nonexistent-dir", "ag-tree"]
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(mut.main(), 2)


if __name__ == "__main__":
    unittest.main()
