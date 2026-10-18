#!/usr/bin/env python3
"""Tests of scripts/security_exceptions.py.

The scanner reports in scripts/testdata/security/ are RECORDED output of the real tools (npm audit 10.9,
pip-audit 2.10, govulncheck v1.9.0 built with go1.27.2 and run against go1.24.7 and go1.27.2), trimmed to a few
entries, because the advisory databases change daily and a test must not depend on them. See the README there.
Everything else - the exceptions files - is written by the tests, in the format of security/exceptions.toml.

Run:  python3 -m unittest scripts/test_security_exceptions.py
"""
import contextlib
import datetime as dt
import io
import json
import os
import pathlib
import sys
import tempfile
import textwrap
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import security_exceptions as se  # noqa: E402

DATA = pathlib.Path(__file__).resolve().parent / "testdata" / "security"
TODAY = dt.date(2026, 10, 18)

REASON = "Prototype pollution in worker options only matters if an attacker controls them; vitest sets its own."


def entry(id="GHSA-5gmw-xhrv-c9v3", eco="npm", package="tinypool", added="2026-10-17", expires="2027-01-15", **kw):
    fields = {
        "id": id, "ecosystem": eco, "package": package, "scope": "dev dependency", "reason": REASON,
        "owner": "maintainers", "added": added, "expires": expires, **kw,
    }
    body = "\n".join(
        f'{k} = "{v}"' if k not in ("added", "expires") else f"{k} = {v}" for k, v in fields.items() if v is not None
    )
    return f"[[exception]]\n{body}\n"


class Env:
    """A temporary exceptions.toml and deny.toml wired into the module."""

    def __init__(self, exceptions="", deny=""):
        self.dir = tempfile.TemporaryDirectory()
        root = pathlib.Path(self.dir.name)
        (root / "exceptions.toml").write_text(exceptions)
        (root / "deny.toml").write_text(deny or "[advisories]\nignore = []\n")
        self.patches = [
            mock.patch.object(se, "EXCEPTIONS", root / "exceptions.toml"),
            mock.patch.object(se, "DENY", root / "deny.toml"),
        ]

    def __enter__(self):
        for p in self.patches:
            p.start()
        return self

    def __exit__(self, *a):
        for p in self.patches:
            p.stop()
        self.dir.cleanup()


def run(*argv, today=TODAY):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err), mock.patch.dict(os.environ, {"GITHUB_ACTIONS": ""}):
        rc = se.main(["--today", str(today), *map(str, argv)])
    return rc, out.getvalue()


class ParseAndValidate(unittest.TestCase):
    def test_the_repository_file_is_valid_on_the_day_it_was_written(self):
        # The real file, judged on the latest `added` date in it: structure, fields, 92-day limit. (It is not
        # judged against today's clock: expiry is the job of `check` in CI, not of a unit test.)
        text = (se.ROOT / "security" / "exceptions.toml").read_text()
        excs, problems = se.parse_exceptions(text)
        self.assertEqual(problems, [])
        self.assertTrue(excs, "the repository accepts at least the vitest findings")
        self.assertEqual(se.validate(excs, max(e.added for e in excs)), [])

    def test_a_missing_field_is_reported(self):
        _, problems = se.parse_exceptions(entry(owner=None))
        self.assertEqual(len(problems), 1)
        self.assertIn("missing field(s) owner", problems[0])

    def test_unknown_fields_and_ecosystems_are_reported(self):
        _, problems = se.parse_exceptions(entry(eco="ruby", severity="low"))
        self.assertTrue(any("ecosystem must be one of" in p for p in problems))
        self.assertTrue(any("unknown field(s) severity" in p for p in problems))

    def test_a_bad_date_is_reported(self):
        _, problems = se.parse_exceptions(entry(expires='"next spring"'))
        self.assertTrue(any("not a YYYY-MM-DD date" in p for p in problems))

    def test_an_id_that_is_not_an_advisory_is_reported(self):
        _, problems = se.parse_exceptions(entry(id="trust-me"))
        self.assertTrue(any("not a RUSTSEC/GHSA" in p for p in problems))

    def test_a_reason_that_says_nothing_is_reported(self):
        _, problems = se.parse_exceptions(entry(reason="fine"))
        self.assertTrue(any("too short to be a reason" in p for p in problems))

    def test_an_expired_entry_fails(self):
        excs, _ = se.parse_exceptions(entry(expires="2026-10-17"))
        problems = se.validate(excs, TODAY)
        self.assertEqual(len(problems), 1)
        self.assertIn("expired on 2026-10-17", problems[0])

    def test_the_expiry_day_itself_is_still_valid(self):
        excs, _ = se.parse_exceptions(entry(expires="2026-10-18"))
        self.assertEqual(se.validate(excs, TODAY), [])

    def test_more_than_92_days_fails(self):
        excs, _ = se.parse_exceptions(entry(added="2026-10-01", expires="2027-01-10"))  # 101 days
        problems = se.validate(excs, TODAY)
        self.assertEqual(len(problems), 1)
        self.assertIn("101 days", problems[0])

    def test_92_days_is_allowed(self):
        excs, _ = se.parse_exceptions(entry(added="2026-10-17", expires="2027-01-17"))
        self.assertEqual(se.validate(excs, TODAY), [])

    def test_an_entry_from_the_future_or_ending_before_it_starts_fails(self):
        excs, _ = se.parse_exceptions(entry(added="2026-12-01", expires="2026-12-31"))
        self.assertTrue(any("in the future" in p for p in se.validate(excs, TODAY)))
        excs, _ = se.parse_exceptions(entry(added="2026-10-17", expires="2026-10-16"))
        self.assertTrue(any("before it was added" in p for p in se.validate(excs, dt.date(2026, 10, 17))))

    def test_duplicates_fail(self):
        excs, _ = se.parse_exceptions(entry() + entry())
        self.assertTrue(any("listed twice" in p for p in se.validate(excs, TODAY)))

    def test_the_same_advisory_in_two_packages_is_not_a_duplicate(self):
        excs, _ = se.parse_exceptions(entry() + entry(package="vitest"))
        self.assertEqual(se.validate(excs, TODAY), [])


