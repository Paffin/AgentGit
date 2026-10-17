#!/usr/bin/env bash
# Builds the Web UI (apps/web -> apps/web/dist) so that the next `cargo build -p ag-server` embeds it in the
# binary (cargo feature `ui`, on by default). Release and CI builds run this first.
#
#   scripts/build_ui.sh                         # npm ci + production build + pre-compression
#   AG_UI_SKIP_INSTALL=1 scripts/build_ui.sh    # reuse the existing node_modules (local iteration)
#   cargo build --release -p ag-server          # ... now contains the UI
#
# A server built without the UI still works: it serves a page that says how to build it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root/apps/web"

if ! command -v node >/dev/null 2>&1; then
  echo "build_ui.sh: node is not installed (the Web UI needs Node 22.18 or newer)" >&2
  exit 1
fi
node -e '
  const [maj, min] = process.versions.node.split(".").map(Number);
  if (maj < 22 || (maj === 22 && min < 18)) {
    console.error(`build_ui.sh: Node ${process.versions.node} is too old; the Web UI needs 22.18 or newer`);
    process.exit(1);
  }'

if [ -z "${AG_UI_SKIP_INSTALL:-}" ] || [ ! -d node_modules ]; then
  npm ci
fi
npm run build

test -f dist/index.html || { echo "build_ui.sh: the build produced no dist/index.html" >&2; exit 1; }

# Cargo cannot watch a directory that did not exist when the server was last built; touching the build script
# makes the next cargo build look at dist/ again.
touch "$root/crates/ag-server/build.rs"

files="$(find dist -type f ! -name '*.br' ! -name '*.gz' | wc -l | tr -d ' ')"
echo "Web UI built: apps/web/dist ($files files)."
echo "Next: cargo build --release -p ag-server   (the binary then serves the UI itself)"
