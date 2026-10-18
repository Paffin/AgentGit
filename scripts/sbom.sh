#!/usr/bin/env bash
# CycloneDX 1.5 software bills of materials: one per shipped Rust binary and one for the web UI bundle.
#
#   scripts/sbom.sh [--version V] [--out DIR] [--no-web]
#
# Used by the release workflow (the SBOMs are release assets and are attested) and by the Security workflow, which
# runs it on every change so that the release job never meets a surprise. Output, in DIR (default dist/):
#
#   agentgit-V-ag.cdx.json  agentgit-V-ag-server.cdx.json  agentgit-V-ag-worker.cdx.json   (cargo-cyclonedx)
#   agentgit-V-web-ui.cdx.json                                                            (scripts/sbom_npm.py)
#
# Rust SBOMs list the dependencies of Cargo.lock for the binary, build-time dependencies included. The web UI one
# lists the production dependencies of apps/web/package-lock.json (scripts/sbom_npm.py: `npm sbom --omit dev`
# drops two thirds of them). The orchestrator image's Go module list is in THIRD_PARTY_LICENSES.md; it is not a
# CycloneDX document here.
#
# Needs: cargo-cyclonedx 0.5.9 (cargo install cargo-cyclonedx --version 0.5.9 --locked) and python3.
set -euo pipefail
cd "$(dirname "$0")/.."

version=0.0.0-dev
out=dist
web=1
while [ $# -gt 0 ]; do
  case "$1" in
    --version) version=${2:?}; shift 2 ;;
    --out) out=${2:?}; shift 2 ;;
    --no-web) web=0; shift ;;
    *) echo "usage: $0 [--version V] [--out DIR] [--no-web]" >&2; exit 2 ;;
  esac
done

command -v cargo-cyclonedx >/dev/null || cargo cyclonedx --version >/dev/null 2>&1 \
  || { echo "cargo-cyclonedx is not installed: cargo install cargo-cyclonedx --version 0.5.9 --locked" >&2; exit 2; }

mkdir -p "$out"
cargo cyclonedx --format json --spec-version 1.5 --describe binaries --target all
for pair in ag-cli:ag ag-server:ag-server ag-worker:ag-worker; do
  crate=${pair%%:*}; bin=${pair##*:}
  # a move, not a copy: the tool writes next to Cargo.toml and the working tree must stay clean
  mv "crates/$crate/${bin}_bin.cdx.json" "$out/agentgit-${version}-${bin}.cdx.json"
done
# any other binary SBOM the tool wrote is not a release asset
find crates -maxdepth 2 -name '*.cdx.json' -delete

if [ "$web" = 1 ]; then
  python3 scripts/sbom_npm.py apps/web > "$out/agentgit-${version}-web-ui.cdx.json"
fi

python3 - "$out" "$version" "$web" <<'PY'
import glob, json, sys
out, version, web = sys.argv[1], sys.argv[2], sys.argv[3] == "1"
want = 4 if web else 3
files = sorted(glob.glob(f"{out}/agentgit-{version}-*.cdx.json"))
assert len(files) == want, f"expected {want} SBOMs, found {files}"
for f in files:
    d = json.load(open(f))
    assert d["bomFormat"] == "CycloneDX", f
    names = {c["name"] for c in d.get("components", [])}
    if f.endswith("-web-ui.cdx.json"):
        assert d["specVersion"] == "1.5", f
        assert len(names) > 30 and {"vue", "pinia", "vue-router"} <= names, f
    else:
        assert d["specVersion"] == "1.5", f
        assert len(names) > 50 and "ag-types" in names and "tokio" in names, f
    print(f, len(names), "components")
PY