class DenyCrossCheck(unittest.TestCase):
    def excs(self, *ids):
        text = "".join(entry(id=i, eco="rust", package="time") for i in ids)
        return se.parse_exceptions(text)[0]

    def test_string_and_table_ignores_are_read(self):
        ignored = se.deny_ignores('[advisories]\nignore = ["RUSTSEC-2024-0001", { id = "RUSTSEC-2024-0002", reason = "x" }]\n')
        self.assertEqual(ignored, {"RUSTSEC-2024-0001", "RUSTSEC-2024-0002"})

    def test_an_ignore_without_an_exception_fails(self):
        problems = se.cross_check_deny(self.excs(), {"RUSTSEC-2024-0001"})
        self.assertEqual(len(problems), 1)
        self.assertIn("deny.toml ignores RUSTSEC-2024-0001", problems[0])

    def test_an_exception_deny_does_not_ignore_fails(self):
        problems = se.cross_check_deny(self.excs("RUSTSEC-2024-0001"), set())
        self.assertEqual(len(problems), 1)
        self.assertIn("deny.toml does not ignore it", problems[0])

    def test_matching_lists_pass(self):
        self.assertEqual(se.cross_check_deny(self.excs("RUSTSEC-2024-0001"), {"RUSTSEC-2024-0001"}), [])

    def test_other_ecosystems_do_not_need_a_deny_entry(self):
        excs = se.parse_exceptions(entry())[0]  # npm
        self.assertEqual(se.cross_check_deny(excs, set()), [])

    def test_the_repository_deny_toml_and_exceptions_agree(self):
        excs = se.parse_exceptions((se.ROOT / "security" / "exceptions.toml").read_text())[0]
        self.assertEqual(se.cross_check_deny(excs, se.deny_ignores((se.ROOT / "deny.toml").read_text())), [])


class CheckCommand(unittest.TestCase):
    def test_a_clean_file_passes(self):
        with Env(entry()):
            rc, out = run("check")
        self.assertEqual(rc, 0, out)

    def test_an_expired_file_fails(self):
        with Env(entry()):
            rc, out = run("check", today=dt.date(2027, 1, 16))
        self.assertEqual(rc, 1)
        self.assertIn("expired on 2027-01-15", out)

    def test_an_entry_about_to_expire_warns_but_passes(self):
        with Env(entry()):
            rc, out = run("check", today=dt.date(2027, 1, 10))
        self.assertEqual(rc, 0)
        self.assertIn("expires soon", out)

    def test_a_deny_mismatch_fails(self):
        with Env(entry(id="RUSTSEC-2024-0001", eco="rust", package="time"), deny="[advisories]\nignore = []\n"):
            rc, out = run("check")
        self.assertEqual(rc, 1)
        self.assertIn("deny.toml does not ignore it", out)

    def test_an_empty_file_is_fine(self):
        with Env(""):
            rc, _ = run("check")
        self.assertEqual(rc, 0)


