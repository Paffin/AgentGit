#!/usr/bin/env python3
"""Checks that a release tag matches the code, and extracts its CHANGELOG notes.

    scripts/check_release.py v0.2.0                 # prints key=value lines (see below); fails if the tag,
                                                    # Cargo.toml and CHANGELOG.md disagree
    scripts/check_release.py v0.2.0 --notes         # prints the CHANGELOG section of that version
    scripts/check_release.py 0.0.0-rc.dry --dry-run # a rehearsal: any semver version; Cargo.toml need not agree

Output (one `key=value` per line, ready for $GITHUB_OUTPUT):

    version=0.2.0              the version without the leading `v`
    cargo_version=0.2.0        what Cargo.toml says (what the binaries report in `--version`)
    prerelease=false           true when the version has a `-suffix`
    dry_run=false
    notes_section=0.2.0        the CHANGELOG section the release notes come from

A real release needs the tag to equal the workspace version and a non-empty `## [X.Y.Z]` CHANGELOG section.
`--dry-run` (the release workflow's rehearsal, which publishes nothing) relaxes both: the version is only
checked for being semver, and without a section of its own the notes come from `## [Unreleased]`.
"""
import os
import pathlib
import re
import sys

# AG_RELEASE_ROOT lets the tests point the script at a fixture checkout.
ROOT = pathlib.Path(os.environ.get("AG_RELEASE_ROOT") or pathlib.Path(__file__).resolve().parent.parent)
SEMVER = r"(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)"


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


def changelog_section(name: str) -> str | None:
    """The body of `## [name]`, or None if there is no such heading. Fails if the heading exists but is empty."""
    text = (ROOT / "CHANGELOG.md").read_text()
    m = re.search(rf"^## \[{re.escape(name)}\][^\n]*\n(.*?)(?=^## \[|\Z)", text, re.S | re.M)
    if not m:
        return None
    if not m.group(1).strip():
        die(f"CHANGELOG.md has an empty section for [{name}]")
    return m.group(1).strip() + "\n"


def resolve(ref: str, dry_run: bool) -> dict:
    """Validates `ref` (a tag `v1.2.3` or, for a rehearsal, a bare `1.2.3-rc.1`); returns the facts."""
    pattern = rf"v{SEMVER}" if not dry_run else rf"v?{SEMVER}"
    m = re.fullmatch(pattern, ref)
    if not m:
        shape = "a version like 1.2.3 or 1.2.3-rc.1 (optionally with a leading v)" if dry_run else (
            "a version tag like v1.2.3 or v1.2.3-rc.1")
        die(f"'{ref}' is not {shape}")
    version, cargo = m.group(1), workspace_version()
    if not dry_run and version != cargo:
        die(f"tag {ref} does not match the workspace version {cargo} in Cargo.toml")
    notes = changelog_section(version)
    section = version
    if notes is None and dry_run:
        notes, section = changelog_section("Unreleased"), "Unreleased"
    if notes is None:
        die(f"CHANGELOG.md has no (non-empty) section for [{version}]" +
            (" or [Unreleased]" if dry_run else ""))
    return {
        "version": version,
        "cargo_version": cargo,
        "prerelease": str("-" in version).lower(),
        "dry_run": str(dry_run).lower(),
        "notes_section": section,
        "notes": notes,
    }


def main() -> None:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if len(args) != 1:
        die("usage: check_release.py (vX.Y.Z | X.Y.Z-rc.N --dry-run) [--notes]")
    facts = resolve(args[0], "--dry-run" in sys.argv)
    if "--notes" in sys.argv:
        print(facts["notes"])
    else:
        for key in ("version", "cargo_version", "prerelease", "dry_run", "notes_section"):
            print(f"{key}={facts[key]}")


if __name__ == "__main__":
    main()
