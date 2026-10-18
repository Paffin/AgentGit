#!/usr/bin/env python3
"""Prints what a CodeQL analysis found, from the SARIF files the analysis wrote, and optionally fails on it.

  codeql_summary.py DIR [--fail-on high|critical|medium|low]

The alerts are also uploaded to Security -> Code scanning by the workflow; this is the same information in the job
log (so a run can be judged without the Security tab), and the place where a severity can fail the build. Without
--fail-on it always exits 0.

Severity is the rule's `security-severity` score when it has one (CodeQL's CVSS-like number: >= 9.0 critical,
>= 7.0 high, >= 4.0 medium, > 0 low), otherwise the SARIF level (error = high, warning = medium, note = low).
A result suppressed in the SARIF (`suppressions`) is not counted.
"""
from __future__ import annotations

import json
import pathlib
import sys

ORDER = ["low", "medium", "high", "critical"]


def severity(rule: dict, result: dict) -> str:
    score = (rule.get("properties") or {}).get("security-severity")
    if score is not None:
        s = float(score)
        return "critical" if s >= 9 else "high" if s >= 7 else "medium" if s >= 4 else "low"
    level = result.get("level") or (rule.get("defaultConfiguration") or {}).get("level") or "warning"
    return {"error": "high", "warning": "medium"}.get(level, "low")


def findings(sarif: dict) -> list[dict]:
    out = []
    for run in sarif.get("runs", []):
        driver = (run.get("tool") or {}).get("driver") or {}
        rules = {r["id"]: r for r in driver.get("rules", [])}
        for ext in (run.get("tool") or {}).get("extensions", []):
            rules.update({r["id"]: r for r in ext.get("rules", [])})
        for res in run.get("results", []):
            if res.get("suppressions"):
                continue
            rule = rules.get(res.get("ruleId"), {})
            loc = ((res.get("locations") or [{}])[0].get("physicalLocation")) or {}
            out.append(
                {
                    "rule": res.get("ruleId", "?"),
                    "severity": severity(rule, res),
                    "path": (loc.get("artifactLocation") or {}).get("uri", "?"),
                    "line": (loc.get("region") or {}).get("startLine", 0),
                    "message": (res.get("message") or {}).get("text", "").splitlines()[0][:160] if res.get("message") else "",
                }
            )
    return out


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__, file=sys.stderr)
        return 2
    directory = pathlib.Path(argv[0])
    fail_on = None
    if "--fail-on" in argv:
        fail_on = argv[argv.index("--fail-on") + 1]
        if fail_on not in ORDER:
            print(f"--fail-on must be one of {ORDER}", file=sys.stderr)
            return 2
    files = sorted(directory.glob("*.sarif"))
    if not files:
        print(f"no SARIF files in {directory}: the analysis did not write results", file=sys.stderr)
        return 2
    found: list[dict] = []
    for f in files:
        found += findings(json.loads(f.read_text()))
    found.sort(key=lambda r: (-ORDER.index(r["severity"]), r["rule"], r["path"], r["line"]))
    counts = {s: sum(1 for r in found if r["severity"] == s) for s in reversed(ORDER)}
    print(f"CodeQL: {len(found)} alert(s) in {len(files)} SARIF file(s): " + ", ".join(f"{n} {s}" for s, n in counts.items()))
    for r in found:
        print(f"  {r['severity']:8} {r['rule']:55} {r['path']}:{r['line']}  {r['message']}")
    if fail_on and any(ORDER.index(r["severity"]) >= ORDER.index(fail_on) for r in found):
        print(f"::error title=CodeQL::alerts of severity {fail_on} or above (listed above)")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
