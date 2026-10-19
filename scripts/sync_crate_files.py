#!/usr/bin/env python3
"""Keeps the files that a crate needs from outside its own directory inside it, so `cargo package` works.

    python3 scripts/sync_crate_files.py           # copy the top-level files into the crates
    python3 scripts/sync_crate_files.py --check   # fail when a copy differs, is missing, or is left over
    python3 scripts/sync_crate_files.py --ui      # copy the built Web UI (apps/web/dist) into crates/ag-ui-assets/dist

The top-level directories stay the single source of truth (docs, the Dockerfile and the SDK generators read them);
the crates build from their copies. Cargo only packages files inside a crate, and symlinks do not survive packaging.

    proto/agwp                -> crates/ag-protocol/proto/agwp       (build.rs compiles it)
    migrations/{sqlite,postgres} -> crates/ag-storage/migrations     (include_str!)
    migrations/control        -> crates/ag-server/migrations/control (include_str!)

`--ui` is a release step, not a source copy: `crates/ag-ui-assets/dist` is git-ignored, and the Packaging workflow
and the crates.io release run it after `scripts/build_ui.sh`, so that the published `agentgit-ui-assets` carries the UI.

Python 3.9+ standard library only.
"""
import filecmp
import pathlib
import shutil
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

COPIES = [
    ("proto/agwp", "crates/ag-protocol/proto/agwp"),
    ("migrations/sqlite", "crates/ag-storage/migrations/sqlite"),
    ("migrations/postgres", "crates/ag-storage/migrations/postgres"),
    ("migrations/control", "crates/ag-server/migrations/control"),
]


def files(base: pathlib.Path) -> dict:
    return {str(p.relative_to(base)): p for p in sorted(base.rglob("*")) if p.is_file()}


def check() -> int:
    bad = 0
    for src, dst in COPIES:
        a, b = files(ROOT / src), files(ROOT / dst)
        for rel in sorted(set(a) | set(b)):
            if rel not in b:
                print(f"missing   {dst}/{rel}  (copy of {src}/{rel})")
            elif rel not in a:
                print(f"left over {dst}/{rel}  (no {src}/{rel})")
            elif not filecmp.cmp(a[rel], b[rel], shallow=False):
                print(f"differs   {dst}/{rel}  from {src}/{rel}")
            else:
                continue
            bad += 1
    if bad:
        print(f"\n{bad} file(s) out of sync; run: python3 scripts/sync_crate_files.py")
        return 1
    print(f"ok: {sum(len(files(ROOT / s)) for s, _ in COPIES)} files in {len(COPIES)} directories are identical to their crate copies")
    return 0


def sync() -> int:
    for src, dst in COPIES:
        target = ROOT / dst
        if target.exists():
            shutil.rmtree(target)
        shutil.copytree(ROOT / src, target)
    return check()


def ui() -> int:
    dist = ROOT / "apps/web/dist"
    if not (dist / "index.html").is_file():
        print("apps/web/dist/index.html does not exist: run scripts/build_ui.sh first", file=sys.stderr)
        return 1
    target = ROOT / "crates/ag-ui-assets/dist"
    if target.exists():
        shutil.rmtree(target)
    shutil.copytree(dist, target)
    print(f"copied {len(files(target))} files into crates/ag-ui-assets/dist")
    return 0


if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else ""
    if mode == "--check":
        sys.exit(check())
    if mode == "--ui":
        sys.exit(ui())
    if mode:
        sys.exit(__doc__)
    sys.exit(sync())
