#!/usr/bin/env python3
"""Checks that everything that can be published has complete package metadata, and that nothing is half-ready.

    python3 scripts/check_publish_metadata.py            # crates, npm and PyPI packages
    python3 scripts/check_publish_metadata.py --online   # also: are the names free on crates.io / npm / PyPI?

Rust crates (`cargo metadata`): every package is named `agentgit-<x>` for the directory `crates/ag-<x>` and keeps
the library name `ag_<x>` (so `use ag_<x>::` paths and `-p agentgit-<x>` both work; the `ag-*` names on crates.io
belong to other people); every crate has a description that says something, license, repository, homepage,
documentation, readme (the file exists), at most five keywords of at most 20 characters, categories, a rust-version
and the workspace version; no source of a publishable crate `include_str!`s or `include_bytes!`s a file outside the
crate (cargo does not package it). A crate that is publishable (no `publish = false`) must only depend on publishable
workspace crates; a crate that is not publishable must say why in its Cargo.toml (the comment above
`publish = false`). `--list` prints the publishable crates in dependency order (the order to publish them in).

TypeScript SDK (sdk/typescript/package.json) and Python SDK (sdk/python/pyproject.toml): name, description,
license, repository, homepage / URLs, keywords, readme, version equal to the workspace version.

Nothing here publishes anything. Python 3.11+ standard library only (tomllib).
"""
import json
import pathlib
import re
import subprocess
import sys
import tomllib
import urllib.error
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
failures: list[str] = []


def bad(msg: str) -> None:
    failures.append(msg)
    print(f"FAIL  {msg}")


def ok(msg: str) -> None:
    print(f"ok    {msg}")


def cargo_metadata() -> dict:
    out = subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps", "--locked"], cwd=ROOT,
                         capture_output=True, text=True)
    if out.returncode:
        sys.exit(out.stderr)
    return json.loads(out.stdout)


def workspace_version() -> str:
    return tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]


def check_crates(meta: dict) -> list[str]:
    """Returns the publishable crate names in dependency order."""
    pkgs = {p["name"]: p for p in meta["packages"]}
    publishable = {n for n, p in pkgs.items() if p["publish"] is None}
    version = workspace_version()
    for name, p in sorted(pkgs.items()):
        where = f"crate {name}"
        manifest = pathlib.Path(p["manifest_path"])
        dirname = manifest.parent.name
        if name != "agentgit-" + dirname.removeprefix("ag-"):
            bad(f"{where}: the package of crates/{dirname} must be named agentgit-{dirname.removeprefix('ag-')}")
        for tgt in p["targets"]:
            if "lib" in tgt["kind"] and tgt["name"] != dirname.replace("-", "_"):
                bad(f"{where}: the library is named {tgt['name']}, expected {dirname.replace('-', '_')} (set [lib] name)")
        if p["version"] != version:
            bad(f"{where}: version {p['version']} is not the workspace version {version}")
        if name in publishable:
            desc = (p["description"] or "").strip()
            if len(desc) < 30 or re.fullmatch(rf"AgentGit: {re.escape(name)}", desc):
                bad(f"{where}: the description says nothing ({desc!r})")
            for field in ("license", "repository", "homepage", "documentation", "rust_version"):
                if not p.get(field):
                    bad(f"{where}: no {field}")
            if not p["categories"]:
                bad(f"{where}: no categories")
            if not p["keywords"] or len(p["keywords"]) > 5 or any(len(k) > 20 for k in p["keywords"]):
                bad(f"{where}: keywords must be 1-5 entries of at most 20 characters ({p['keywords']})")
            readme = p.get("readme")
            if not readme or not (manifest.parent / readme).is_file():
                bad(f"{where}: readme {readme!r} does not exist")
            for src in sorted(manifest.parent.glob("**/*.rs")):
                rel = src.relative_to(manifest.parent)
                if rel.parts[0] in ("tests", "target", "benches", "examples"):
                    continue
                for m in re.finditer(r'include_(?:str|bytes)!\(\s*"([^"]+)"', src.read_text()):
                    target = (src.parent / m.group(1)).resolve()
                    if not target.is_relative_to(manifest.parent.resolve()):
                        bad(f"{where}: {rel} includes {m.group(1)}, which lies outside the crate (see scripts/sync_crate_files.py)")
            for dep in p["dependencies"]:
                if dep["name"] in pkgs and dep["name"] not in publishable and dep["kind"] != "dev":
                    bad(f"{where} is publishable but depends on {dep['name']}, which is `publish = false`")
        else:
            text = manifest.read_text()
            m = re.search(r"((?:^#[^\n]*\n)+)publish = false", text, re.M)
            if not m or "Not publishable yet" not in m.group(1):
                bad(f"{where}: `publish = false` needs a comment above it saying why (\"# Not publishable yet: …\")")
    # Dependency order of the publishable crates. Dev-dependencies count: they keep their version in the published
    # manifest, so crates.io must already know them (there is no cycle: `cargo publish --workspace` would refuse one).
    order: list[str] = []
    seen: set[str] = set()

    def visit(n: str) -> None:
        if n in seen:
            return
        seen.add(n)
        for dep in sorted(d["name"] for d in pkgs[n]["dependencies"]):
            if dep in publishable:
                visit(dep)
        order.append(n)

    for n in sorted(publishable):
        visit(n)
    ok(f"{len(publishable)} publishable crates, {len(pkgs) - len(publishable)} marked `publish = false`")
    return order


