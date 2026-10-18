#!/usr/bin/env bash
# Generates THIRD_PARTY_LICENSES.md: the licence texts of everything that ends up in a binary, a bundle or an
# image we distribute.
#
#   scripts/third_party_licenses.sh            rewrite THIRD_PARTY_LICENSES.md
#   scripts/third_party_licenses.sh --check    fail (with a diff) when the committed file is out of date
#
# Sections:
#   Rust          cargo-about over Cargo.lock, for the release targets, without dev- and build-dependencies
#                 (about.toml, about.hbs). The licences must be those of deny.toml's allow list.
#   Web UI        production dependencies of apps/web (embedded in ag-server), from package-lock.json and the
#                 installed node_modules - `npm ci` runs here when node_modules is missing.
#   Orchestrator  the Go modules and the standard library linked into services/agent-orchestrator.
# Not covered: operating-system layers of the container images (Debian, distroless), which carry their own
# notices under /usr/share/doc, and the SDKs, which are libraries whose dependencies pip and npm resolve for the user.
#
# Needs: cargo-about (cargo install cargo-about --locked --version 0.9.2), python3 >= 3.11, node/npm, go.
set -euo pipefail
cd "$(dirname "$0")/.."

out=THIRD_PARTY_LICENSES.md
mode=write
case "${1:-}" in
  "") ;;
  --check) mode=check ;;
  *) echo "usage: $0 [--check]" >&2; exit 2 ;;
esac

command -v cargo-about >/dev/null || { echo "cargo-about is not installed: cargo install cargo-about --locked --version 0.9.2" >&2; exit 2; }

# Whatever about.toml accepts, deny.toml (the gate) must allow too. deny.toml may allow more: the fuzz harness's
# licences are checked there, but no fuzz dependency is shipped.
python3 - <<'PY'
import sys, tomllib
about = set(tomllib.load(open("about.toml", "rb"))["accepted"])
deny = set(tomllib.load(open("deny.toml", "rb"))["licenses"]["allow"])
if about - deny:
    print("about.toml accepts licences that deny.toml does not allow:", sorted(about - deny), file=sys.stderr)
    sys.exit(1)
PY

if [ ! -d apps/web/node_modules ]; then
  (cd apps/web && npm ci --ignore-scripts --no-audit --no-fund >&2)
fi

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
{
  cat <<'HEAD'
# Third-party licences

AgentGit itself is Apache-2.0 (see `LICENSE` and `NOTICE`). The binaries, the web UI bundle and the orchestrator
image we distribute contain the open-source components listed below, each under the licence shown. The texts are
generated from the lock files by `scripts/third_party_licenses.sh`; CI fails when this file is out of date
(`.github/workflows/security.yml`, job "hygiene"). `deny.toml` is the policy that keeps copyleft licences out.

HEAD
  # some licence files use CRLF; the committed file is LF so that git leaves it alone and `--check` stays exact
  cargo-about generate --locked --workspace -c about.toml about.hbs | tr -d '\r'
  echo
  python3 scripts/third_party_licenses.py npm
  echo
  python3 scripts/third_party_licenses.py go
} > "$tmp"

if [ "$mode" = check ]; then
  if ! cmp -s "$tmp" "$out"; then
    diff -u "$out" "$tmp" | head -60 || true
    echo "$out is out of date: run scripts/third_party_licenses.sh and commit the result" >&2
    exit 1
  fi
  echo "$out is up to date"
else
  chmod 644 "$tmp"
  mv "$tmp" "$out"
  trap - EXIT
  echo "wrote $out ($(wc -c < "$out") bytes)"
fi
