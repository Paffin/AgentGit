#!/usr/bin/env python3
"""Applies security/exceptions.toml to the output of the vulnerability scanners.

The scanners (cargo-deny, `npm audit`, pip-audit, govulncheck) each have their own idea of an "ignore list" and
none of them expires an entry. This script is the one place where a finding is accepted, with an owner, a
reason and an expiry date (docs/operations/supply-chain.md):

  security_exceptions.py check            validate security/exceptions.toml and cross-check deny.toml
  security_exceptions.py npm  REPORT      `npm audit --json` (fails on high/critical not accepted)
  security_exceptions.py pip  REPORT      `pip-audit --format json` (fails on ANY vulnerability not accepted;
                                          pip-audit reports no severity)
  security_exceptions.py go   REPORT      `govulncheck -format json` (fails on any vulnerability whose vulnerable
                                          function is reachable from our code and not accepted)

Exit status: 0 clean, 1 findings or an invalid exceptions file, 2 the report could not be read (a scanner that
crashed must never look like a clean scan).

An exception only counts while it is valid: all fields present, `expires` not in the past and at most 92 days
after `added`. An expired entry therefore turns the finding it covered back into a failure, and `check` fails
on the entry itself. `deny.toml`'s `[advisories] ignore` must list exactly the `rust` exceptions.

`--today YYYY-MM-DD` (or SECURITY_EXCEPTIONS_TODAY) replaces the clock; the tests use it.
"""
from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import json
import os
import pathlib
import re
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
EXCEPTIONS = ROOT / "security" / "exceptions.toml"
DENY = ROOT / "deny.toml"

ECOSYSTEMS = ("rust", "npm", "python", "go")
MAX_LIFETIME_DAYS = 92
SOON_DAYS = 14
FIELDS = ("id", "ecosystem", "package", "scope", "reason", "owner", "added", "expires")
# npm audit severities, lowest to highest.
NPM_SEVERITIES = ("info", "low", "moderate", "high", "critical")
ID_PATTERN = re.compile(r"^(RUSTSEC-\d{4}-\d{4}|GHSA(-[0-9a-z]{4}){3}|PYSEC-\d{4}-\d+|GO-\d{4}-\d+|CVE-\d{4}-\d+)$")


class ReportError(Exception):
    """The scanner's output is missing or not what the scanner documents."""


@dataclasses.dataclass(frozen=True)
class Exc:
    id: str
    ecosystem: str
    package: str
    scope: str
    reason: str
    owner: str
    added: dt.date
    expires: dt.date

    def valid_on(self, today: dt.date) -> bool:
        return today <= self.expires


@dataclasses.dataclass(frozen=True)
class Finding:
    ecosystem: str
    package: str
    ids: tuple[str, ...]  # the advisory id first, then its aliases
    severity: str  # "" when the scanner has none
    title: str = ""

    @property
    def primary(self) -> str:
        return self.ids[0]


# --- exceptions file ---------------------------------------------------------------------------------------------


def parse_exceptions(text: str) -> tuple[list[Exc], list[str]]:
    """The entries of exceptions.toml and every problem found while reading them."""
    data = tomllib.loads(text)
    problems: list[str] = []
    unknown_tables = sorted(set(data) - {"exception"})
    if unknown_tables:
        problems.append(f"unknown top-level key(s) {unknown_tables}; only [[exception]] tables are read")
    out: list[Exc] = []
    for n, raw in enumerate(data.get("exception", []), 1):
        label = f"exception #{n} ({raw.get('id', '?')})"
        missing = [f for f in FIELDS if f not in raw]
        extra = sorted(set(raw) - set(FIELDS))
        if missing:
            problems.append(f"{label}: missing field(s) {', '.join(missing)}")
            continue
        if extra:
            problems.append(f"{label}: unknown field(s) {', '.join(extra)}")
        try:
            added = _date(raw["added"])
            expires = _date(raw["expires"])
        except ValueError as e:
            problems.append(f"{label}: {e}")
            continue
        if raw["ecosystem"] not in ECOSYSTEMS:
            problems.append(f"{label}: ecosystem must be one of {', '.join(ECOSYSTEMS)}")
        if not ID_PATTERN.match(str(raw["id"])):
            problems.append(f"{label}: id is not a RUSTSEC/GHSA/PYSEC/GO/CVE advisory id")
        for f in ("package", "scope", "reason", "owner"):
            if not str(raw[f]).strip():
                problems.append(f"{label}: {f} is empty")
        if len(str(raw["reason"]).strip()) < 40:
            problems.append(f"{label}: reason is too short to be a reason (say why it is acceptable and what changes that)")
        out.append(Exc(raw["id"], raw["ecosystem"], raw["package"], raw["scope"], raw["reason"], raw["owner"], added, expires))
    return out, problems


