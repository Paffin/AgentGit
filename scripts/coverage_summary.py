#!/usr/bin/env python3
"""Per-crate coverage from a `cargo llvm-cov` JSON export, and a floor per crate.

  cargo llvm-cov --workspace --json --summary-only --output-path cov.json
  scripts/coverage_summary.py cov.json                              print the table
  scripts/coverage_summary.py cov.json --floors scripts/coverage_floors.toml
                                                                     ... and fail when a crate is below its floor
  scripts/coverage_summary.py cov.json --markdown "$GITHUB_STEP_SUMMARY"
                                                                     ... and append the table to a file

A file belongs to the crate whose `crates/<name>/src/` it lives in; tests, examples and benches of a crate are not
counted (they are the measuring instrument, not the thing measured). The floors file is TOML:

  [lines]          # minimum line coverage in percent, per crate
  ag-types = 90

A crate in the floors file that the report does not contain is a failure (a floor that measures nothing).
"""
from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import sys
import tomllib

CRATE = re.compile(r"/crates/([^/]+)/src/")


def per_crate(report: dict) -> dict[str, dict[str, tuple[int, int]]]:
    """crate -> {"lines"|"functions"|"regions": (covered, total)}."""
    out: dict[str, dict[str, list[int]]] = {}
    for export in report["data"]:
        for f in export["files"]:
            m = CRATE.search(f["filename"])
            if not m:
                continue
            acc = out.setdefault(m.group(1), {"lines": [0, 0], "functions": [0, 0], "regions": [0, 0]})
            for kind in acc:
                acc[kind][0] += f["summary"][kind]["covered"]
                acc[kind][1] += f["summary"][kind]["count"]
    return {c: {k: (v[0], v[1]) for k, v in kinds.items()} for c, kinds in sorted(out.items())}


def pct(pair: tuple[int, int]) -> float:
    return 100.0 * pair[0] / pair[1] if pair[1] else 100.0


def table(crates: dict[str, dict[str, tuple[int, int]]]) -> str:
    rows = ["| crate | lines | functions | regions |", "|---|---:|---:|---:|"]
    for name, k in crates.items():
        rows.append(
            f"| {name} | {pct(k['lines']):.1f}% ({k['lines'][0]}/{k['lines'][1]}) "
            f"| {pct(k['functions']):.1f}% | {pct(k['regions']):.1f}% |"
        )
    total = {kind: (sum(c[kind][0] for c in crates.values()), sum(c[kind][1] for c in crates.values())) for kind in ("lines", "functions", "regions")}
    rows.append(f"| **all crates** | **{pct(total['lines']):.1f}%** ({total['lines'][0]}/{total['lines'][1]}) | {pct(total['functions']):.1f}% | {pct(total['regions']):.1f}% |")
    return "\n".join(rows)


def check_floors(crates: dict, floors: dict[str, float]) -> list[str]:
    problems = []
    for name, floor in sorted(floors.items()):
        if name not in crates:
            problems.append(f"{name}: has a floor of {floor}% but the report has no source file of it")
        elif pct(crates[name]["lines"]) < floor:
            problems.append(f"{name}: line coverage {pct(crates[name]['lines']):.1f}% is below its floor of {floor}%")
    return problems


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("report", type=pathlib.Path)
    ap.add_argument("--floors", type=pathlib.Path)
    ap.add_argument("--markdown", type=pathlib.Path)
    args = ap.parse_args()
    crates = per_crate(json.loads(args.report.read_text()))
    if not crates:
        print("the report contains no file under crates/<name>/src/", file=sys.stderr)
        return 2
    text = table(crates)
    print(text)
    if args.markdown:
        with args.markdown.open("a") as fh:
            fh.write("### Coverage by crate\n\n" + text + "\n")
    if args.floors:
        floors = tomllib.loads(args.floors.read_text()).get("lines", {})
        problems = check_floors(crates, floors)
        for p in problems:
            print(f"::error title=coverage::{p}" if "GITHUB_ACTIONS" in os.environ else f"error: {p}", file=sys.stderr)
        if problems:
            return 1
        print(f"\nall {len(floors)} floors hold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
