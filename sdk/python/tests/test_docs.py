"""Documentation cannot drift from the code: every ```python block in the READMEs and in the SDK guide
is a verbatim copy of a region of a program under examples/ (which test_examples.py runs), announced by

    <!-- example: sdk/python/examples/quickstart.py#quickstart -->
"""

from __future__ import annotations

import re
import textwrap
from dataclasses import dataclass

import pytest

from tests.harness import REPO_ROOT

DOCS = ["sdk/python/README.md", "docs/guides/sdks.md"]
MARKER = re.compile(r"^<!-- example: (\S+) -->$")


def region_text(file: str, name: str) -> str:
    lines = (REPO_ROOT / file).read_text().splitlines()
    start = next((i for i, line in enumerate(lines) if line.strip() == f"# region {name}"), None)
    end = next((i for i, line in enumerate(lines) if line.strip() == f"# endregion {name}"), None)
    if start is None or end is None or end < start:
        raise AssertionError(f"{file}: no region {name!r}")
    return textwrap.dedent("\n".join(lines[start + 1 : end]))


@dataclass
class Block:
    doc: str
    line: int
    lang: str
    marker: str | None
    text: str


def blocks(doc: str) -> list[Block]:
    lines = (REPO_ROOT / doc).read_text().splitlines()
    out: list[Block] = []
    i = 0
    while i < len(lines):
        opening = re.fullmatch(r"```(\w+)\s*", lines[i])
        if not opening:
            i += 1
            continue
        j = i + 1
        while j < len(lines) and not re.fullmatch(r"```\s*", lines[j]):
            j += 1
        k = i - 1
        while k >= 0 and not lines[k].strip():
            k -= 1
        m = MARKER.match(lines[k]) if k >= 0 else None
        out.append(
            Block(doc, i + 1, opening.group(1), m.group(1) if m else None, "\n".join(lines[i + 1 : j]))
        )
        i = j + 1
    return out


@pytest.mark.parametrize("doc", DOCS)
def test_every_python_block_is_a_verbatim_region_of_an_example(doc: str) -> None:
    assert (REPO_ROOT / doc).exists(), f"{doc} exists"
    python = [b for b in blocks(doc) if b.lang in ("python", "py")]
    for b in python:
        assert b.marker, f"{doc}:{b.line} needs an <!-- example: path#region --> marker"
        file, region = b.marker.split("#")
        assert re.fullmatch(r"sdk/python/examples/[\w-]+\.py", file), f"{doc}:{b.line}"
        assert b.text == region_text(file, region), f"{doc}:{b.line} differs from {b.marker}"
    if doc == "sdk/python/README.md":
        assert len(python) >= 7


def test_the_guide_only_has_known_languages() -> None:
    for b in blocks("docs/guides/sdks.md"):
        assert b.lang in ("ts", "typescript", "python", "bash", "sh", "text", "json", "toml"), (
            f"{b.doc}:{b.line} {b.lang}"
        )


def test_every_region_quoted_somewhere_exists() -> None:
    for doc in DOCS:
        for b in blocks(doc):
            if b.marker and b.marker.startswith("sdk/python/"):
                file, region = b.marker.split("#")
                region_text(file, region)
