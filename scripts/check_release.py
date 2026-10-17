#!/usr/bin/env python3
"""Checks that a release tag matches the code, and extracts its CHANGELOG notes.

    scripts/check_release.py v0.2.0           # prints `version=0.2.0`; fails if the tag, Cargo.toml
                                              # and CHANGELOG.md disagree
    scripts/check_release.py v0.2.0 --notes   # prints the CHANGELOG section of that version
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent


def die(msg: str) -> None:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def workspace_version() -> str:
    text = (ROOT / "Cargo.toml").read_text()
    section = re.search(r"\[workspace\.package\](.*?)(?:\n\[|\Z)", text, re.S)
    m = re.search(r'^version\s*=\s*"([^"]+)"', section.group(1) if section else "", re.M)
    if not m:
        die("no [workspace.package] version in Cargo.toml")
    return m.group(1)


def changelog_section(version: str) -> str:
    text = (ROOT / "CHANGELOG.md").read_text()
    m = re.search(rf"^## \[{re.escape(version)}\][^\n]*\n(.*?)(?=^## \[|\Z)", text, re.S | re.M)
    if not m or not m.group(1).strip():
        die(f"CHANGELOG.md has no (non-empty) section for [{version}]")
    return m.group(1).strip() + "\n"


def main() -> None:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if len(args) != 1:
        die("usage: check_release.py vX.Y.Z [--notes]")
    tag = args[0]
    m = re.fullmatch(r"v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)", tag)
    if not m:
        die(f"'{tag}' is not a version tag like v1.2.3 or v1.2.3-rc.1")
    version = m.group(1)
    if version != workspace_version():
        die(f"tag {tag} does not match the workspace version {workspace_version()} in Cargo.toml")
    notes = changelog_section(version)
    print(notes if "--notes" in sys.argv else f"version={version}")


main()
