#!/bin/sh
# Installs AgentGit (`ag`, `ag-server`, `ag-worker`, `ag-mcp`, `orchestrator`) from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/Paffin/AgentGit/main/scripts/install.sh | sh
#   sh install.sh --version 0.2.0 --prefix /opt/agentgit
#
# What it does: works out your platform, downloads the release archive and SHA256SUMS, verifies the archive
# against the checksum (and, when the GitHub CLI is installed and logged in, against its build-provenance
# attestation), and installs the binaries into <prefix>/bin without needing root. Nothing is installed if a
# check fails. Read it before piping it into a shell; it is short on purpose.
#
# Options:
#   --version X.Y.Z   the release to install (default: the latest non-prerelease)
#   --prefix DIR      install under DIR (default: $AG_PREFIX, else ~/.local); binaries go to DIR/bin
#   --dry-run         say what would happen, download and install nothing
#   --require-attestation   fail unless `gh attestation verify` ran and passed (needs `gh auth login`)
#   --no-attestation  never run `gh attestation verify`
#   --target TRIPLE   override platform detection (x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu,
#                     aarch64-apple-darwin, x86_64-apple-darwin)
#   --base-url URL    where the archive and SHA256SUMS are (default: the GitHub release for the version);
#                     a https:// or file:// URL or a plain directory — for mirrors and tests
#   -h, --help
#
# Environment: AG_PREFIX, AG_VERSION, GH_TOKEN (used by `gh`), NO_COLOR is ignored (there is no colour).
set -eu

REPO="Paffin/AgentGit"
BINARIES="ag ag-server ag-worker ag-mcp"

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
  if [ -f "$0" ]; then
    sed -n '2,/^set -eu/p' "$0" | sed -e '/^set -eu/d' -e 's/^# \{0,1\}//'
  else
    say "usage: install.sh [--version X.Y.Z] [--prefix DIR] [--dry-run] [--require-attestation] [--no-attestation]"
    say "       (the full option list is at the top of scripts/install.sh in the repository)"
  fi
}

version="${AG_VERSION:-latest}"
prefix="${AG_PREFIX:-}"
dry_run=0
attestation=auto # auto | require | never
target=""
base_url=""

while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || die "--version needs a value"; version=$2; shift 2 ;;
    --version=*) version=${1#--version=}; shift ;;
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix=$2; shift 2 ;;
    --prefix=*) prefix=${1#--prefix=}; shift ;;
    --target) [ $# -ge 2 ] || die "--target needs a value"; target=$2; shift 2 ;;
    --target=*) target=${1#--target=}; shift ;;
    --base-url) [ $# -ge 2 ] || die "--base-url needs a value"; base_url=$2; shift 2 ;;
    --base-url=*) base_url=${1#--base-url=}; shift ;;
    --dry-run) dry_run=1; shift ;;
    --require-attestation) attestation=require; shift ;;
    --no-attestation) attestation=never; shift ;;
    -h | --help) usage; exit 0 ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
done

# ---- platform -----------------------------------------------------------------------------------
detect_target() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$arch" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "unsupported CPU architecture: $arch (releases exist for x86_64 and aarch64)" ;;
  esac
  case "$os" in
    Linux)
      # The Linux binaries are linked against glibc. Say so instead of failing with 'not found' later.
      if command -v ldd >/dev/null 2>&1 && ldd --version 2>&1 | grep -qi musl; then
        die "this looks like a musl (Alpine) system; the release binaries need glibc. Use the container image ghcr.io/paffin/agentgit-cli, or build with 'cargo install --path crates/ag-cli'"
      fi
      echo "$arch-unknown-linux-gnu" ;;
    Darwin) echo "$arch-apple-darwin" ;;
    *) die "unsupported operating system: $os (releases exist for Linux and macOS; see docs/getting-started.md to build from source)" ;;
  esac
}
[ -n "$target" ] || target=$(detect_target)
case "$target" in
  x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu | aarch64-apple-darwin | x86_64-apple-darwin) ;;
  *) die "unsupported target: $target" ;;
esac

# ---- tools --------------------------------------------------------------------------------------
fetch() { # fetch SRC DEST  (SRC: https://, http://, file:// or a path)
  case "$1" in
    http://* | https://*)
      if command -v curl >/dev/null 2>&1; then curl -fsSL --retry 3 -o "$2" "$1"
      elif command -v wget >/dev/null 2>&1; then wget -q -O "$2" "$1"
      else die "need curl or wget to download $1"; fi ;;
    file://*) cp "${1#file://}" "$2" ;;
    *) cp "$1" "$2" ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1
  elif command -v openssl >/dev/null 2>&1; then openssl dgst -sha256 "$1" | sed 's/^.*= //'
  else die "need sha256sum, shasum or openssl to verify the download"; fi
}

# ---- which release ------------------------------------------------------------------------------
if [ "$version" = latest ]; then
  [ -z "$base_url" ] || die "--base-url needs an explicit --version"
  command -v curl >/dev/null 2>&1 || die "need curl to look up the latest release (or pass --version)"
  final=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") ||
    die "could not look up the latest release (pass --version X.Y.Z)"
  version=${final##*/}
  case "$version" in v[0-9]*) ;; *) die "no published release found at https://github.com/$REPO/releases (pass --version, or build from source)" ;; esac
