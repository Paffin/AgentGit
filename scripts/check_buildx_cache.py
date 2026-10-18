#!/usr/bin/env python3
"""Reads a `docker buildx build --progress=plain` log and checks that the expensive steps came from the cache.

    scripts/check_buildx_cache.py build.log                       # the default steps below
    scripts/check_buildx_cache.py build.log 'cargo build' 'apt-get'

Without arguments it requires every `RUN` step that installs cargo-chef, cooks the dependencies, installs and
builds the Web UI (`npm ci`, scripts/build_ui.sh) or compiles AgentGit (deploy/docker/Dockerfile) to be `CACHED`,
and at least one such step per needle to exist: a log that has none (a changed Dockerfile, a renamed step) must
fail rather than pass vacuously.
"""
import re
import sys

DEFAULT = ["cargo install cargo-chef", "cargo chef cook", "cargo build --release", "npm ci", "scripts/build_ui.sh"]


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    log = open(sys.argv[1], errors="replace").read()
    wanted = sys.argv[2:] or DEFAULT
    # `#12 [build 4/7] RUN --mount=... cargo chef cook ...` ... later `#12 CACHED`
    steps: dict[str, str] = {}
    for m in re.finditer(r"^#(\d+) (\[[^\]]+\] .*)$", log, re.M):
        steps.setdefault(m.group(1), m.group(2))
    cached = set(re.findall(r"^#(\d+) CACHED", log, re.M))
    bad = []
    seen = 0
    for needle in wanted:
        hits = [(n, text) for n, text in steps.items() if text.split("] ", 1)[1].startswith("RUN") and needle in text]
        if not hits:
            bad.append(f"no RUN step containing '{needle}' in the log")
            continue
        for n, text in hits:
            seen += 1
            state = "CACHED" if n in cached else "BUILT (not cached)"
            print(f"{state:20} #{n} {text[:110]}")
            if n not in cached:
                bad.append(f"step #{n} '{needle}' was rebuilt instead of restored from the cache")
    if bad:
        print("\n".join(f"FAIL: {b}" for b in bad), file=sys.stderr)
        return 1
    print(f"ok: all {seen} expensive step(s) were restored from the cache")
    return 0


sys.exit(main())
