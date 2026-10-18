#!/usr/bin/env python3
"""Fail when production source contains the markers of unfinished or simulated code.

    python3 scripts/check_no_stubs.py              # exit 1 on a finding that is not allow-listed
    python3 scripts/check_no_stubs.py --list       # print every finding, allow-listed or not, with its reason
    python3 scripts/check_no_stubs.py --strict     # also fail on allow-list entries that no longer match anything

The bar for the product is: real components everywhere, configuration instead of literals, no mocks, no stubs, no
simulated behaviour in product code. A test double is acceptable only for something that cannot be real in a test, and
only in test code. This script keeps the *known markers* of the opposite from creeping back in:

    todo-macro       `todo!()`, `unimplemented!()`
    marker           TODO, FIXME, XXX, HACK
    for-now          "for now", "for the time being"
    not-implemented  "not implemented", "not yet implemented", "not supported yet", "coming soon"
    placeholder      "placeholder"
    stub             "stub", "stubs"
    test-double      mock, fake, dummy, simulate/simulated/simulation (also inside identifiers: FakeClock, mock_x)

What is scanned: production sources only. Rust `crates/*/src` and `build.rs` without `#[cfg(test)]` / `#[test]` items
and without the files such items pull in (`#[cfg(test)] mod tests;`); Go under `services/` without `_test.go` files
and without `testutil`/`testdata` directories; the Web UI (`apps/web/src`, TypeScript and Vue) and the TypeScript and
Python SDKs (`src`) without test files. Tests, examples, benches, scripts, deployment files and documentation are not
scanned: test code may use a double of a third-party service, and the documentation says which.

A pattern check cannot prove that nothing is a stub: a function that returns a constant has no marker. It stops the
markers (and the words people use for them) from coming back, and the audit (dev/audit/) and reviews cover the rest.

Exceptions live in scripts/no_stubs_allowlist.txt, one per line: `glob | rule | needle | reason`. `needle` is text that
must occur in the offending line (`*` = any line of that rule in those files); the reason is mandatory and says why the
use is legitimate. An entry that no longer matches anything is reported (and fails with --strict) so that the list
cannot silently grow stale. Usage tests: scripts/test_check_no_stubs.py.
"""
from __future__ import annotations

import argparse
import fnmatch
import re
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ALLOWLIST = ROOT / "scripts" / "no_stubs_allowlist.txt"

RULES: dict[str, re.Pattern[str]] = {
    "todo-macro": re.compile(r"\b(?:todo|unimplemented)!\s*[(\[{]"),
    "marker": re.compile(r"\b(?:TODO|FIXME|XXX|HACK)\b"),
    "for-now": re.compile(r"\bfor now\b|\bfor the time being\b", re.I),
    # (Not the bare word "unimplemented": it is also the name of a protocol error code, `Unimplemented`/HTTP 501.)
    "not-implemented": re.compile(r"\bnot (?:yet )?implemented\b|\bnot supported yet\b|\bcoming soon\b", re.I),
    "placeholder": re.compile(r"placeholder", re.I),
    # The word, in any capitalisation convention (stub, Stub, STUB, stub_store, StubStore), but not "stubborn".
    "stub": re.compile(r"(?<![A-Za-z])(?:[Ss]tubs?|STUBS?)(?![a-z])"),
    # Mock/Fake/Dummy/Simulat* as a word or the start of a CamelCase or snake_case identifier: FakeClock, mock_store,
    # is_dummy, simulated; but not "mockingbird" or "fakir".
    "test-double": re.compile(
        r"(?<![A-Za-z])(?:(?:[Mm]ock|MOCK|[Ff]ake|FAKE|[Dd]ummy|DUMMY)(?:s|ed)?|[Ss]imulat(?:e|es|ed|ing|ion|ions|or)|SIMULAT\w*)(?![a-z])"
    ),
}

# What is production code, per language: (glob under ROOT, excluded path components, excluded file suffixes)
SOURCES: list[tuple[str, str]] = [
    ("crates/*/src/**/*.rs", "rust"),
    ("crates/*/build.rs", "rust"),
    ("services/**/*.go", "go"),
    ("apps/web/src/**/*.ts", "web"),
    ("apps/web/src/**/*.vue", "web"),
    ("sdk/typescript/src/**/*.ts", "web"),
    ("sdk/python/src/**/*.py", "python"),
]
TEST_DIRS = {"testutil", "testdata", "__tests__", "tests", "test", "e2e", "node_modules", "fixtures", "mocks", "__mocks__"}
TEST_SUFFIXES = ("_test.go", ".test.ts", ".spec.ts", ".test.js", ".spec.js", "_test.py", ".stories.ts")


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    rule: str
    text: str


