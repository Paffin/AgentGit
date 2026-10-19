#!/usr/bin/env bash
# Builds a compatibility fixture: a repository created by the CURRENT build of `ag`, exercising every kind of
# persisted state - history with clean and conflict-resolved merges and a rename-aware merge, special files
# (executable, symlink, binary, unicode and space in names), two policy versions, a signed CI attestation,
# agent identities, runs, a task, provenance, a lease, a second stream, a pin, and the hash-chained audit log.
#
# It writes, into the output directory:
#   repo-<label>.tar.gz         the repository (".ag/" and its working tree) as the tool left it
#   repo-<label>.expected.json  what a later version must still read from it: stream heads, changeset and tree
#                               ids, policy history, attestation ids, audit head, blame of one file, ...
#
# Fixtures are append-only. Generate a new one only when a persisted format changes on purpose (see
# docs/architecture/compatibility.md); never regenerate or edit an existing one - its whole value is that an
# OLD build wrote it.
#
#   tests/compat/generate.sh <label> [output-dir]       e.g.  tests/compat/generate.sh 0.1.0
#   AG=/path/to/ag tests/compat/generate.sh 0.1.0       use this binary (default: target/{release,debug}/ag)
set -euo pipefail

LABEL="${1:?usage: generate.sh <label> [output-dir]}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OUT="${2:-$HERE/fixtures}"
AG="${AG:-}"
if [ -z "$AG" ]; then
  if [ -x "$ROOT/target/release/ag" ]; then AG="$ROOT/target/release/ag"
  elif [ -x "$ROOT/target/debug/ag" ]; then AG="$ROOT/target/debug/ag"
  elif command -v ag >/dev/null 2>&1; then AG=ag
  else echo "ag not found: cargo build -p agentgit-cli, or set AG=" >&2; exit 1; fi
fi
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
REPO="$WORK/compat"
mkdir -p "$REPO"
cd "$REPO"

# Nothing from the invoking user's environment may leak into the fixture.
export HOME="$WORK/home" XDG_CONFIG_HOME="$WORK/home/.config" AG_KEY_DIR="" NO_COLOR=1
mkdir -p "$HOME"
unset AG_IDENTITY AG_KEY_DIR

as()    { local who=$1; shift; AG_IDENTITY="$who" "$AG" "$@"; }
run()   { local who=$1 want=$2; shift 2; set +e; as "$who" "$@" >"$WORK/last.out" 2>&1; local got=$?; set -e
          if [ "$got" -ne "$want" ]; then echo "ag $* (as $who): expected exit $want, got $got" >&2; cat "$WORK/last.out" >&2; exit 1; fi; }
