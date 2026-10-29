#!/usr/bin/env python3
"""Builds the release archives and checksums. Python 3 standard library only, so it behaves the same on the
Linux, macOS and Windows runners (BSD tar has no --sort/--mtime).

    scripts/package_release.py archive --version 0.2.0 --target x86_64-unknown-linux-gnu \\
        --bin-dir target/x86_64-unknown-linux-gnu/release --out dist
    scripts/package_release.py sums dist/agentgit-*.tar.gz dist/*.cdx.json > dist/SHA256SUMS

An archive `agentgit-<version>-<target>.tar.gz` holds one directory `agentgit-<version>-<target>/` with `ag`,
`ag-server`, `ag-worker`, `ag-mcp`, `orchestrator`, LICENSE, NOTICE, THIRD_PARTY_LICENSES.md and README.md, plus `completions/` and `man/` when the files exist
(`--include-dir completions=PATH`). It is deterministic: entries sorted, owner 0:0, modes 0755/0644, a fixed
mtime (SOURCE_DATE_EPOCH, else the HEAD commit time) and no gzip timestamp, so the same inputs give the same
bytes. Windows x86_64 archives use .zip and contain the supported ag.exe, ag-server.exe and ag-mcp.exe;
the Linux-only worker and unqualified Windows executor are not included.
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
import time
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
RUST_BINARIES = ["ag", "ag-server", "ag-worker", "ag-mcp"]
BINARIES = [*RUST_BINARIES, "orchestrator"]
WINDOWS_TARGET = "x86_64-pc-windows-msvc"
WINDOWS_BINARIES = ["ag.exe", "ag-server.exe", "ag-mcp.exe"]
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


def archive_entries(args: argparse.Namespace) -> list[tuple[str, bytes, int]]:
    bin_dir = pathlib.Path(args.bin_dir)
    entries: list[tuple[str, bytes, int]] = []
    binaries = WINDOWS_BINARIES if args.target == WINDOWS_TARGET else BINARIES
    for binary in binaries:
        path = bin_dir / binary
        if not path.is_file():
            die(f"{path} does not exist (build the binaries; see docs/operations/releasing.md)")
        entries.append((binary, path.read_bytes(), 0o755))
    for doc in DOCS:
        entries.append((doc, (ROOT / doc).read_bytes(), 0o644))
    entries.extend(included_entries(args.include_dir or []))
    return sorted(entries)


def included_entries(specs: list[str]) -> list[tuple[str, bytes, int]]:
    entries: list[tuple[str, bytes, int]] = []
    for spec in specs:
        name, _, path = spec.partition("=")
        src = pathlib.Path(path)
        if not src.is_dir():
            print(f"note: {src} does not exist; no {name}/ in the archive", file=sys.stderr)
            continue
        for file in sorted(p for p in src.rglob("*") if p.is_file()):
            entries.append((f"{name}/{file.relative_to(src).as_posix()}", file.read_bytes(), 0o644))
    return entries


def write_tar(dest: pathlib.Path, top: str, entries: list[tuple[str, bytes, int]], mtime: int) -> None:
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.PAX_FORMAT) as tar:
        add(tar, top, None, 0o755, mtime, is_dir=True)
        seen_dirs: set[str] = set()
        for rel, data, mode in entries:
            parent = os.path.dirname(rel)
            if parent and parent not in seen_dirs:
                seen_dirs.add(parent)
                add(tar, f"{top}/{parent}", None, 0o755, mtime, is_dir=True)
            add(tar, f"{top}/{rel}", data, mode, mtime)
    with open(dest, "wb") as file, gzip.GzipFile(filename="", mode="wb", fileobj=file, mtime=0, compresslevel=9) as gz:
        gz.write(raw.getvalue())


def write_zip(dest: pathlib.Path, top: str, entries: list[tuple[str, bytes, int]], mtime: int) -> None:
    # ZIP timestamps have two-second precision and cover 1980 through 2107.
    timestamp = time.gmtime(min(max(mtime, 315532800), 4354819198))[:6]
    with zipfile.ZipFile(dest, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for rel, data, mode in entries:
            info = zipfile.ZipInfo(f"{top}/{rel}", timestamp)
            info.create_system = 3
            info.external_attr = mode << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, data, compresslevel=9)


def cmd_archive(args: argparse.Namespace) -> None:
    entries = archive_entries(args)
    top = f"agentgit-{args.version}-{args.target}"
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    windows = args.target == WINDOWS_TARGET
    dest = out / (top + (".zip" if windows else ".tar.gz"))
    writer = write_zip if windows else write_tar
    writer(dest, top, entries, source_date_epoch())
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
