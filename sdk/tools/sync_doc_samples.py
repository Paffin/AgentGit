#!/usr/bin/env python3
"""Copies the marked regions of the SDK examples into the documentation.

Every code block of the SDK READMEs and of docs/guides/sdks.md that follows a marker line

    <!-- example: sdk/python/examples/quickstart.py#quickstart -->

is a verbatim copy of the region between ``# region quickstart`` and ``# endregion quickstart`` (or
``// #region`` / ``// #endregion`` in TypeScript). The tests of both SDKs fail when a block differs from
its region; this tool rewrites the blocks so that they match again:

    python3 sdk/tools/sync_doc_samples.py          rewrite
    python3 sdk/tools/sync_doc_samples.py --check  exit 1 if anything would change
"""

from __future__ import annotations

import re
import sys
import textwrap
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOCS = ["sdk/typescript/README.md", "sdk/python/README.md", "docs/guides/sdks.md"]
MARKER = re.compile(r"^<!-- example: (\S+)#(\S+) -->$")


def region(file: str, name: str) -> str:
    prefix = "// #" if file.endswith(".ts") else "# "
    lines = (ROOT / file).read_text().splitlines()
    start = next((i for i, line in enumerate(lines) if line.strip() == f"{prefix}region {name}"), None)
    end = next((i for i, line in enumerate(lines) if line.strip() == f"{prefix}endregion {name}"), None)
    if start is None or end is None or end < start:
        raise SystemExit(f"{file}: no region {name!r}")
    return textwrap.dedent("\n".join(lines[start + 1 : end]))


def sync(doc: str) -> str:
    lines = (ROOT / doc).read_text().split("\n")
    out: list[str] = []
    i = 0
    while i < len(lines):
        out.append(lines[i])
        m = MARKER.match(lines[i])
        i += 1
        if not m:
            continue
        # blank lines, then the fenced block
        while i < len(lines) and not lines[i].strip():
            out.append(lines[i])
            i += 1
        if i >= len(lines) or not re.match(r"^```\w+\s*$", lines[i]):
            raise SystemExit(f"{doc}: the marker {m.group(0)} is not followed by a code block")
        out.append(lines[i])
        i += 1
        while i < len(lines) and not re.match(r"^```\s*$", lines[i]):
            i += 1
        out.extend(region(m.group(1), m.group(2)).split("\n"))
    return "\n".join(out)


def main() -> None:
    changed = []
    for doc in DOCS:
        path = ROOT / doc
        if not path.exists():
            continue
        new = sync(doc)
        if new != path.read_text():
            changed.append(doc)
            if "--check" not in sys.argv:
                path.write_text(new)
    if "--check" in sys.argv:
        if changed:
            print("out of date: " + ", ".join(changed) + " - run python3 sdk/tools/sync_doc_samples.py", file=sys.stderr)
            sys.exit(1)
        print("documentation samples are up to date")
    else:
        print("updated: " + (", ".join(changed) if changed else "nothing"))


if __name__ == "__main__":
    main()
