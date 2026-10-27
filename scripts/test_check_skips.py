#!/usr/bin/env python3
"""Tests for scripts/check_skips.py, the skip report and its gate.

    python3 scripts/test_check_skips.py            # also run by .github/workflows/ci.yml (docs-and-demo), on Python 3.9 too

The markers in these tests are written the way the real helpers write them (a JSON line per skip), the allow-lists
are small texts in the subset of TOML the script reads, and `main` is run end to end for the exit status.
"""

from __future__ import annotations

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))
sys.dont_write_bytecode = True
import check_skips as cs  # noqa: E402

ALLOW = '''
# a comment
[[skip]]
component = "postgres"
test = "the_*"
jobs = ["ci/test-*", "windows/windows"]   # trailing comment
reason = "The ordinary jobs have no PostgreSQL server."
owner = "storage"
runs_for_real = "ci.yml backends"

[[skip]]
component = "n/a:*"
reason = "Not applicable under this configuration (by design)."
owner = "sandbox"
runs_for_real = "none"

[[native]]
path = "crates/*/tests/scale.rs"
pattern = "ignore"
reason = "Measurements, not tests; they take minutes."
owner = "engine"
runs_for_real = "none"
'''


def marker(component: str, test: str, reason: str = "not here", lang: str = "rust") -> dict:
    return {"marker": "AG-SKIP", "component": component, "test": test, "reason": reason, "lang": lang}


def tables(text: str = ALLOW) -> dict:
    return cs.parse_allowlist(text)


def run_main(*args: str) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        try:
            rc = cs.main(list(args))
        except SystemExit as e:
            rc = e.code if isinstance(e.code, int) else 2
            if isinstance(e.code, str):
                err.write(e.code)
    return rc, out.getvalue(), err.getvalue()


class Parser(unittest.TestCase):
    def test_reads_tables_strings_and_arrays(self):
        t = tables()
        self.assertEqual(len(t["skip"]), 2)
        self.assertEqual(t["skip"][0]["jobs"], ["ci/test-*", "windows/windows"])
        self.assertEqual(t["skip"][0]["owner"], "storage")
        self.assertEqual(t["native"][0]["pattern"], "ignore")

    def test_an_array_may_span_lines_and_a_hash_inside_a_string_is_not_a_comment(self):
        t = cs.parse_allowlist('[[skip]]\ncomponent = "a"\njobs = [\n  "x/y",  # why\n  "z#1",\n]\nreason = "r # not a comment"\n')
        self.assertEqual(t["skip"][0]["jobs"], ["x/y", "z#1"])
        self.assertEqual(t["skip"][0]["reason"], "r # not a comment")

    def test_escapes(self):
        t = cs.parse_allowlist('[[skip]]\nreason = "say \\"hi\\"\\\\n"\n')
        self.assertEqual(t["skip"][0]["reason"], 'say "hi"\\n')

    def test_refuses_what_it_does_not_understand(self):
        for text in (
            "[[other]]\n",  # unknown table
            "[skip]\n",  # not an array of tables
            "component = 'x'\n",  # outside a table
            "[[skip]]\ncomponent = x\n",  # unquoted
            "[[skip]]\ncomponent = 3\n",
            '[[skip]]\ncomponent = "a"\ncomponent = "b"\n',  # twice
            '[[skip]]\njobs = [1, 2]\n',
            '[[skip]]\njobs = ["a"\n',  # not closed
            '[[skip]]\nreason = "a\\qb"\n',  # unknown escape
        ):
            with self.assertRaises(cs.AllowlistError, msg=text):
                cs.parse_allowlist(text)


class Validation(unittest.TestCase):
    def test_a_good_list_has_no_problems(self):
        self.assertEqual(cs.validate_allowlist(tables()), [])

    def test_reason_and_owner_are_mandatory(self):
        t = tables('[[skip]]\ncomponent = "postgres"\nruns_for_real = "ci"\n')
        problems = " ".join(cs.validate_allowlist(t))
        self.assertIn("reason is mandatory", problems)
        self.assertIn("owner is mandatory", problems)

    def test_a_reason_that_says_nothing_is_refused(self):
        t = tables('[[skip]]\ncomponent = "x"\nreason = "later"\nowner = "me"\nruns_for_real = "none"\n')
        self.assertIn("says nothing", " ".join(cs.validate_allowlist(t)))

    def test_unknown_fields_and_empty_jobs_are_refused(self):
        t = tables('[[skip]]\ncomponent = "x"\nreason = "a long enough reason"\nowner = "o"\nruns_for_real = "n"\nwho = "?"\njobs = []\n')
        problems = " ".join(cs.validate_allowlist(t))
        self.assertIn("unknown field 'who'", problems)
        self.assertIn("jobs must be", problems)

    def test_the_real_allowlist_is_valid(self):
        cs.load_allowlist(cs.ALLOWLIST)