def check_npm() -> str:
    pkg = json.loads((ROOT / "sdk/typescript/package.json").read_text())
    where = "npm package " + pkg.get("name", "?")
    for field in ("name", "version", "description", "license", "homepage", "keywords", "files", "engines", "exports"):
        if not pkg.get(field):
            bad(f"{where}: no {field}")
    if not str(pkg.get("repository", {}).get("url", "")).endswith("Paffin/AgentGit.git"):
        bad(f"{where}: repository.url is not the project's")
    if not pkg.get("bugs", {}).get("url"):
        bad(f"{where}: no bugs.url")
    if pkg.get("private"):
        bad(f"{where}: is private")
    for f in ("README.md", "LICENSE"):
        if not (ROOT / "sdk/typescript" / f).is_file():
            bad(f"{where}: {f} is missing")
    if pkg.get("version") != workspace_version():
        bad(f"{where}: version {pkg.get('version')} is not the workspace version {workspace_version()}")
    ok(f"{where} {pkg.get('version')}")
    return pkg["name"]


def check_pypi() -> str:
    proj = tomllib.loads((ROOT / "sdk/python/pyproject.toml").read_text())["project"]
    where = "PyPI package " + proj.get("name", "?")
    for field in ("name", "version", "description", "readme", "requires-python", "license", "keywords", "classifiers",
                  "urls", "dependencies"):
        if not proj.get(field):
            bad(f"{where}: no {field}")
    if "repository" not in {k.lower() for k in proj.get("urls", {})}:
        bad(f"{where}: no Repository URL")
    if proj.get("version") != workspace_version():
        bad(f"{where}: version {proj.get('version')} is not the workspace version {workspace_version()}")
    for f in ("README.md", "LICENSE"):
        if not (ROOT / "sdk/python" / f).is_file():
            bad(f"{where}: {f} is missing")
    ok(f"{where} {proj.get('version')}")
    return proj["name"]


def exists(url: str) -> bool:
    req = urllib.request.Request(url, headers={"User-Agent": "agentgit-release-check"})
    try:
        with urllib.request.urlopen(req, timeout=20):
            return True
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return False
        raise


def check_names_online(crates: list[str], npm: str, pypi: str) -> None:
    import time

    for c in crates:
        if exists(f"https://crates.io/api/v1/crates/{c}"):
            bad(f"crates.io already has a crate named {c} (not ours, or published before)")
        time.sleep(1)
    for n, url in (("npm " + npm, f"https://registry.npmjs.org/{npm.replace('/', '%2f')}"),
                   ("PyPI " + pypi, f"https://pypi.org/pypi/{pypi}/json")):
        if exists(url):
            ok(f"{n} exists already (fine if it is ours)")
        else:
            ok(f"{n} is free")


def main() -> None:
    order = check_crates(cargo_metadata())
    npm, pypi = check_npm(), check_pypi()
    if "--list" in sys.argv:
        print("publish order:", " ".join(order))
    if "--online" in sys.argv:
        check_names_online(order, npm, pypi)
    if failures:
        print(f"\n{len(failures)} problem(s)")
        sys.exit(1)
    print("\nall checks passed")


if __name__ == "__main__":
    main()
