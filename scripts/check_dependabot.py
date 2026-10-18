#!/usr/bin/env python3
"""Fails when a dependency manifest of the repository is not covered by .github/dependabot.yml.

A lock file nobody updates is a vulnerability scheduled for later. For every manifest below, an `updates:` entry of
the matching ecosystem must name its directory, and every `directory:` in the file must exist:

  Cargo.lock             cargo            package-lock.json     npm
  uv.lock                uv               go.mod                gomod
  Dockerfile*            docker           compose.y(a)ml, docker-compose*.y(a)ml   docker-compose
  .github/workflows/     github-actions (directory "/")

Deliberate gaps are listed in NOT_UPDATED with the reason; each must still exist (a stale exception fails too).

Usage: scripts/check_dependabot.py [repository-root]
"""
from __future__ import annotations

import fnmatch
import pathlib
import sys

try:
    import yaml
except ImportError:  # pragma: no cover
    sys.exit("PyYAML is required: pip install pyyaml")

ROOT = pathlib.Path(__file__).resolve().parent.parent
SKIP_DIRS = {".git", "node_modules", "target", ".venv", ".venv-docs", "site", "dist", "__pycache__", "vendor"}

MANIFESTS = [
    ("Cargo.lock", "cargo"),
    ("package-lock.json", "npm"),
    ("uv.lock", "uv"),
    ("go.mod", "gomod"),
    ("Dockerfile*", "docker"),
    ("compose.yaml", "docker-compose"),
    ("compose.yml", "docker-compose"),
    ("docker-compose*.yaml", "docker-compose"),
    ("docker-compose*.yml", "docker-compose"),
]

# (ecosystem, directory) pairs that are deliberately not updated by Dependabot, and why.
NOT_UPDATED = {
    ("pip", "/docs"): "docs/requirements.txt is hash-locked by `uv pip compile`; a Dependabot update would drop the hashes",
}


def discover(root: pathlib.Path) -> set[tuple[str, str]]:
    found: set[tuple[str, str]] = set()
    for path in root.rglob("*"):
        rel = path.relative_to(root)
        if any(part in SKIP_DIRS for part in rel.parts) or not path.is_file():
            continue
        for pattern, ecosystem in MANIFESTS:
            if fnmatch.fnmatch(path.name, pattern):
                directory = "/" + "/".join(rel.parts[:-1])
                found.add((ecosystem, directory if directory != "/" else "/"))
    if list((root / ".github" / "workflows").glob("*.y*ml")):
        found.add(("github-actions", "/"))
    if (root / "docs" / "requirements.txt").exists():
        found.add(("pip", "/docs"))
    return found


def configured(root: pathlib.Path) -> set[tuple[str, str]]:
    doc = yaml.safe_load((root / ".github" / "dependabot.yml").read_text())
    if doc.get("version") != 2:
        raise SystemExit("dependabot.yml: version must be 2")
    out = set()
    for entry in doc.get("updates") or []:
        for key in ("package-ecosystem", "directory", "schedule"):
            if key not in entry:
                raise SystemExit(f"dependabot.yml: an update has no `{key}`: {entry}")
        out.add((entry["package-ecosystem"], entry["directory"]))
    return out


def problems(root: pathlib.Path, not_updated: dict = NOT_UPDATED) -> list[str]:
    have = discover(root)
    want = configured(root)
    out = []
    for eco, directory in sorted(have - want - set(not_updated)):
        out.append(f"{directory} has a {eco} manifest that .github/dependabot.yml does not update")
    for eco, directory in sorted(want - have):
        out.append(f"dependabot.yml updates {eco} in {directory}, where no such manifest exists")
    for (eco, directory), why in not_updated.items():
        if (eco, directory) not in have:
            out.append(f"NOT_UPDATED lists {eco} {directory} ({why}), which no longer exists: remove the exception")
        if (eco, directory) in want:
            out.append(f"NOT_UPDATED lists {eco} {directory}, but dependabot.yml now updates it: remove the exception")
    return out


def main() -> int:
    root = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT
    found = problems(root)
    for p in found:
        print(p, file=sys.stderr)
    if found:
        return 1
    print(f"dependabot.yml covers {len(discover(root)) - len(NOT_UPDATED)} manifest directories")
    return 0


if __name__ == "__main__":
    sys.exit(main())