class Npm(unittest.TestCase):
    """The recorded `npm audit --json` of sdk/typescript has two critical tinypool advisories, a moderate
    vitest one and a low esbuild one."""

    report = DATA / "npm-sdk-all.json"

    def test_unaccepted_critical_findings_fail(self):
        with Env(""):
            rc, out = run("npm", self.report)
        self.assertEqual(rc, 1)
        self.assertIn("GHSA-5gmw-xhrv-c9v3", out)
        self.assertIn("GHSA-85c8-ppgw-ccpr", out)
        self.assertIn("2 unaccepted", out)

    def test_accepted_findings_pass_and_lower_severities_do_not_count(self):
        both = entry() + entry(id="GHSA-85c8-ppgw-ccpr")
        with Env(both):
            rc, out = run("npm", self.report)
        self.assertEqual(rc, 0, out)
        self.assertIn("0 unaccepted, 2 accepted, 3 below the threshold", out)

    def test_an_expired_exception_fails_the_scan_again(self):
        both = entry() + entry(id="GHSA-85c8-ppgw-ccpr")
        with Env(both):
            rc, out = run("npm", self.report, today=dt.date(2027, 1, 16))
        self.assertEqual(rc, 1)

    def test_an_exception_for_another_package_does_not_cover_it(self):
        both = entry(package="vitest") + entry(id="GHSA-85c8-ppgw-ccpr", package="vitest")
        with Env(both):
            rc, _ = run("npm", self.report)
        self.assertEqual(rc, 1)

    def test_an_exception_for_another_ecosystem_does_not_cover_it(self):
        both = entry(eco="python") + entry(id="GHSA-85c8-ppgw-ccpr", eco="python")
        with Env(both):
            rc, _ = run("npm", self.report)
        self.assertEqual(rc, 1)

    def test_the_threshold_can_be_lowered(self):
        both = entry() + entry(id="GHSA-85c8-ppgw-ccpr")
        with Env(both):
            rc, out = run("npm", self.report, "--fail-on", "moderate")
        self.assertEqual(rc, 1)
        self.assertIn("GHSA-82fw-gwwq-j7x9", out)
        self.assertNotIn("error: npm audit: GHSA-g7r4-m6w7-qqqr", out)  # low stays below a moderate threshold

    def test_a_clean_production_report_passes(self):
        with Env(""):
            rc, out = run("npm", DATA / "npm-web-prod.json")
        self.assertEqual(rc, 0)
        self.assertIn("0 unaccepted", out)

    def test_an_exception_nothing_reports_is_called_out(self):
        with Env(entry(id="GHSA-aaaa-bbbb-cccc")):
            rc, out = run("npm", DATA / "npm-web-prod.json")
        self.assertEqual(rc, 0)
        self.assertIn("no longer needed", out)


class Pip(unittest.TestCase):
    def test_a_clean_report_passes(self):
        with Env(""):
            rc, out = run("pip", DATA / "pip-audit.json")
        self.assertEqual(rc, 0, out)

    def test_every_vulnerability_fails_because_pip_audit_has_no_severity(self):
        with Env(""):
            rc, out = run("pip", DATA / "pip-audit-vulnerable.json")
        self.assertEqual(rc, 1)
        self.assertIn("PYSEC-2019-133", out)
        self.assertIn("PYSEC-2019-132", out)

    def test_an_exception_matches_by_id(self):
        with Env(entry(id="PYSEC-2019-133", eco="python", package="urllib3")):
            rc, out = run("pip", DATA / "pip-audit-vulnerable.json")
        self.assertEqual(rc, 1)  # PYSEC-2019-132 is not covered
        self.assertEqual(out.count("accepted  PYSEC-2019-133"), 1)
        self.assertIn("error: pip-audit: PYSEC-2019-132", out)

    def test_an_exception_matches_by_alias(self):
        # the scanner reports PYSEC-2019-133; the entry names its CVE or GHSA alias
        for alias in ("CVE-2019-11324", "GHSA-mh33-7rrq-662w"):
            with Env(entry(id=alias, eco="python", package="urllib3")):
                rc, out = run("pip", DATA / "pip-audit-vulnerable.json")
            self.assertEqual(rc, 1, alias)
            self.assertIn("accepted  PYSEC-2019-133", out, alias)

    def test_both_accepted_passes(self):
        both = entry(id="PYSEC-2019-133", eco="python", package="urllib3") + entry(id="PYSEC-2019-132", eco="python", package="urllib3")
        with Env(both):
            rc, _ = run("pip", DATA / "pip-audit-vulnerable.json")
        self.assertEqual(rc, 0)