class Judging(unittest.TestCase):
    def test_a_skip_on_the_list_for_this_job_is_allowed(self):
        allowed, bad = cs.judge([marker("postgres", "the_pool_reconnects")], "ci/test-ubuntu", tables())
        self.assertEqual((len(allowed), bad), (1, []))

    def test_the_same_skip_in_another_job_is_not(self):
        _, bad = cs.judge([marker("postgres", "the_pool_reconnects")], "ci/backends-moto", tables())
        self.assertEqual([s["test"] for s in bad], ["the_pool_reconnects"])

    def test_the_test_pattern_matters(self):
        _, bad = cs.judge([marker("postgres", "another_test")], "ci/test-ubuntu", tables())
        self.assertEqual(len(bad), 1)

    def test_a_component_pattern_covers_a_family_and_no_jobs_means_every_job(self):
        allowed, bad = cs.judge([marker("n/a:host-privilege", "t"), marker("n/a:s3-clock", "u")], "anything/at-all", tables())
        self.assertEqual((len(allowed), bad), (2, []))

    def test_an_unknown_component_is_not_allowed(self):
        _, bad = cs.judge([marker("keycloak", "t")], "ci/test-ubuntu", tables())
        self.assertEqual(len(bad), 1)

    def test_one_row_per_test_however_often_it_skipped(self):
        rows = cs.collapse([marker("postgres", "t"), marker("postgres", "t"), marker("postgres", "u")])
        self.assertEqual([r["test"] for r in rows], ["t", "u"])


class Reports(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)

    def write(self, name: str, text: str) -> Path:
        p = self.dir / name
        p.write_text(text)
        return p

    def allowlist(self, text: str = ALLOW) -> Path:
        return self.write("allow.toml", text)

    def test_a_missing_report_means_no_skips(self):
        self.assertEqual(cs.read_report(self.dir / "nope.jsonl"), [])

    def test_the_report_is_read_line_by_line(self):
        p = self.write("r.jsonl", json.dumps(marker("postgres", "t")) + "\n\n" + json.dumps(marker("s3", "u", lang="go")) + "\n")
        self.assertEqual([(s["component"], s["lang"]) for s in cs.read_report(p)], [("postgres", "rust"), ("s3", "go")])

    def test_a_line_that_is_not_a_marker_is_an_error(self):
        for text in ("not json\n", json.dumps({"component": "x"}) + "\n"):
            with self.assertRaises(SystemExit):
                cs.read_report(self.write("bad.jsonl", text))

    def test_markers_are_found_in_a_raw_log(self):
        p = self.write("log.txt", "running 3 tests\nAG-SKIP component=oci:docker test=a::b reason=the image is not there\nok\n")
        (s,) = cs.read_log(p)
        self.assertEqual((s["component"], s["test"], s["reason"]), ("oci:docker", "a::b", "the image is not there"))

    def test_main_passes_when_every_skip_is_reviewed(self):
        r = self.write("r.jsonl", json.dumps(marker("postgres", "the_x")) + "\n")
        summary = self.dir / "summary.md"
        rc, out, err = run_main("--job", "ci/test-ubuntu", "--report", str(r), "--allowlist", str(self.allowlist()), "--summary", str(summary))
        self.assertEqual((rc, err), (0, ""))
        self.assertIn("1 skipped, 0 not on the reviewed list", out)
        self.assertIn("`postgres`", summary.read_text())

    def test_main_fails_and_names_the_test_when_a_skip_is_not_reviewed(self):
        r = self.write("r.jsonl", json.dumps(marker("postgres", "the_x")) + "\n" + json.dumps(marker("keycloak", "kc", "no keycloak")) + "\n")
        rc, out, err = run_main("--job", "ci/test-ubuntu", "--report", str(r), "--allowlist", str(self.allowlist()), "--summary", str(self.dir / "s.md"))
        self.assertEqual(rc, 1)
        self.assertIn("kc skipped (keycloak): no keycloak", err)
        self.assertNotIn("the_x skipped", err)
        self.assertIn("**NO**", out)

    def test_main_with_nothing_skipped_says_so(self):
        rc, out, _ = run_main("--job", "ci/test-ubuntu", "--report", str(self.dir / "none.jsonl"), "--allowlist", str(self.allowlist()), "--summary", str(self.dir / "s.md"))
        self.assertEqual(rc, 0)
        self.assertIn("No test skipped", out)

    def test_main_refuses_an_invalid_allowlist(self):
        bad = self.allowlist('[[skip]]\ncomponent = "x"\n')
        rc, _, err = run_main("--validate", "--allowlist", str(bad))
        self.assertNotEqual(rc, 0)
        self.assertIn("reason is mandatory", err)

    def test_main_needs_something_to_do(self):
        rc, _, _ = run_main("--allowlist", str(self.allowlist()))
        self.assertEqual(rc, 2)

    def test_a_pipe_in_a_reason_does_not_break_the_table(self):
        text = cs.render("j", [], [marker("x", "t", "a | b")])
        self.assertIn("a \\| b", text)