fi
version=${version#v}
case "$version" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) die "'$version' is not a version like 1.2.3 or 1.2.3-rc.1" ;;
esac
case "$version" in *[!0-9A-Za-z.+-]*) die "'$version' contains unexpected characters" ;; esac

# Releases before beta.4 did not ship the native executor.
case "$version" in
  0.0.*|0.1.*|0.2.*|0.3.0-beta.[123]) ;;
  *) BINARIES="$BINARIES orchestrator" ;;
esac

name="agentgit-$version-$target"
archive="$name.tar.gz"
[ -n "$base_url" ] || base_url="https://github.com/$REPO/releases/download/v$version"
base_url=${base_url%/}

if [ -z "$prefix" ]; then
  [ -n "${HOME:-}" ] || die "HOME is not set; pass --prefix"
  prefix="$HOME/.local"
fi
bindir="$prefix/bin"

say "AgentGit $version for $target"
say "  from    $base_url/$archive"
say "  into    $bindir ($BINARIES)"
if [ "$dry_run" -eq 1 ]; then
  say "dry run: nothing was downloaded or installed"
  exit 0
fi

# ---- download and verify ----------------------------------------------------------------------------------
tmp=$(mktemp -d "${TMPDIR:-/tmp}/agentgit-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT INT TERM

fetch "$base_url/$archive" "$tmp/$archive" || die "could not download $base_url/$archive"
fetch "$base_url/SHA256SUMS" "$tmp/SHA256SUMS" || die "could not download $base_url/SHA256SUMS"

# SHA256SUMS lines look like '<64 hex>  <file>' (or '<hex> *<file>'); find ours exactly.
expected=$(awk -v f="$archive" '{ n = $2; sub(/^\*/, "", n); if (n == f) { print $1; exit } }' "$tmp/SHA256SUMS")
[ -n "$expected" ] || die "SHA256SUMS has no entry for $archive"
actual=$(sha256_of "$tmp/$archive")
if [ "$actual" != "$expected" ]; then
  die "checksum mismatch for $archive: SHA256SUMS says $expected, the download is $actual. Nothing was installed."
fi
say "checksum ok ($actual)"

if [ "$attestation" != never ]; then
  if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    gh attestation verify --help >/dev/null 2>&1 ||
      die "this GitHub CLI does not support attestation verification. Upgrade gh, or explicitly choose --no-attestation for checksum-only verification. Nothing was installed."
    if gh attestation verify "$tmp/$archive" --repo "$REPO" >"$tmp/attestation.log" 2>&1; then
      say "build-provenance attestation ok (gh attestation verify)"
    else
      cat "$tmp/attestation.log" >&2
      die "the build-provenance attestation of $archive did not verify. Nothing was installed."
    fi
  elif [ "$attestation" = require ]; then
    die "--require-attestation needs the GitHub CLI ('gh') installed and logged in ('gh auth login')"
  else
    say "note: the build-provenance attestation was not checked (needs 'gh', logged in); the checksum was"
  fi
fi

# ---- install ---------------------------------------------------------------------------------------------------
tar -xzf "$tmp/$archive" -C "$tmp" || die "could not unpack $archive"
[ -d "$tmp/$name" ] || die "the archive does not contain the expected directory $name/"
# Rehearsal archives can include the executor even when named with an older version.
case " $BINARIES " in
  *" orchestrator "*) ;;
  *) if [ -f "$tmp/$name/orchestrator" ]; then BINARIES="$BINARIES orchestrator"; fi ;;
esac
# Validate the whole distribution before touching an existing installation.
for b in $BINARIES; do
  [ -f "$tmp/$name/$b" ] || die "the archive is missing $b. Nothing was installed."
done
mkdir -p "$bindir" || die "cannot create $bindir (choose a directory you own with --prefix)"
installed=""
for b in $BINARIES; do
  # Copy beside the destination first, then rename: replacing a running binary must not corrupt it.
  if ! cp "$tmp/$name/$b" "$bindir/.$b.new.$$"; then die "cannot write $bindir/$b"; fi
  chmod 0755 "$bindir/.$b.new.$$"
  mv -f "$bindir/.$b.new.$$" "$bindir/$b"
  installed="$installed $b"
done
[ -n "$installed" ] || die "the archive contains none of: $BINARIES"
docdir="$prefix/share/doc/agentgit"
if mkdir -p "$docdir" 2>/dev/null; then
  for f in LICENSE NOTICE README.md THIRD_PARTY_LICENSES.md; do [ -f "$tmp/$name/$f" ] && cp "$tmp/$name/$f" "$docdir/$f"; done
fi

say "installed:$installed"
"$bindir/ag" --version 2>/dev/null || warn "installed, but '$bindir/ag --version' did not run on this machine"
case ":${PATH:-}:" in
  *":$bindir:"*) ;;
  *) say "note: $bindir is not on your PATH; add it, e.g.  export PATH=\"$bindir:\$PATH\"" ;;
esac