class Go(unittest.TestCase):
    def test_a_clean_run_passes(self):
        with Env(""):
            rc, out = run("go", DATA / "govulncheck-clean.json")
        self.assertEqual(rc, 0, out)

    def test_called_vulnerabilities_fail_and_imported_only_ones_do_not(self):
        with Env(""):
            rc, out = run("go", DATA / "govulncheck-vulnerable.json")
        self.assertEqual(rc, 1)
        self.assertIn("GO-2025-4007", out)
        self.assertIn("GO-2025-4008", out)
        self.assertNotIn("GO-2025-4006", out)  # imported, never called
        self.assertIn("1 more vulnerabilities affect a package we import but no function we call", out)

    def test_accepting_by_go_id_or_by_cve_alias(self):
        both = entry(id="GO-2025-4007", eco="go", package="stdlib") + entry(id="CVE-2025-58189", eco="go", package="stdlib")
        with Env(both):
            rc, out = run("go", DATA / "govulncheck-vulnerable.json")
        self.assertEqual(rc, 0, out)
        self.assertIn("0 unaccepted, 2 accepted", out)

    def test_the_toolchain_the_dockerfile_uses_is_what_makes_the_scan_clean(self):
        # The clean report was recorded with go1.27.2 (services/agent-orchestrator/Dockerfile); the vulnerable one
        # with go1.24.7 (what go.mod asked for), which has 36 reachable standard-library advisories.
        text = (DATA / "govulncheck-vulnerable.json").read_text()
        self.assertIn("go1.24.7", text)
        self.assertIn("go1.27.2", (DATA / "govulncheck-clean.json").read_text())

    def test_a_stream_is_parsed_as_a_stream(self):
        messages = list(se.go_messages('{"a":1}\n{"b":2}  {"c":3}\n'))
        self.assertEqual(messages, [{"a": 1}, {"b": 2}, {"c": 3}])


def sarif_dir(*results, language="rust"):
    """A directory with one SARIF file in the layout CodeQL writes (the one test double of this file: CodeQL cannot
    run in a unit test; the real thing runs in .github/workflows/codeql.yml). results: (rule, path, score)."""
    d = pathlib.Path(tempfile.mkdtemp())
    rules = [{"id": r, "properties": {"security-severity": str(score)}} for r, _, score in results]
    res = [
        {
            "ruleId": r,
            "message": {"text": f"{r} here"},
            "locations": [{"physicalLocation": {"artifactLocation": {"uri": path}, "region": {"startLine": 7}}}],
        }
        for r, path, _ in results
    ]
    (d / f"{language}.sarif").write_text(json.dumps({"version": "2.1.0", "runs": [{"tool": {"driver": {"rules": rules}}, "results": res}]}))
    return d