class Scan(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def put(self, rel: str, text: str) -> None:
        p = self.root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)

    def sites(self):
        return [(rel, name) for rel, _, name, _ in cs.scan_tree(self.root)]

    def test_every_language_is_scanned(self):
        self.put("crates/a/tests/t.rs", '#[test]\n#[ignore = "slow"]\nfn t() {}\nfn u() { eprintln!("skipping: no git"); }\n')
        self.put("services/o/x_test.go", 'func TestX(t *testing.T) {\n\tt.Skip("no server")\n\tif testing.Short() {}\n}\n')
        self.put("apps/web/e2e/a.spec.ts", "test.skip(!x, 'no')\ntest.describe.skip('d', () => {})\nit.skipIf(x)('t', () => {})\nxit('t', () => {})\n")
        self.put("sdk/python/tests/test_a.py", "@pytest.mark.skipif(x, reason='y')\ndef test_a():\n    pytest.skip('z')\n    self.skipTest('w')\n")
        self.put("scripts/t.sh", "echo skipping the thing\n")
        found = self.sites()
        self.assertEqual(sorted({n for _, n in found}), ["ignore", "print-skip", "skip", "t.Skip", "testing.Short"])
        for rel in ("crates/a/tests/t.rs", "services/o/x_test.go", "apps/web/e2e/a.spec.ts", "sdk/python/tests/test_a.py", "scripts/t.sh"):
            self.assertIn(rel, [r for r, _ in found])

    def test_prose_and_product_code_are_not_skips(self):
        self.put("crates/a/src/lib.rs", '// we skip the #[ignore] tests\nfn f() { eprintln!("skipped {n} paths"); }\n')
        self.put("crates/a/tests/t.rs", '//! `#[ignore]`d in a plain run\n// t.Skip is not used\n')
        self.put("scripts/x.py", "# pytest.skip is banned here\n")
        self.assertEqual(self.sites(), [])

    def test_the_helpers_and_vendored_trees_are_exempt(self):
        self.put("scripts/ag_testkit.py", "raise unittest.SkipTest('x')\n")
        self.put("apps/web/node_modules/p/index.js", "test.skip()\n")
        self.put("apps/web/dist/a.js", "test.skip()\n")
        self.assertEqual(self.sites(), [])

    def test_a_native_entry_covers_its_files_and_nothing_else(self):
        self.put("crates/ag-tree/tests/scale.rs", '#[ignore = "measurement"]\n')
        self.put("crates/ag-tree/tests/other.rs", '#[ignore = "measurement"]\n')
        bad = cs.unreviewed_sites(cs.scan_tree(self.root), tables())
        self.assertEqual([b[0] for b in bad], ["crates/ag-tree/tests/other.rs"])

    def test_main_scan_status(self):
        allow = self.root / "allow.toml"
        allow.write_text(ALLOW)
        self.put("crates/ag-tree/tests/scale.rs", '#[ignore = "measurement"]\n')
        rc, _, _ = run_main("--scan", "--root", str(self.root), "--allowlist", str(allow))
        self.assertEqual(rc, 0)
        self.put("services/o/x_test.go", 't.Skip("x")\n')
        rc, _, err = run_main("--scan", "--root", str(self.root), "--allowlist", str(allow))
        self.assertEqual(rc, 1)
        self.assertIn("services/o/x_test.go:1", err)

    def test_the_repository_itself_is_clean(self):
        bad = cs.unreviewed_sites(cs.scan_tree(cs.ROOT), cs.load_allowlist(cs.ALLOWLIST))
        self.assertEqual(bad, [])


if __name__ == "__main__":
    unittest.main()