def _date(value: object) -> dt.date:
    if isinstance(value, dt.date) and not isinstance(value, dt.datetime):
        return value
    try:
        return dt.date.fromisoformat(str(value))
    except ValueError:
        raise ValueError(f"{value!r} is not a YYYY-MM-DD date") from None


def validate(excs: list[Exc], today: dt.date) -> list[str]:
    """Problems that make the file itself wrong: duplicates, expired, over-long."""
    problems: list[str] = []
    seen: set[tuple[str, str, str]] = set()
    for e in excs:
        key = (e.ecosystem, e.id, e.package)
        if key in seen:
            problems.append(f"{e.id} ({e.ecosystem}, {e.package}) is listed twice")
        seen.add(key)
        if e.expires < today:
            problems.append(
                f"{e.id} ({e.package}) expired on {e.expires}: fix the finding, or renew the entry with a new reason "
                f"and a new expiry (at most {MAX_LIFETIME_DAYS} days)"
            )
        if e.added > today:
            problems.append(f"{e.id} ({e.package}) is dated {e.added}, in the future")
        if e.expires < e.added:
            problems.append(f"{e.id} ({e.package}) expires ({e.expires}) before it was added ({e.added})")
        elif (e.expires - e.added).days > MAX_LIFETIME_DAYS:
            problems.append(
                f"{e.id} ({e.package}) lives {(e.expires - e.added).days} days; the limit is {MAX_LIFETIME_DAYS}. "
                "Renew it (new `added`, new reason) instead of extending it"
            )
    return problems


def deny_ignores(text: str) -> set[str]:
    """Advisory ids that deny.toml ignores. Entries are strings or `{ id = ..., reason = ... }` tables."""
    ignore = tomllib.loads(text).get("advisories", {}).get("ignore", [])
    return {i if isinstance(i, str) else i.get("id", "") for i in ignore}


def cross_check_deny(excs: list[Exc], ignored: set[str]) -> list[str]:
    problems = []
    rust = {e.id for e in excs if e.ecosystem == "rust"}
    for adv in sorted(ignored - rust):
        problems.append(f"deny.toml ignores {adv}, which is not listed (ecosystem = \"rust\") in security/exceptions.toml")
    for adv in sorted(rust - ignored):
        problems.append(f"security/exceptions.toml accepts {adv}, but deny.toml does not ignore it, so cargo-deny still fails")
    return problems


# --- matching ----------------------------------------------------------------------------------------------------


def accepted_by(finding: Finding, excs: list[Exc], today: dt.date) -> Exc | None:
    """The valid exception that covers `finding`, if any: same ecosystem, package and advisory id (or alias)."""
    for e in excs:
        if e.ecosystem != finding.ecosystem or not e.valid_on(today):
            continue
        if e.package != finding.package:
            continue
        if e.id in finding.ids:
            return e
    return None


def judge(findings: list[Finding], excs: list[Exc], today: dt.date, failing) -> tuple[list[Finding], list[tuple[Finding, Exc]]]:
    """Splits the findings that count (`failing(finding)`) into (unaccepted, accepted)."""
    bad, ok = [], []
    for f in findings:
        if not failing(f):
            continue
        e = accepted_by(f, excs, today)
        if e:
            ok.append((f, e))
        else:
            bad.append(f)
    return bad, ok


# --- scanner reports ---------------------------------------------------------------------------------------------