quiet() { local who=$1; shift; as "$who" "$@"; }
field() { python3 -c "import sys,json; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }

# --- 1. the first commit: every kind of entry -----------------------------------------------------------------
run user:alice 0 init --namespace acme/compat
mkdir -p src docs bin data "docs/with space" unicode
cat > src/main.rs <<'RUST'
mod util;

fn main() {
    let name = "world";
    println!("hello, {name}");
    println!("{}", util::shout("done"));
}
RUST
cat > src/util.rs <<'RUST'
pub fn shout(s: &str) -> String {
    s.to_uppercase()
}

pub fn whisper(s: &str) -> String {
    s.to_lowercase()
}
RUST
printf '# compat\n\nA repository written by an old build.\n' > README.md
printf '# Guide\n\nline one\nline two\nline three\nline four\nline five\n' > docs/guide.md
printf 'inside a directory with a space\n' > "docs/with space/note.txt"
printf '#!/bin/sh\necho run\n' > bin/run.sh && chmod +x bin/run.sh
printf 'bin\000ary\377\376\n' > data/blob.bin
printf 'unicode name\n' > "unicode/日本語.txt"
ln -s src/main.rs main-link
run user:alice 0 snapshot -m "Initial import: every kind of entry"
P0=$(quiet user:alice --json propose -t "Initial import" | field 'd["id"]')
run user:alice 0 integrate "$P0"

# --- 2. policy version 1: CI must pass on the exact tree; agents leave provenance ------------------------------
KEY=$(quiet user:alice --json validate keygen ci | field 'd["public_key"]')
cat > "$WORK/policy1.json" <<JSON
{
  "trusted_issuers": [{ "identity": "service:ci", "public_key": "$KEY" }],
  "required_validations": [{ "check": "tests", "issuers": ["service:ci"] }],
  "approvals": { "min": 0, "min_for_agents": 1 },
  "agent_autonomy": { "auto_integrate": true, "allowed_paths": ["docs/**"], "max_files": 5 },
  "require_provenance_for_agents": true
}
JSON
run user:alice 0 policy set "$WORK/policy1.json"
ci() { run service:ci "${2:-0}" validate run "$1" --check tests --key ci -- sh -c 'test -f README.md && test -f src/main.rs'; }

# --- 3. an agent: identity, run, workspace, provenance, autonomous docs change ---------------------------------
run user:alice 0 agent register docs-bot
J=$(quiet user:alice --json agent start docs-bot)
DOCS=$(echo "$J" | field 'd["identity"]')
DOCS_RUN=$(echo "$J" | field 'd["run_id"]')
run "$DOCS" 0 workspace create docs --dir "$WORK/ws-docs"
printf '# Guide\n\nline one\nline two\nline three\nline four\nline five\n\n## Added by an agent\n' > "$WORK/ws-docs/docs/guide.md"
run "$DOCS" 0 snapshot -m "Document the guide" --workspace docs
PD=$(quiet "$DOCS" --json propose -t "Document the guide" @docs | field 'd["id"]')
run "$DOCS" 0 provenance attach @docs --agent docs-bot --model compat-model --rationale "the guide had no closing section" --check "make docs"
ci "$PD"
run "$DOCS" 0 integrate "$PD"
run "$DOCS" 0 agent finish "$DOCS_RUN"
run user:alice 0 workspace remove docs

# --- 4. tasks (the second waits for the first) -----------------------------------------------------------------
T1=$(quiet user:alice --json task create "write the changelog" | field 'd["id"]')
T2=$(quiet user:alice --json task create "publish the release" | field 'd["id"]')
run user:alice 0 task depend "$T2" "$T1"

# --- 5. a clean three-way merge: bob and alice edit different parts of one file --------------------------------
run user:bob 0 workspace create bob --dir "$WORK/ws-bob"
python3 - "$WORK/ws-bob/src/main.rs" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read().replace('let name = "world";', 'let name = "bob";')
open(p, "w").write(s)
PY
run user:bob 0 snapshot -m "Bob renames the greeting target" --workspace bob
PB=$(quiet user:bob --json propose -t "Bob renames the greeting target" @bob | field 'd["id"]')
python3 - src/main.rs <<'PY'
import sys
p = sys.argv[1]
s = open(p).read().replace('println!("{}", util::shout("done"));', 'println!("{}", util::shout("finished"));')
open(p, "w").write(s)
PY
run user:alice 0 snapshot -m "Alice changes the closing line"
PA=$(quiet user:alice --json propose -t "Alice changes the closing line" | field 'd["id"]')
ci "$PA"
run user:alice 0 integrate "$PA"
ci "$PB"
run user:alice 0 integrate "$PB"          # the second one is merged with the first: a two-parent changeset

# --- 6. a conflict, resolved in a workspace and landed ----------------------------------------------------------
run user:carol 0 workspace create carol --dir "$WORK/ws-carol"
printf '# compat\n\nA repository written by an old build.\n\nCarol was here.\n' > "$WORK/ws-carol/README.md"
run user:carol 0 snapshot -m "Carol edits the README" --workspace carol
PC=$(quiet user:carol --json propose -t "Carol edits the README" @carol | field 'd["id"]')
run user:alice 0 sync
printf '# compat\n\nA repository written by an old build.\n\nAlice was here.\n' > README.md
run user:alice 0 snapshot -m "Alice edits the README"
PR=$(quiet user:alice --json propose -t "Alice edits the README" | field 'd["id"]')
ci "$PR"
run user:alice 0 integrate "$PR"
run user:carol 4 preview "$PC"              # conflict, before anything happens
run user:carol 4 sync --workspace carol     # the markers are written for Carol
printf '# compat\n\nA repository written by an old build.\n\nAlice and Carol were here.\n' > "$WORK/ws-carol/README.md"
run user:carol 0 snapshot -m "Carol resolves the README" --workspace carol
run user:carol 0 proposal update "$PC" @carol
ci "$PC"
run user:alice 0 integrate "$PC"

# --- 7. a rename-aware merge: one side moves a file, the other edits it ------------------------------------------
run user:dave 0 workspace create dave --dir "$WORK/ws-dave"
mv "$WORK/ws-dave/src/util.rs" "$WORK/ws-dave/src/helpers.rs"
python3 - "$WORK/ws-dave/src/main.rs" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read().replace("mod util;", "mod helpers;").replace("util::shout", "helpers::shout")
open(p, "w").write(s)
PY
run user:dave 0 snapshot -m "Dave renames util.rs to helpers.rs" --workspace dave
PDV=$(quiet user:dave --json propose -t "Dave renames util.rs to helpers.rs" @dave | field 'd["id"]')
run user:alice 0 sync
python3 - src/util.rs <<'PY'
import sys
p = sys.argv[1]
s = open(p).read() + "\npub fn trim(s: &str) -> &str {\n    s.trim()\n}\n"
open(p, "w").write(s)
PY
run user:alice 0 snapshot -m "Alice adds trim() to util.rs"
PE=$(quiet user:alice --json propose -t "Alice adds trim() to util.rs" | field 'd["id"]')
ci "$PE"
run user:alice 0 integrate "$PE"
ci "$PDV"
run user:alice 0 integrate "$PDV"          # util.rs was renamed on one side and edited on the other
run user:dave 0 workspace remove dave
run user:bob 0 workspace remove bob
run user:carol 0 workspace remove carol

# --- 8. policy version 2, a second stream, a pin --------------------------------------------------------------
cat > "$WORK/policy2.json" <<JSON
{
  "trusted_issuers": [{ "identity": "service:ci", "public_key": "$KEY" }],
  "required_validations": [{ "check": "tests", "issuers": ["service:ci"], "max_age_secs": 86400 }],
  "approvals": { "min": 1, "min_for_agents": 2, "forbid_self_approval": true },
  "protected_paths": [{ "glob": "src/**", "min_approvals": 1 }],
  "agent_autonomy": { "auto_integrate": true, "allowed_paths": ["docs/**"], "max_files": 3 },
  "require_provenance_for_agents": true,
  "max_changed_files": 100
}
JSON
run user:alice 0 policy set "$WORK/policy2.json"
run user:alice 0 stream create feature
HEAD=$(quiet user:alice --json log main -n 1 | field 'd[0]["id"]')
run user:alice 0 pin v1-compat "$HEAD" --reason "the head the fixture was generated from"
# leases: an exclusive one with a fencing token, and a soft one
run user:alice 0 lease acquire 'data/**' --exclusive --ttl 24h
run user:bob 0 lease acquire 'docs/**' --ttl 24h

# --- 9. the repository must be healthy before it is frozen ----------------------------------------------------
run user:alice 0 verify
run user:alice 0 audit

# --- 10. what a later version must read back ---------------------------------------------------------------------
mkdir -p "$WORK/json"
quiet user:alice --json log main -n 200            > "$WORK/json/log.json"
quiet user:alice --json stream list                > "$WORK/json/streams.json"
quiet user:alice --json policy history             > "$WORK/json/policy_history.json"
quiet user:alice --json verify                     > "$WORK/json/verify.json"
quiet user:alice --json audit                      > "$WORK/json/audit.json"
quiet user:alice --json ls main                    > "$WORK/json/ls_root.json"
quiet user:alice --json show main                  > "$WORK/json/show_head.json"
quiet user:alice --json blame main:src/main.rs     > "$WORK/json/blame_main.json"
quiet user:alice --json blame main:src/helpers.rs  > "$WORK/json/blame_helpers.json"
quiet user:alice --json provenance show "$PD"      > "$WORK/json/provenance.json" || true
quiet user:alice --json agent runs                 > "$WORK/json/runs.json"
quiet user:alice --json task list                  > "$WORK/json/tasks.json"
quiet user:alice --json lease list                 > "$WORK/json/leases.json"
quiet user:alice --json pins                       > "$WORK/json/pins.json"
quiet user:alice --json audit --list -n 500        > "$WORK/json/audit_list.json"

AG_IDENTITY=user:alice python3 "$HERE/expected.py" "$WORK/json" "$AG" "$REPO" > "$WORK/expected.json"

TAR="$OUT/repo-$LABEL.tar.gz"
# The repository as the tool left it: the directory itself ("compat/"), reproducible member order.
( cd "$WORK" && tar --sort=name --owner=0 --group=0 --numeric-owner -czf "$TAR" compat )
cp "$WORK/expected.json" "$OUT/repo-$LABEL.expected.json"
echo "wrote $TAR ($(du -h "$TAR" | cut -f1)) and repo-$LABEL.expected.json"
