#!/usr/bin/env python3
"""Fills the package-manager templates under packaging/ from a release's SHA256SUMS.

    scripts/render_packaging.py --version 0.2.0 --sums dist/SHA256SUMS --out dist
    scripts/render_packaging.py --version 0.2.0 --sums dist/SHA256SUMS --out dist --only homebrew
    scripts/render_packaging.py --version 0.2.0 --sums SHA256SUMS --out /tmp/pkg --base-url file:///tmp/dist

Placeholders in the templates:

    @VERSION@                version without the leading v
    @BASE_URL@               where the archives live (default: the GitHub release of v<version>)
    @SHA256_<target>@        checksum of agentgit-<version>-<target>.(tar.gz|zip) from SHA256SUMS

A checksum that SHA256SUMS does not have is an error (a formula with a made-up hash would only fail later,
on a user's machine). Pass --allow-missing to render anyway; the missing hashes become 64 zeros so that the
file is plainly unusable. Writes <out>/agentgit.rb and <out>/agentgit.json.
"""
import argparse
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
TEMPLATES = {
    "homebrew": ("packaging/homebrew/agentgit.rb", "agentgit.rb"),
    "scoop": ("packaging/scoop/agentgit.json", "agentgit.json"),
}
ZEROS = "0" * 64


def die(msg: str) -> None:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def read_sums(path: pathlib.Path) -> dict[str, str]:
    sums: dict[str, str] = {}
    for n, line in enumerate(path.read_text().splitlines(), 1):
        if not line.strip():
            continue
        m = re.fullmatch(r"([0-9a-f]{64}) [ *]?(\S+)", line.strip())
        if not m:
            die(f"{path}:{n}: not a sha256sum line: {line!r}")
        sums[m.group(2)] = m.group(1)
    return sums


def render(text: str, version: str, base_url: str, sums: dict[str, str], allow_missing: bool) -> str:
    missing: list[str] = []

    def sha(m: re.Match) -> str:
        target = m.group(1)
        for ext in ("tar.gz", "zip"):
            digest = sums.get(f"agentgit-{version}-{target}.{ext}")
            if digest:
                return digest
        missing.append(target)
        return ZEROS

    out = re.sub(r"@SHA256_([A-Za-z0-9_.-]+)@", sha, text)
    out = out.replace("@VERSION@", version).replace("@BASE_URL@", base_url)
    if missing and not allow_missing:
        die(f"SHA256SUMS has no archive for version {version} and target(s): {', '.join(sorted(set(missing)))}")
    if missing:
        print(f"warning: no checksum for {', '.join(sorted(set(missing)))}; wrote zeros", file=sys.stderr)
    # The template's explanatory header does not belong in the rendered formula.
    out = re.sub(r"\A(?:#[^\n]*\n)+(?=class )", f"# Rendered by scripts/render_packaging.py for AgentGit {version}.\n", out)
    left = re.findall(r"@[A-Z0-9_a-z.-]+@", out)
    if left:
        die(f"unreplaced placeholders: {sorted(set(left))}")
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--version", required=True)
    ap.add_argument("--sums", required=True, type=pathlib.Path)
    ap.add_argument("--out", required=True, type=pathlib.Path)
    ap.add_argument("--base-url")
    ap.add_argument("--only", choices=sorted(TEMPLATES))
    ap.add_argument("--allow-missing", action="store_true")
    args = ap.parse_args()
    version = args.version.removeprefix("v")
    base_url = (args.base_url or f"https://github.com/Paffin/AgentGit/releases/download/v{version}").rstrip("/")
    sums = read_sums(args.sums)
    args.out.mkdir(parents=True, exist_ok=True)
    for name, (src, dest) in TEMPLATES.items():
        if args.only and args.only != name:
            continue
        text = render((ROOT / src).read_text(), version, base_url, sums, args.allow_missing)
        if dest.endswith(".json"):
            json.loads(text)  # a broken manifest must not be written
        (args.out / dest).write_text(text)
        print(args.out / dest)


if __name__ == "__main__":
    main()