def _read(path: pathlib.Path) -> str:
    try:
        text = path.read_text()
    except OSError as e:
        raise ReportError(f"cannot read the report {path}: {e}") from e
    if not text.strip():
        raise ReportError(f"the report {path} is empty: the scanner did not run to completion")
    return text


def npm_findings(text: str) -> list[Finding]:
    """Advisories of an `npm audit --json` report (auditReportVersion 2)."""
    try:
        doc = json.loads(text)
    except json.JSONDecodeError as e:
        raise ReportError(f"npm audit output is not JSON: {e}") from e
    if "error" in doc:
        raise ReportError(f"npm audit failed: {json.dumps(doc['error'])[:300]}")
    if doc.get("auditReportVersion") != 2 or "vulnerabilities" not in doc:
        raise ReportError("npm audit output has no auditReportVersion 2 / vulnerabilities (npm >= 7 is required)")
    seen: dict[str, Finding] = {}
    for name, vuln in doc["vulnerabilities"].items():
        for via in vuln.get("via", []):
            if not isinstance(via, dict):
                continue  # a package that is vulnerable only through another; the advisory sits on that one
            m = re.search(r"(GHSA(?:-[0-9a-z]{4}){3})", via.get("url", ""))
            adv = m.group(1) if m else str(via.get("source", "unknown"))
            seen.setdefault(
                f"{via.get('name', name)}:{adv}",
                Finding("npm", via.get("name", name), (adv,), via.get("severity", "high"), via.get("title", "")),
            )
    return list(seen.values())


def pip_findings(text: str) -> list[Finding]:
    """Vulnerabilities of a `pip-audit --format json` report."""
    try:
        doc = json.loads(text)
    except json.JSONDecodeError as e:
        raise ReportError(f"pip-audit output is not JSON: {e}") from e
    if not isinstance(doc, dict) or "dependencies" not in doc:
        raise ReportError("pip-audit output has no `dependencies` list")
    out = []
    for dep in doc["dependencies"]:
        for v in dep.get("vulns", []):
            ids = (v["id"], *dict.fromkeys(a for a in v.get("aliases", []) if a != v["id"]))
            out.append(Finding("python", dep["name"], ids, "", (v.get("description") or "")[:120]))
    return out


def go_messages(text: str):
    """The concatenated JSON values of `govulncheck -format json` (a stream, not an array)."""
    dec = json.JSONDecoder()
    i, n = 0, len(text)
    while i < n:
        while i < n and text[i].isspace():
            i += 1
        if i >= n:
            break
        try:
            obj, i = dec.raw_decode(text, i)
        except json.JSONDecodeError as e:
            raise ReportError(f"govulncheck output is not a JSON stream: {e}") from e
        yield obj


def go_findings(text: str) -> tuple[list[Finding], int]:
    """(vulnerabilities that are called from our code, number of vulnerable modules only imported/required)."""
    osv: dict[str, dict] = {}
    called: dict[tuple[str, str], Finding] = {}
    not_called: set[str] = set()
    saw_config = False
    for msg in go_messages(text):
        if "config" in msg:
            saw_config = True
        if "osv" in msg:
            osv[msg["osv"]["id"]] = msg["osv"]
        if "finding" in msg:
            f = msg["finding"]
            trace = f.get("trace") or []
            if not trace:
                continue
            # Frame 0 is the vulnerable symbol. A `function` there means the symbol is reachable from our code;
            # without one the module is merely required or the package imported.
            if trace[0].get("function"):
                module = trace[0].get("module", "?")
                called.setdefault((f["osv"], module), Finding("go", module, (f["osv"],), "", ""))
            else:
                not_called.add(f["osv"])
    if not saw_config:
        raise ReportError("govulncheck output has no `config` message: the scanner did not run")
    out = []
    for (adv, module), f in sorted(called.items()):
        info = osv.get(adv, {})
        aliases = tuple(dict.fromkeys(a for a in info.get("aliases", []) if a != adv))
        out.append(Finding("go", module, (adv, *aliases), "", info.get("summary", "")))
    return out, len(not_called - {f.primary for f in out})


# --- commands ----------------------------------------------------------------------------------------------------


