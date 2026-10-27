#!/usr/bin/env python3
"""The skip report of a CI test job, and the gate that fails it when a skip has not been reviewed.

A test that cannot reach an external component (PostgreSQL, S3, a container runtime, Keycloak, the server binary,
a model, ...) either fails (the job says it must have the component: `AG_REQUIRE_COMPONENTS`) or skips and leaves a
marker through the helper of its language (`crates/ag-testkit`, `scripts/ag_testkit.py`, the Go `testutil` package,
`apps/web/e2e/support/components.ts`). The markers are appended, one JSON object per line, to the file named by
`AG_SKIP_REPORT`; this script reads that file, prints what was skipped and why, and exits 1 when a skip is not covered
by `tests/skip-allowlist.toml` for this job (reason and owner are mandatory for every entry).

    AG_SKIP_REPORT=$RUNNER_TEMP/skips.jsonl cargo test --workspace
    python3 scripts/check_skips.py --job ci/test-ubuntu --report $RUNNER_TEMP/skips.jsonl

    python3 scripts/check_skips.py --scan        # the source tree: no skip construct outside the helpers, no
                                                 # #[ignore] that is not on the list ([[native]] entries)
    python3 scripts/check_skips.py --validate    # the allow-list itself is well formed

`--log FILE` reads the `AG-SKIP component=... test=... reason=...` lines from a raw log as well (the report file is
the reliable source: test harnesses swallow output). Python 3.9 compatible, standard library only (the allow-list is
read by the small TOML subset parser below, because `tomllib` is Python 3.11+).
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ALLOWLIST = ROOT / "tests" / "skip-allowlist.toml"
MARKER = "AG-SKIP"

SKIP_FIELDS = {"component", "test", "jobs", "reason", "owner", "runs_for_real"}
NATIVE_FIELDS = {"path", "pattern", "reason", "owner", "runs_for_real"}


# ---- the allow-list ---------------------------------------------------------------------------------------------


class AllowlistError(Exception):
    pass


def _unquote(raw: str, where: str) -> str:
    raw = raw.strip()
    if len(raw) < 2 or raw[0] != '"' or raw[-1] != '"':
        raise AllowlistError(f"{where}: a string must be in double quotes: {raw!r}")
    out, i, body = [], 0, raw[1:-1]
    while i < len(body):
        c = body[i]
        if c == "\\":
            i += 1
            if i >= len(body) or body[i] not in '"\\nt':
                raise AllowlistError(f"{where}: unsupported escape in {raw!r}")
            out.append({"n": "\n", "t": "\t"}.get(body[i], body[i]))
        elif c == '"':
            raise AllowlistError(f"{where}: an unescaped quote inside {raw!r}")
        else:
            out.append(c)
        i += 1
    return "".join(out)


def _strip_comment(line: str) -> str:
    in_str, esc = False, False
    for i, c in enumerate(line):
        if esc:
            esc = False
        elif c == "\\" and in_str:
            esc = True
        elif c == '"':
            in_str = not in_str
        elif c == "#" and not in_str:
            return line[:i]
    return line


def parse_allowlist(text: str) -> dict[str, list[dict]]:
    """The subset of TOML the allow-list uses: `[[skip]]` and `[[native]]` tables whose values are strings or
    arrays of strings (which may span lines). Anything else is an error."""
    tables: dict[str, list[dict]] = {"skip": [], "native": []}
    cur: dict | None = None
    pending_key: str | None = None
    pending: list[str] = []
    for no, raw in enumerate(text.splitlines(), 1):
        where = f"line {no}"
        line = _strip_comment(raw).strip()
        if pending_key is not None:
            pending.append(line)
            if line.endswith("]"):
                assert cur is not None
                cur[pending_key] = _array(" ".join(pending), where)
                pending_key, pending = None, []
            continue
        if not line:
            continue
        m = re.fullmatch(r"\[\[(\w+)\]\]", line)
        if m:
            if m.group(1) not in tables:
                raise AllowlistError(f"{where}: unknown table [[{m.group(1)}]] (expected [[skip]] or [[native]])")
            cur = {"_line": no}
            tables[m.group(1)].append(cur)
            continue
        m = re.fullmatch(r"(\w+)\s*=\s*(.*)", line)
        if not m or cur is None:
            raise AllowlistError(f"{where}: expected `[[skip]]`, `[[native]]` or `key = value`: {raw.strip()!r}")
        key, value = m.group(1), m.group(2).strip()
        if key in cur:
            raise AllowlistError(f"{where}: {key} is set twice")
        if value.startswith("["):
            if value.endswith("]"):
                cur[key] = _array(value, where)
            else:
                pending_key, pending = key, [value]
        else:
            cur[key] = _unquote(value, where)
    if pending_key is not None:
        raise AllowlistError(f"the array {pending_key} is not closed")
    return tables


def _array(raw: str, where: str) -> list[str]:
    inner = raw.strip()[1:-1].strip()
    if not inner:
        return []
    parts = re.findall(r'"(?:[^"\\]|\\.)*"', inner)
    rest = re.sub(r'"(?:[^"\\]|\\.)*"', "", inner).replace(",", "").strip()
    if rest:
        raise AllowlistError(f"{where}: an array holds only strings: {raw!r}")
    return [_unquote(p, where) for p in parts]


def validate_allowlist(tables: dict[str, list[dict]]) -> list[str]:
    """Every problem with the allow-list, as sentences. Reason and owner are mandatory."""
    problems: list[str] = []
    for kind, fields, required in (
        ("skip", SKIP_FIELDS, ("component", "reason", "owner", "runs_for_real")),
        ("native", NATIVE_FIELDS, ("path", "pattern", "reason", "owner", "runs_for_real")),
    ):
        for e in tables[kind]:
            where = f"[[{kind}]] at line {e['_line']}"
            for k in e:
                if k != "_line" and k not in fields:
                    problems.append(f"{where}: unknown field {k!r}")
            for k in required:
                v = e.get(k)
                if not isinstance(v, str) or not v.strip():
                    problems.append(f"{where}: {k} is mandatory")
            if isinstance(e.get("reason"), str) and 0 < len(e["reason"].strip()) < 15:
                problems.append(f"{where}: the reason says nothing ({e['reason']!r})")
            if "jobs" in e and (not isinstance(e["jobs"], list) or not e["jobs"]):
                problems.append(f"{where}: jobs must be a non-empty list of job-name patterns")
    return problems


# ---- the markers ------------------------------------------------------------------------------------------------

LOG_MARKER = re.compile(rf"{MARKER} component=(\S+) test=(.*?) reason=(.*)$")


def read_report(path: Path) -> list[dict]:
    out: list[dict] = []
    if not path.exists():
        return out
    for no, line in enumerate(path.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except ValueError:
            raise SystemExit(f"{path}:{no}: not a JSON line: {line[:80]!r}")
        if obj.get("marker") != MARKER or not obj.get("component"):
            raise SystemExit(f"{path}:{no}: not a skip marker: {line[:80]!r}")
        out.append({k: str(obj.get(k, "")) for k in ("component", "test", "reason", "lang")})
    return out


def read_log(path: Path) -> list[dict]:
    out: list[dict] = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        m = LOG_MARKER.search(line)
        if m:
            out.append({"component": m.group(1), "test": m.group(2), "reason": m.group(3).strip(), "lang": "log"})
    return out


def collapse(skips: list[dict]) -> list[dict]:
    """One row per (component, test): the same test skips once per process and per call, which is one skip."""
    seen: dict[tuple[str, str], dict] = {}
    for s in skips:
        seen.setdefault((s["component"], s["test"]), s)
    return sorted(seen.values(), key=lambda s: (s["component"], s["test"]))


def covering_entry(skip: dict, job: str, entries: list[dict]) -> dict | None:
    for e in entries:
        if not fnmatch.fnmatchcase(skip["component"], e["component"]):
            continue
        if not fnmatch.fnmatchcase(skip["test"], e.get("test", "*")):
            continue
        if not any(fnmatch.fnmatchcase(job, j) for j in e.get("jobs", ["*"])):
            continue
        return e
    return None


def judge(skips: list[dict], job: str, tables: dict[str, list[dict]]) -> tuple[list[tuple[dict, dict]], list[dict]]:
    allowed, unreviewed = [], []
    for s in collapse(skips):
        e = covering_entry(s, job, tables["skip"])
        if e is None:
            unreviewed.append(s)
        else:
            allowed.append((s, e))
    return allowed, unreviewed


def render(job: str, allowed: list[tuple[dict, dict]], unreviewed: list[dict]) -> str:
    lines = [f"### Skipped tests: {job}", ""]
    if not allowed and not unreviewed:
        lines += ["No test skipped.", ""]
        return "\n".join(lines)
    lines += [f"{len(allowed) + len(unreviewed)} skipped, {len(unreviewed)} not on the reviewed list.", ""]
    lines += ["| component | test | why | reviewed |", "|---|---|---|---|"]

    def cell(s: str) -> str:
        return s.replace("|", "\\|").replace("\n", " ")[:160]

    for s in unreviewed:
        lines.append(f"| `{s['component']}` | `{cell(s['test'])}` | {cell(s['reason'])} | **NO** |")
    for s, e in allowed:
        lines.append(f"| `{s['component']}` | `{cell(s['test'])}` | {cell(s['reason'])} | {cell(e['reason'])} ({cell(e['owner'])}) |")
    lines.append("")
    return "\n".join(lines)


# ---- the source scan --------------------------------------------------------------------------------------------

SCAN_DIRS = ["crates", "apps", "services", "sdk", "scripts", "tests", "examples", "fuzz"]
SKIP_DIRS = {"node_modules", "target", "dist", ".git", ".venv", ".venv-docs", "__pycache__", "playwright-report", "test-results"}
# The helpers themselves and the tests that talk about skip constructs.
SCAN_EXEMPT = {
    "crates/ag-testkit/src/lib.rs",
    "scripts/ag_testkit.py",
    "sdk/python/tests/components.py",
    "scripts/test_ag_testkit.py",
    "scripts/check_skips.py",
    "scripts/test_check_skips.py",
    "apps/web/e2e/support/components.ts",
    "services/agent-orchestrator/internal/testutil/components.go",
    "services/agent-orchestrator/internal/testutil/components_test.go",
}
# (suffixes, pattern name, regex): a construct that lets a test pass without running.
PATTERNS = [
    ((".rs",), "ignore", re.compile(r"#\[\s*ignore\b")),
    ((".rs",), "print-skip", re.compile(r"(?:e?print(?:ln)?|writeln)!\(\s*(?:std::io::stderr\(\)\s*,\s*)?\"\s*(?:skipp?(?:ed|ing)?\b|SKIP\b)", re.I)),
    ((".go",), "t.Skip", re.compile(r"\.\s*Skip(?:Now|f)?\(")),
    ((".go",), "testing.Short", re.compile(r"testing\.Short\(\)")),
    ((".ts", ".tsx", ".js", ".mjs", ".vue"), "skip", re.compile(r"\b(?:test|it|describe|suite)\s*\.\s*(?:skip|fixme|todo|skipIf)\b|\b(?:xit|xdescribe|xtest)\s*\(")),
    ((".py",), "skip", re.compile(r"\bpytest\s*\.\s*(?:skip|importorskip|xfail|mark\s*\.\s*(?:skip|skipif|xfail))\b|\bunittest\s*\.\s*skip\w*|\bskipTest\s*\(|\bSkipTest\b|@\s*skip(?:If|Unless)\b")),
    ((".sh",), "print-skip", re.compile(r"\becho\b.*\bskipp(?:ed|ing)\b", re.I)),
]


def scan_tree(root: Path) -> list[tuple[str, int, str, str]]:
    """(relative path, line, pattern name, the line) for every skip construct outside the exempt files."""
    found = []
    for top in SCAN_DIRS:
        base = root / top
        if not base.is_dir():
            continue
        for dirpath, dirnames, filenames in os.walk(base):
            dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
            for fn in filenames:
                suffix = os.path.splitext(fn)[1]
                path = Path(dirpath) / fn
                rel = path.relative_to(root).as_posix()
                if rel in SCAN_EXEMPT:
                    continue
                is_test_code = "/tests/" in rel or rel.endswith("tests.rs")  # product code may say "skipped"
                pats = [(n, rx) for sfx, n, rx in PATTERNS if suffix in sfx and (n != "print-skip" or suffix != ".rs" or is_test_code)]
                if not pats:
                    continue
                try:
                    text = path.read_text(encoding="utf-8")
                except (OSError, UnicodeDecodeError):
                    continue
                for no, line in enumerate(text.splitlines(), 1):
                    stripped = line.strip()
                    if stripped.startswith(("//", "#", "*", "/*")) and not stripped.startswith("#["):
                        continue  # prose about skipping, not a skip
                    for name, rx in pats:
                        if rx.search(line):
                            found.append((rel, no, name, stripped[:140]))
    return found


def unreviewed_sites(sites: list[tuple[str, int, str, str]], tables: dict[str, list[dict]]) -> list[tuple[str, int, str, str]]:
    bad = []
    for rel, no, name, text in sites:
        if not any(fnmatch.fnmatchcase(rel, e["path"]) and e["pattern"] in (name, "*") for e in tables["native"]):
            bad.append((rel, no, name, text))
    return bad


# ---- main -------------------------------------------------------------------------------------------------------


def load_allowlist(path: Path) -> dict[str, list[dict]]:
    try:
        tables = parse_allowlist(path.read_text(encoding="utf-8"))
    except (AllowlistError, OSError) as e:
        raise SystemExit(f"{path}: {e}")
    problems = validate_allowlist(tables)
    if problems:
        raise SystemExit(f"{path} is not valid:\n  " + "\n  ".join(problems))
    return tables


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--job", help="the name of the CI job (matched against the `jobs` patterns of the allow-list)")
    ap.add_argument("--report", action="append", default=[], type=Path, help="a skip report (JSON lines); repeatable")
    ap.add_argument("--log", action="append", default=[], type=Path, help="a raw log to read AG-SKIP lines from")
    ap.add_argument("--allowlist", type=Path, default=ALLOWLIST)
    ap.add_argument("--summary", type=Path, default=os.environ.get("GITHUB_STEP_SUMMARY"), help="append the table here")
    ap.add_argument("--scan", action="store_true", help="scan the source tree for unreviewed skip constructs")
    ap.add_argument("--validate", action="store_true", help="only check that the allow-list is well formed")
    ap.add_argument("--root", type=Path, default=ROOT, help="the tree --scan reads")
    args = ap.parse_args(argv)

    tables = load_allowlist(args.allowlist)
    status = 0
    did = False
    if args.validate:
        print(f"ok    {args.allowlist.name}: {len(tables['skip'])} skip entries, {len(tables['native'])} native entries")
        did = True
    if args.scan:
        did = True
        bad = unreviewed_sites(scan_tree(args.root), tables)
        for rel, no, name, text in bad:
            print(f"FAIL  {rel}:{no}: {name} outside the helper: {text}", file=sys.stderr)
        if bad:
            print(
                "      A test must not skip by itself: use require_component(...) of its language (see "
                "docs/architecture/test-skips.md), or review the site in tests/skip-allowlist.toml ([[native]]).",
                file=sys.stderr,
            )
            status = 1
        else:
            print("ok    no skip construct outside the helpers and the reviewed [[native]] list")
    if args.report or args.log or args.job:
        did = True
        if not args.job:
            ap.error("--job is required to judge a report")
        skips: list[dict] = []
        for r in args.report:
            skips += read_report(r)
        for lg in args.log:
            skips += read_log(lg)
        allowed, unreviewed = judge(skips, args.job, tables)
        text = render(args.job, allowed, unreviewed)
        print(text)
        if args.summary:
            try:
                with open(args.summary, "a", encoding="utf-8") as f:
                    f.write(text + "\n")
            except OSError:
                pass
        if unreviewed:
            for s in unreviewed:
                print(f"FAIL  {args.job}: {s['test']} skipped ({s['component']}): {s['reason']}", file=sys.stderr)
            print(
                "      A skip nobody reviewed. Make the job provide the component (and set AG_REQUIRE_COMPONENTS), or "
                "add a [[skip]] entry with a reason and an owner to tests/skip-allowlist.toml.",
                file=sys.stderr,
            )
            status = 1
    if not did:
        ap.error("nothing to do: give --job/--report, --scan or --validate")
    return status


if __name__ == "__main__":
    sys.exit(main())
