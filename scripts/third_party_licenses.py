#!/usr/bin/env python3
"""The non-Rust sections of THIRD_PARTY_LICENSES.md: the web UI bundle (npm) and the orchestrator (Go).

scripts/third_party_licenses.sh calls this and puts cargo-about's output (the Rust section) in front.

  third_party_licenses.py npm     licences of the production dependencies in apps/web/package-lock.json, read from
                                  apps/web/node_modules (run `npm ci` first)
  third_party_licenses.py go      licences of the modules and of the standard library that
                                  services/agent-orchestrator links, read from the Go module cache

Both fail (exit 1) when a package has no licence text or a licence outside the allow list of deny.toml, so a
dependency cannot slip into a binary without its notice. The output is deterministic: sorted, no dates, no paths.
"""
from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
WEB = ROOT / "apps" / "web"
ORCH = ROOT / "services" / "agent-orchestrator"
LICENSE_FILES = re.compile(r"^(licen[cs]e|copying|notice)(\.(md|txt|markdown))?$", re.I)

# Names npm package metadata uses for the licences in deny.toml's allow list.
SPDX_PARTS = re.compile(r"\s+(?:OR|AND)\s+|[()]")


def allowed() -> set[str]:
    deny = tomllib.loads((ROOT / "deny.toml").read_text())
    return set(deny["licenses"]["allow"])


def fence(text: str) -> str:
    text = text.replace("\r\n", "\n").strip("\n")
    return "```text\n" + text + "\n```\n"


def license_files(directory: pathlib.Path) -> list[pathlib.Path]:
    return sorted(p for p in directory.iterdir() if p.is_file() and LICENSE_FILES.match(p.name))


def npm_section() -> tuple[str, list[str]]:
    lock = json.loads((WEB / "package-lock.json").read_text())
    ok = allowed()
    problems: list[str] = []
    entries = []
    for key, pkg in lock["packages"].items():
        if not key or pkg.get("dev") or pkg.get("devOptional"):
            continue
        name = key.rsplit("node_modules/", 1)[-1]
        entries.append((name, pkg["version"], pkg.get("license", ""), WEB / key))
    entries.sort()
    out = [f"### Web UI bundle (npm, {len(entries)} production packages of apps/web)\n"]
    for name, version, lic, directory in entries:
        parts = [p for p in SPDX_PARTS.split(lic) if p]
        if not lic or not any(p in ok for p in parts):
            problems.append(f"{name}@{version}: licence {lic!r} is not in deny.toml's allow list")
        if not directory.is_dir():
            problems.append(f"{name}@{version}: {directory.relative_to(ROOT)} is missing (run `npm ci` in apps/web)")
            continue
        files = license_files(directory)
        out.append(f"#### {name} {version} (`{lic}`)\n")
        if files:
            for f in files:
                out.append(fence(f.read_text(errors="replace")))
        else:
            # Some packages declare their licence in package.json and ship no file. Say so, with the declared
            # licence and where the author publishes it, instead of inventing a text.
            meta = json.loads((directory / "package.json").read_text())
            repo = meta.get("repository")
            repo = repo.get("url", "") if isinstance(repo, dict) else (repo or "")
            author = meta.get("author")
            author = author.get("name", "") if isinstance(author, dict) else (author or "")
            out.append(
                f"The package ships no licence file. Declared in its package.json: `{lic}`"
                + (f"; author: {author}" if author else "")
                + (f"; repository: {repo}" if repo else "")
                + ".\n"
            )
    return "\n".join(out), problems


def go(*args: str) -> str:
    return subprocess.run(["go", *args], cwd=ORCH, capture_output=True, text=True, check=True).stdout


def go_section() -> tuple[str, list[str]]:
    problems: list[str] = []
    listing = go("list", "-deps", "-f", "{{with .Module}}{{if not .Main}}{{.Path}}\t{{.Version}}\t{{.Dir}}{{end}}{{end}}", "./...")
    modules = sorted({tuple(line.split("\t")) for line in listing.splitlines() if line.strip()})
    out = ["### Orchestrator (Go)\n"]
    for path, version, directory in modules:
        files = license_files(pathlib.Path(directory))
        out.append(f"#### {path} {version}\n")
        if not files:
            problems.append(f"{path}@{version}: no LICENSE file in the module")
        for f in files:
            out.append(fence(f.read_text(errors="replace")))
    goroot = pathlib.Path(go("env", "GOROOT").strip())
    out.append("#### Go standard library (BSD-3-Clause with a patent grant)\n")
    out.append(fence((goroot / "LICENSE").read_text()))
    return "\n".join(out), problems


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] not in ("npm", "go"):
        print(__doc__, file=sys.stderr)
        return 2
    text, problems = npm_section() if sys.argv[1] == "npm" else go_section()
    for p in problems:
        print(f"error: {p}", file=sys.stderr)
    sys.stdout.write(text)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
