#!/bin/sh
# Type-checks the workspace for x86_64-pc-windows-msvc from a Linux or macOS machine, in about a minute.
#
#   scripts/windows_typecheck.sh                      # cargo check --workspace --all-targets
#   scripts/windows_typecheck.sh -p agentgit-cli --tests    # anything `cargo check` takes
#
# It catches what `#[cfg(windows)]` / `#[cfg(unix)]` get wrong at compile time (a type that cannot be inferred
# because the code that fills it is cfg'd out, a Unix-only import in a test, an unused import on Windows) and nothing
# else: no code runs, so the Windows *runtime* is only ever verified by the Windows workflow. The C parts of the
# dependency graph (ring, aws-lc, SQLite) cannot be compiled for MSVC here, so their build scripts are given a
# stand-in compiler and archiver that create empty outputs; that is fine for `check`, never for `build`.
#
# Needs `rustup target add x86_64-pc-windows-msvc`. The zero-warnings bar of the Windows workflow applies.
set -eu

target=x86_64-pc-windows-msvc
if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
  echo "windows_typecheck.sh: run 'rustup target add $target' first" >&2
  exit 1
fi

shim_dir=$(mktemp -d)
trap 'rm -rf "$shim_dir"' EXIT
cat > "$shim_dir/cc" <<'SHIM'
#!/bin/sh
# Stand-in for cl.exe / lib.exe as the `cc` crate calls them: create the file that was asked for, succeed.
out=""
prev=""
for a in "$@"; do
  case "$prev" in -o) out="$a" ;; esac
  case "$a" in
    /Fo*|-Fo*) out="${a#?Fo}" ;;
    /[Oo][Uu][Tt]:*|-[Oo][Uu][Tt]:*) out="${a#?[Oo][Uu][Tt]:}" ;;
  esac
  prev="$a"
done
case "${1:-}" in
  --version|-v|-V) echo "clang version 18.0.0"; exit 0 ;;
esac
if [ -z "$out" ]; then # ar style: `ar cq libx.a obj.o` - the archive is the second plain argument
  n=0
  for a in "$@"; do
    case "$a" in -*|/*) ;; *) n=$((n + 1)); if [ "$n" = 2 ]; then out="$a"; break; fi ;; esac
  done
fi
if [ -n "$out" ]; then
  mkdir -p "$(dirname "$out")" 2>/dev/null || true
  : > "$out"
fi
exit 0
SHIM
chmod +x "$shim_dir/cc"

CC_x86_64_pc_windows_msvc="$shim_dir/cc"
AR_x86_64_pc_windows_msvc="$shim_dir/cc"
export CC_x86_64_pc_windows_msvc AR_x86_64_pc_windows_msvc

if [ $# -eq 0 ]; then
  set -- --workspace --all-targets
fi
cargo check --target "$target" --locked "$@"
