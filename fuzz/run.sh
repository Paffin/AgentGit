#!/usr/bin/env bash
# Runs fuzz targets for a bounded time. Used by CI (.github/workflows/fuzz.yml) and by hand.
#
#   fuzz/run.sh <seconds> [target ...]      run the named targets (default: every one that is built/buildable)
#
# Environment:
#   FUZZ_FEATURES   cargo features to build with (default: heavy)
#   FUZZ_BIN_DIR    run already-built binaries from this directory instead of calling `cargo fuzz`
#                   (the directory `cargo fuzz build` wrote them to, e.g. target/x86_64-unknown-linux-gnu/release)
#   FUZZ_SANITIZER  address (default), none, ... passed to `cargo fuzz -s`
#   FUZZ_CORPUS     where the working corpora live (default: fuzz/corpus, seeds checked in)
#
# A crash, hang (> 20 s on one input) or out-of-memory (> 2 GB) stops the target, leaves the reproducer in
# fuzz/artifacts/<target>/ and makes this script exit 1 after the remaining targets have run.
set -u
cd "$(dirname "$0")"
secs="${1:?usage: fuzz/run.sh <seconds> [target ...]}"; shift
features="${FUZZ_FEATURES:-heavy}"
sanitizer="${FUZZ_SANITIZER:-address}"
corpus_root="${FUZZ_CORPUS:-corpus}"

# Longest input worth trying per target (libFuzzer's default of 4 KiB is too small for text merges and the
# generators need only a few hundred bytes). Everything else uses 4096.
declare -A MAXLEN=(
  [diff_myers]=16384 [merge_text]=16384 [policy_json]=8192 [signed_json]=4096
  [merge_trees]=1024 [tree_ops]=1024 [rename_detect]=1024 [policy_eval]=512
  [agwp_json]=8192 [agwp_proto]=8192 [job_spec]=2048 [sandbox_spec]=4096
)
declare -A DICT=(
  [policy_json]=dictionaries/policy.dict [policy_eval]=
  [signed_json]=dictionaries/signed.dict [sandbox_spec]=dictionaries/sandbox.dict
  [agwp_json]=dictionaries/json.dict [git_parse]=dictionaries/git.dict
  [glob_match]=dictionaries/glob.dict [ignore_rules]=dictionaries/glob.dict
)

if [ $# -eq 0 ]; then
  set -- $(ls fuzz_targets | sed 's/\.rs$//')
fi

failed=0
for t in "$@"; do
  mkdir -p "$corpus_root/$t" "artifacts/$t"
  args=(-max_total_time="$secs" -timeout=20 -rss_limit_mb=2048 -max_len="${MAXLEN[$t]:-4096}" -print_final_stats=1)
  if [ -n "${DICT[$t]:-}" ] && [ -f "${DICT[$t]}" ]; then args+=(-dict="${DICT[$t]}"); fi
  echo "::group::fuzz $t (${secs}s, max_len ${MAXLEN[$t]:-4096})"
  if [ -n "${FUZZ_BIN_DIR:-}" ]; then
    "$FUZZ_BIN_DIR/$t" "${args[@]}" -artifact_prefix="artifacts/$t/" "$corpus_root/$t"
  else
    cargo +nightly fuzz run -s "$sanitizer" --features "$features" "$t" "$corpus_root/$t" -- "${args[@]}" \
      -artifact_prefix="$PWD/artifacts/$t/"
  fi
  rc=$?
  echo "::endgroup::"
  if [ $rc -ne 0 ]; then
    echo "::error title=fuzz target $t failed::see fuzz/artifacts/$t/ (reproduce: cargo +nightly fuzz run $t fuzz/artifacts/$t/<file>)"
    failed=1
  fi
done
exit $failed