def say(level: str, title: str, msg: str) -> None:
    """A GitHub annotation in Actions, a plain line elsewhere."""
    if os.environ.get("GITHUB_ACTIONS") == "true":
        print(f"::{level} title={title}::{msg}")
    else:
        print(f"{level}: {title}: {msg}")


def load() -> tuple[list[Exc], list[str]]:
    return parse_exceptions(EXCEPTIONS.read_text())


def report(label: str, bad: list[Finding], ok: list[tuple[Finding, Exc]], noted: list[Finding]) -> int:
    for f, e in ok:
        print(f"accepted  {f.primary:24} {f.package} ({f.severity or 'no severity'}) until {e.expires} - {e.owner}")
    for f in noted:
        print(f"below threshold  {f.primary:24} {f.package} ({f.severity}) {f.title}")
    for f in bad:
        say("error", f"{label}: {f.primary}", f"{f.package} {f.severity} {f.title}".strip() + " - fix it or add a dated entry to security/exceptions.toml")
    print(f"{label}: {len(bad)} unaccepted, {len(ok)} accepted, {len(noted)} below the threshold")
    return 1 if bad else 0


def stale(excs: list[Exc], ecosystem: str, seen: set[str]) -> None:
    for e in excs:
        if e.ecosystem == ecosystem and e.id not in seen:
            say("notice", "exception no longer needed", f"{e.id} ({e.package}) is not reported by this scan; remove it from security/exceptions.toml")


def cmd_check(today: dt.date) -> int:
    excs, problems = load()
    problems += validate(excs, today)
    problems += cross_check_deny(excs, deny_ignores(DENY.read_text()))
    for e in excs:
        left = (e.expires - today).days
        if 0 <= left <= SOON_DAYS:
            say("warning", "exception expires soon", f"{e.id} ({e.package}) expires on {e.expires} ({left} days)")
    for p in problems:
        say("error", "security/exceptions.toml", p)
    print(f"{len(excs)} accepted exception(s); {len(problems)} problem(s)")
    return 1 if problems else 0


def _always(_: Finding) -> bool:
    return True


def cmd_scan(kind: str, report_path: pathlib.Path, today: dt.date, fail_on: str) -> int:
    excs, problems = load()
    if problems:
        for p in problems:
            say("error", "security/exceptions.toml", p)
        return 1
    text = _read(report_path)
    if kind == "npm":
        findings = npm_findings(text)
        floor = NPM_SEVERITIES.index(fail_on)

        def counts(f: Finding) -> bool:
            # a severity npm adds one day that we do not know is treated as the worst
            return NPM_SEVERITIES.index(f.severity) >= floor if f.severity in NPM_SEVERITIES else True

        label = "npm audit"
    elif kind == "pip":
        findings = pip_findings(text)
        counts = _always
        label = "pip-audit"
    else:
        findings, imported_only = go_findings(text)
        counts = _always
        label = "govulncheck"
        print(f"{label}: {imported_only} more vulnerabilities affect a package we import but no function we call")
    bad, ok = judge(findings, excs, today, counts)
    noted = [f for f in findings if not counts(f)]
    eco = {"npm": "npm", "pip": "python", "go": "go"}[kind]
    stale(excs, eco, {i for f in findings for i in f.ids})
    return report(label, bad, ok, noted)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--today", default=os.environ.get("SECURITY_EXCEPTIONS_TODAY"), help="YYYY-MM-DD (tests)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("check")
    npm = sub.add_parser("npm")
    npm.add_argument("report", type=pathlib.Path)
    npm.add_argument("--fail-on", choices=NPM_SEVERITIES, default="high")
    for name in ("pip", "go"):
        sub.add_parser(name).add_argument("report", type=pathlib.Path)
    args = ap.parse_args(argv)
    today = dt.date.fromisoformat(args.today) if args.today else dt.date.today()
    try:
        if args.cmd == "check":
            return cmd_check(today)
        return cmd_scan(args.cmd, args.report, today, getattr(args, "fail_on", "high"))
    except ReportError as e:
        say("error", "scanner report", str(e))
        return 2


if __name__ == "__main__":
    sys.exit(main())