@dataclass
class Allow:
    glob: str
    rule: str
    needle: str
    reason: str
    lineno: int
    used: int = 0


# -- Rust: drop test items ----------------------------------------------------------------------------------


def _mask_rust(src: str) -> str:
    """`src` with comments, strings and char literals blanked (newlines kept), so braces can be counted."""
    out: list[str] = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        two = src[i : i + 2]
        if two == "//":
            j = src.find("\n", i)
            j = n if j < 0 else j
            out.append(" " * (j - i))
            i = j
        elif two == "/*":
            depth, j = 1, i + 2
            while j < n and depth:
                if src[j : j + 2] == "/*":
                    depth, j = depth + 1, j + 2
                elif src[j : j + 2] == "*/":
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            out.append("".join(ch if ch == "\n" else " " for ch in src[i:j]))
            i = j
        elif c == "r" and re.match(r'r#*"', src[i:]) and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
            hashes = len(re.match(r"r(#*)\"", src[i:]).group(1))
            end = src.find('"' + "#" * hashes, i + 2 + hashes)
            j = n if end < 0 else end + 1 + hashes
            out.append("".join(ch if ch == "\n" else " " for ch in src[i:j]))
            i = j
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            j = min(j + 1, n)
            out.append("".join(ch if ch == "\n" else " " for ch in src[i:j]))
            i = j
        elif c == "'":
            m = re.match(r"'(?:\\.[^']*|[^\\'])'", src[i:])  # a char literal; otherwise a lifetime
            if m:
                out.append(" " * len(m.group(0)))
                i += len(m.group(0))
            else:
                out.append(c)
                i += 1
        else:
            out.append(c)
            i += 1
    return "".join(out)


TEST_ATTR = re.compile(r"#\s*\[\s*(?:test|cfg\s*\((?:[^\]]*[(,\s])?test\b[^\]]*\))\s*\]")  # #[test], #[cfg(test)], #[cfg(any(.., test))]
NOT_TEST = re.compile(r"cfg\s*\(\s*not\s*\(\s*test")


def rust_production_lines(src: str) -> tuple[list[tuple[int, str]], list[str]]:
    """The (1-based line number, text) pairs of `src` that are not test code, and the `#[cfg(test)] mod x;` names."""
    masked = _mask_rust(src)
    lines = src.split("\n")
    keep = [True] * len(lines)
    test_mods: list[str] = []
    line_starts = [0]
    for ln in masked.split("\n"):
        line_starts.append(line_starts[-1] + len(ln) + 1)

    def line_of(pos: int) -> int:
        lo, hi = 0, len(line_starts) - 1
        while lo < hi:
            mid = (lo + hi + 1) // 2
            if line_starts[mid] <= pos:
                lo = mid
            else:
                hi = mid - 1
        return lo

    pos = 0
    for m in TEST_ATTR.finditer(masked):
        if m.start() < pos or NOT_TEST.search(m.group(0)):
            continue
        start_line = line_of(m.start())
        # skip further attributes, then find where the item ends: a `;` or the matching `}` at bracket depth 0
        j = m.end()
        while True:
            mm = re.match(r"\s*#\s*\[", masked[j:])
            if not mm:
                break
            depth, k = 1, j + mm.end()
            while k < len(masked) and depth:
                depth += {"[": 1, "]": -1}.get(masked[k], 0)
                k += 1
            j = k
        item_start = j
        depth_paren, k = 0, j
        end = None
        while k < len(masked):
            ch = masked[k]
            if ch in "([":
                depth_paren += 1
            elif ch in ")]":
                depth_paren -= 1
            elif ch == ";" and depth_paren <= 0:
                end = k
                break
            elif ch == "{" and depth_paren <= 0:
                depth, q = 1, k + 1
                while q < len(masked) and depth:
                    depth += {"{": 1, "}": -1}.get(masked[q], 0)
                    q += 1
                if depth:
                    raise ValueError("unbalanced braces after a test attribute")
                end = q - 1
                break
            k += 1
        if end is None:
            raise ValueError("a test attribute is not followed by an item")
        head = masked[item_start:k]
        mod = re.match(r"\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*$", head)
        if mod and masked[end] == ";":
            test_mods.append(mod.group(1))
        for ln in range(start_line, line_of(end) + 1):
            keep[ln] = False
        pos = end
    return [(i + 1, lines[i]) for i in range(len(lines)) if keep[i]], test_mods


def rust_test_files(files: list[Path]) -> set[Path]:
    """Files that are test modules because a `#[cfg(test)] mod x;` in the crate pulls them in."""
    out: set[Path] = set()
    for f in files:
        try:
            _, mods = rust_production_lines(f.read_text(errors="ignore"))
        except ValueError:
            continue
        if f.name in ("lib.rs", "main.rs", "mod.rs", "build.rs"):
            base = f.parent
        else:
            base = f.parent / f.stem
        for name in mods:
            out.add(base / f"{name}.rs")
            out.add(base / name / "mod.rs")
    return out


