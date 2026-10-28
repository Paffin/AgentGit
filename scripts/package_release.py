#!/usr/bin/env python3
"""Builds the release archives and checksums. Python 3 standard library only, so it behaves the same on the
Linux and macOS runners (BSD tar has no --sort/--mtime).

    scripts/package_release.py archive --version 0.2.0 --target x86_64-unknown-linux-gnu \\
        --bin-dir target/x86_64-unknown-linux-gnu/release --out dist
    scripts/package_release.py sums dist/agentgit-*.tar.gz dist/*.cdx.json > dist/SHA256SUMS

An archive `agentgit-<version>-<target>.tar.gz` holds one directory `agentgit-<version>-<target>/` with `ag`,
`ag-server`, `ag-worker`, `ag-mcp`, `orchestrator`, LICENSE, NOTICE, THIRD_PARTY_LICENSES.md and README.md, plus `completions/` and `man/` when the files exist
(`--include-dir completions=PATH`). It is deterministic: entries sorted, owner 0:0, modes 0755/0644, a fixed
mtime (SOURCE_DATE_EPOCH, else the HEAD commit time) and no gzip timestamp, so the same inputs give the same
bytes.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import os
import pathlib
import subprocess
import sys
import tarfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
RUST_BINARIES = ["ag", "ag-server", "ag-worker", "ag-mcp"]
BINARIES = [*RUST_BINARIES, "orchestrator"]
DOCS = ["LICENSE", "NOTICE", "README.md", "THIRD_PARTY_LICENSES.md"]


def die(msg: str) -> None:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def source_date_epoch() -> int:
    env = os.environ.get("SOURCE_DATE_EPOCH")
    if env:
        return int(env)
    out = subprocess.run(["git", "log", "-1", "--format=%ct"], cwd=ROOT, capture_output=True, text=True)
    return int(out.stdout.strip()) if out.returncode == 0 and out.stdout.strip() else 0


def add(tar: tarfile.TarFile, arcname: str, data: bytes | None, mode: int, mtime: int, is_dir: bool = False) -> None:
    info = tarfile.TarInfo(arcname)
    info.mtime, info.uid, info.gid, info.uname, info.gname, info.mode = mtime, 0, 0, "", "", mode
    if is_dir:
        info.type = tarfile.DIRTYPE
        tar.addfile(info)
    else:
        assert data is not None
        info.size = len(data)
        tar.addfile(info, io.BytesIO(data))


def cmd_archive(args: argparse.Namespace) -> None:
    mtime = source_date_epoch()
    top = f"agentgit-{args.version}-{args.target}"
    bin_dir = pathlib.Path(args.bin_dir)
    entries: list[tuple[str, bytes, int]] = []  # (path inside the top dir, content, mode)
    for b in BINARIES:
        p = bin_dir / b
        if not p.is_file():
            die(f"{p} does not exist (build the Rust binaries and Go orchestrator; see docs/operations/releasing.md)")
        entries.append((b, p.read_bytes(), 0o755))
    for d in DOCS:
        entries.append((d, (ROOT / d).read_bytes(), 0o644))
    for spec in args.include_dir or []:
        name, _, path = spec.partition("=")
        src = pathlib.Path(path)
        if not src.is_dir():
            print(f"note: {src} does not exist; no {name}/ in the archive", file=sys.stderr)
            continue
        for f in sorted(p for p in src.rglob("*") if p.is_file()):
            entries.append((f"{name}/{f.relative_to(src).as_posix()}", f.read_bytes(), 0o644))
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    dest = out / f"{top}.tar.gz"
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.PAX_FORMAT) as tar:
        add(tar, top, None, 0o755, mtime, is_dir=True)
        seen_dirs: set[str] = set()
        for rel, data, mode in sorted(entries):
            parent = os.path.dirname(rel)
            if parent and parent not in seen_dirs:
                seen_dirs.add(parent)
                add(tar, f"{top}/{parent}", None, 0o755, mtime, is_dir=True)
            add(tar, f"{top}/{rel}", data, mode, mtime)
    with open(dest, "wb") as f, gzip.GzipFile(filename="", mode="wb", fileobj=f, mtime=0, compresslevel=9) as gz:
        gz.write(raw.getvalue())
    print(dest)


def cmd_sums(args: argparse.Namespace) -> None:
    names = set()
    for f in sorted(args.files):
        p = pathlib.Path(f)
        if p.name in names:
            die(f"two files named {p.name}")
        names.add(p.name)
        print(f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    a = sub.add_parser("archive", help="build one release archive")
    a.add_argument("--version", required=True)
    a.add_argument("--target", required=True)
    a.add_argument("--bin-dir", required=True)
    a.add_argument("--out", required=True)
    a.add_argument("--include-dir", action="append", metavar="NAME=PATH")
    a.set_defaults(fn=cmd_archive)
    s = sub.add_parser("sums", help="print sha256sum-style lines for the given files")
    s.add_argument("files", nargs="+")
    s.set_defaults(fn=cmd_sums)
    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
