#!/usr/bin/env python3
"""Markdown summary of a cargo-mutants run, from its `mutants.out` directory.

  scripts/mutants_summary.py mutants-out/mutants.out ag-merge >> "$GITHUB_STEP_SUMMARY"

cargo-mutants writes caught.txt, missed.txt, timeout.txt and unviable.txt (one mutant per line, with file, line and the
change). The score is caught / (caught + missed + timeout): an unviable mutant does not compile, which is no statement
about the tests. Survivors are listed, because a number alone does not say which test to write.
"""
from __future__ import annotations

import pathlib
import sys


def lines(directory: pathlib.Path, name: str) -> list[str]:
    path = directory / name
    return [l for l in path.read_text().splitlines() if l.strip()] if path.exists() else []


def summary(directory: pathlib.Path, crate: str) -> str:
    caught, missed, timeout, unviable = (lines(directory, f"{n}.txt") for n in ("caught", "missed", "timeout", "unviable"))
    judged = len(caught) + len(missed) + len(timeout)
    score = 100.0 * (len(caught) + len(timeout)) / judged if judged else 100.0
    out = [
        f"### Mutation testing: `{crate}`",
        "",
        f"{len(caught)} caught, {len(timeout)} timed out (caught by the timeout), **{len(missed)} missed**, "
        f"{len(unviable)} did not compile. Score {score:.1f}% of {judged} mutants that built.",
        "",
    ]
    if missed:
        out += ["Survivors:", "", "```text", *missed, "```", ""]
    return "\n".join(out)


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    directory = pathlib.Path(sys.argv[1])
    if not directory.is_dir():
        print(f"{directory} does not exist: cargo-mutants did not run", file=sys.stderr)
        return 2
    print(summary(directory, sys.argv[2]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