# -- scanning ------------------------------------------------------------------------------------------------


def is_test_path(rel: str) -> bool:
    parts = rel.split("/")
    return any(p in TEST_DIRS for p in parts[:-1]) or rel.endswith(TEST_SUFFIXES)


def production_files(root: Path) -> list[tuple[Path, str]]:
    found: dict[Path, str] = {}
    for pattern, kind in SOURCES:
        for f in root.glob(pattern):
            if f.is_file() and not is_test_path(str(f.relative_to(root))):
                found[f] = kind
    rust = [f for f, k in found.items() if k == "rust"]
    for tf in rust_test_files(rust):
        found.pop(tf, None)
    return sorted(found.items())


NAME_RULES = {"stub": re.compile(r"stub", re.I), "test-double": re.compile(r"mock|fake|dummy", re.I)}


def scan(root: Path = ROOT) -> list[Finding]:
    findings: list[Finding] = []
    for f, kind in production_files(root):
        rel = str(f.relative_to(root))
        # A production file whose own name says stub/mock/fake/dummy (line 0 = the file name)
        for rule, pat in NAME_RULES.items():
            if pat.search(f.stem):
                findings.append(Finding(rel, 0, rule, f"(file name) {f.name}"))
        text = f.read_text(errors="ignore")
        lines = rust_production_lines(text)[0] if kind == "rust" else list(enumerate(text.split("\n"), 1))
        for no, line in lines:
            for rule, pat in RULES.items():
                if rule == "todo-macro" and kind != "rust":
                    continue
                if pat.search(line):
                    findings.append(Finding(rel, no, rule, line.strip()))
    return findings


# -- allow-list ----------------------------------------------------------------------------------------------


def load_allowlist(path: Path = ALLOWLIST) -> list[Allow]:
    entries: list[Allow] = []
    if not path.exists():
        return entries
    for no, raw in enumerate(path.read_text().split("\n"), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = [p.strip() for p in line.split("|", 3)]
        if len(parts) != 4 or not all(parts):
            raise SystemExit(f"{path.name}:{no}: expected `glob | rule | needle | reason`, got: {raw!r}")
        glob, rule, needle, reason = parts
        if rule not in RULES:
            raise SystemExit(f"{path.name}:{no}: unknown rule {rule!r} (known: {', '.join(RULES)})")
        if len(reason) < 12:
            raise SystemExit(f"{path.name}:{no}: the reason is too short to be one: {reason!r}")
        entries.append(Allow(glob, rule, needle, reason, no))
    return entries


def match_allow(f: Finding, entries: list[Allow]) -> Allow | None:
    for e in entries:
        if e.rule == f.rule and fnmatch.fnmatch(f.path, e.glob) and (e.needle == "*" or e.needle in f.text):
            e.used += 1
            return e
    return None


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--list", action="store_true", help="print every finding with its allow-list reason")
    ap.add_argument("--strict", action="store_true", help="fail on allow-list entries that match nothing")
    ap.add_argument("--root", type=Path, default=ROOT, help="repository root (tests use a scratch tree)")
    ap.add_argument("--allowlist", type=Path, default=None, help="allow-list file (default: scripts/no_stubs_allowlist.txt)")
    args = ap.parse_args(argv)
    root = args.root.resolve()
    entries = load_allowlist(args.allowlist or (root / "scripts" / "no_stubs_allowlist.txt"))
    findings = scan(root)
    bad: list[Finding] = []
    for f in findings:
        e = match_allow(f, entries)
        if e is None:
            bad.append(f)
        elif args.list:
            print(f"allowed  {f.path}:{f.line}: [{f.rule}] {f.text[:100]}\n         because: {e.reason}")
    for f in bad:
        print(f"{f.path}:{f.line}: [{f.rule}] {f.text[:140]}")
    stale = [e for e in entries if e.used == 0]
    for e in stale:
        print(f"{('error' if args.strict else 'warning')}: allow-list line {e.lineno} matches nothing: {e.glob} | {e.rule} | {e.needle}",
              file=sys.stderr)
    scanned = len(production_files(root))
    if bad:
        print(
            f"\n{len(bad)} marker(s) of unfinished or simulated code in production source ({scanned} files scanned).\n"
            "Finish the code, or, if the use is legitimate, add a line with a reason to scripts/no_stubs_allowlist.txt.",
            file=sys.stderr,
        )
        return 1
    if args.strict and stale:
        return 1
    print(f"no stubs: {scanned} production files scanned, {len(findings)} allow-listed use(s), 0 markers")
    return 0


if __name__ == "__main__":
    sys.exit(main())