class CodeQl(unittest.TestCase):
    def accepted(self, rule="rust/cleartext-logging", path="crates/x/src/a.rs"):
        return entry(id=rule, eco="codeql", package=path)

    def test_no_alerts_passes(self):
        with Env(""):
            rc, out = run("codeql", sarif_dir())
        self.assertEqual(rc, 0, out)
        self.assertIn("0 unaccepted", out)

    def test_an_alert_that_is_not_accepted_fails(self):
        with Env(""):
            rc, out = run("codeql", sarif_dir(("rust/cleartext-logging", "crates/x/src/a.rs", 7.5)))
        self.assertEqual(rc, 1)
        self.assertIn("rust/cleartext-logging", out)

    def test_an_exception_covers_one_rule_in_one_file(self):
        alerts = sarif_dir(("rust/cleartext-logging", "crates/x/src/a.rs", 7.5))
        with Env(self.accepted()):
            self.assertEqual(run("codeql", alerts)[0], 0)
        with Env(self.accepted(path="crates/x/src/other.rs")):
            self.assertEqual(run("codeql", alerts)[0], 1)  # another file
        with Env(self.accepted(rule="rust/hard-coded-cryptographic-value")):
            self.assertEqual(run("codeql", alerts)[0], 1)  # another rule

    def test_an_expired_exception_fails_the_scan_again(self):
        alerts = sarif_dir(("rust/cleartext-logging", "crates/x/src/a.rs", 7.5))
        with Env(self.accepted()):
            self.assertEqual(run("codeql", alerts, today=dt.date(2027, 1, 16))[0], 1)

    def test_the_threshold_is_medium_by_default_and_can_be_raised(self):
        alerts = sarif_dir(("js/some-rule", "a.js", 5.0))
        with Env(""):
            self.assertEqual(run("codeql", alerts)[0], 1)
            self.assertEqual(run("codeql", alerts, "--fail-on", "high")[0], 0)

    def test_an_exception_is_only_called_stale_by_the_scan_of_its_own_language(self):
        # The languages are analysed by separate jobs: the JavaScript scan cannot say that a Python or Rust exception
        # is no longer needed, but it can say so about a JavaScript one.
        both = self.accepted(rule="py/clear-text-logging-sensitive-data", path="scripts/a.py") + self.accepted(
            rule="js/file-access-to-http", path="apps/web/a.ts"
        ) + self.accepted(rule="rust/cleartext-logging", path="crates/x/src/a.rs")
        with Env(both):
            rc, out = run("codeql", sarif_dir(language="javascript"))
        self.assertEqual(rc, 0, out)
        self.assertEqual(out.count("no longer needed"), 1, out)
        self.assertIn("js/file-access-to-http", out)
        with Env(both):
            rc, out = run("codeql", sarif_dir(language="python"))
            self.assertIn("py/clear-text-logging-sensitive-data", out)
            self.assertEqual(out.count("no longer needed"), 1, out)
            # a file named after a language the script does not know: every exception counts
            rc, out = run("codeql", sarif_dir(language="somethingnew"))
        self.assertEqual(out.count("no longer needed"), 3, out)

    def test_a_directory_without_sarif_is_an_error_not_a_pass(self):
        with Env(""):
            self.assertEqual(run("codeql", tempfile.mkdtemp())[0], 2)

    def test_rule_ids_are_valid_exception_ids(self):
        excs, problems = se.parse_exceptions(self.accepted(rule="js/server-side-unvalidated-url-redirection"))
        self.assertEqual(problems, [])
        _, problems = se.parse_exceptions(self.accepted(rule="not a rule"))
        self.assertTrue(any("not a RUSTSEC/GHSA" in p for p in problems))


class BrokenReports(unittest.TestCase):
    """A scanner that crashed must not be mistaken for a clean scan."""

    def run_with(self, kind, content):
        with Env(""), tempfile.TemporaryDirectory() as d:
            p = pathlib.Path(d) / "report.json"
            if content is not None:
                p.write_text(content)
            return run(kind, p)

    def test_a_missing_report_is_an_error(self):
        for kind in ("npm", "pip", "go"):
            rc, out = self.run_with(kind, None)
            self.assertEqual(rc, 2, kind)
            self.assertIn("cannot read the report", out)

    def test_an_empty_report_is_an_error(self):
        for kind in ("npm", "pip", "go"):
            rc, out = self.run_with(kind, "  \n")
            self.assertEqual(rc, 2, kind)
            self.assertIn("is empty", out)

    def test_garbage_is_an_error(self):
        for kind in ("npm", "pip", "go"):
            rc, _ = self.run_with(kind, "<html>502 Bad Gateway</html>")
            self.assertEqual(rc, 2, kind)

    def test_npm_reports_an_error_object(self):
        rc, out = self.run_with("npm", '{"error": {"code": "ENOTFOUND", "summary": "request to registry failed"}}')
        self.assertEqual(rc, 2)
        self.assertIn("ENOTFOUND", out)

    def test_a_govulncheck_stream_without_config_is_an_error(self):
        rc, out = self.run_with("go", '{"progress": {"message": "Scanning"}}')
        self.assertEqual(rc, 2)
        self.assertIn("no `config` message", out)

    def test_an_invalid_exceptions_file_fails_every_scan(self):
        with Env(entry(owner=None)):
            rc, out = run("pip", DATA / "pip-audit.json")
        self.assertEqual(rc, 1)
        self.assertIn("missing field(s) owner", out)


if __name__ == "__main__":
    unittest.main()
