#!/usr/bin/env bash
# AgentGit end-to-end demo: a human, two agents and a CI service share one stream.
#
#   ./examples/quickstart.sh            # uses `ag` from PATH, or target/{release,debug}/ag
#   AG=/path/to/ag ./examples/quickstart.sh
#
# Every step asserts the exit code it expects, so CI runs this script as a test:
# the README transcript cannot drift away from what the tool really does.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AG="${AG:-}"
if [ -z "$AG" ]; then
  if command -v ag >/dev/null 2>&1; then AG=ag
  elif [ -x "$HERE/target/release/ag" ]; then AG="$HERE/target/release/ag"
  elif [ -x "$HERE/target/debug/ag" ]; then AG="$HERE/target/debug/ag"
  else echo "ag not found: run 'cargo build --release' first" >&2; exit 1; fi
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
REPO="$WORK/webapp"
mkdir -p "$REPO"
cd "$REPO"

section() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
say()     { printf '\033[2m$ %s\033[0m\n' "$*"; }
# as <identity> <args...>: run ag as someone
as()      { local who=$1; shift; AG_IDENTITY="$who" "$AG" "$@"; }
# run <identity> <expected-exit-code> <args...>: echo the command, run it, assert the exit code
run() {
  local who=$1 want=$2; shift 2
  say "ag $*   # as $who"
  set +e; as "$who" "$@"; local got=$?; set -e
  if [ "$got" -ne "$want" ]; then echo "expected exit code $want, got $got" >&2; exit 1; fi
}
# quiet <identity> <args...>: run without echo, print stdout (for capturing)
quiet()   { local who=$1; shift; as "$who" "$@"; }
field()   { python3 -c "import sys,json; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }

section "1. A repository, and its first commit"
run user:alice 0 init --namespace acme/webapp
mkdir -p src docs
cat > src/main.rs <<'RUST'
fn greeting() -> &'static str {
    "hello"
}

fn main() {
    println!("{}", greeting());
}
RUST
printf '# webapp\n\nA small demo.\n' > docs/README.md
run user:alice 0 snapshot -m "Initial import"
P0=$(quiet user:alice --json propose -t "Initial import" | field 'd["id"]')
run user:alice 0 integrate "$P0"

section "2. Policy: CI must pass on the exact tree; agents leave provenance; humans approve risky changes"
KEY=$(quiet user:alice --json validate keygen ci | field 'd["public_key"]')
cat > "$WORK/policy.json" <<JSON
{
  "trusted_issuers": [{ "identity": "service:ci", "public_key": "$KEY" }],
  "required_validations": [{ "check": "tests", "issuers": ["service:ci"] }],
  "approvals": { "min": 0, "min_for_agents": 1 },
  "agent_autonomy": { "auto_integrate": true, "allowed_paths": ["docs/**"], "max_files": 5 },
  "require_provenance_for_agents": true
}
JSON
run user:alice 0 policy set "$WORK/policy.json"
ci() { # the CI service validates the exact candidate tree of a proposal
  run service:ci "${2:-0}" validate run "$1" --check tests --key ci -- sh -c 'test -f docs/README.md && test -f src/main.rs'
}

section "3. Two agents get identities and start runs"
run user:alice 0 agent register docs-bot
run user:alice 0 agent register refactor-bot
J=$(quiet user:alice --json agent start docs-bot);     DOCS=$(echo "$J" | field 'd["identity"]')
J=$(quiet user:alice --json agent start refactor-bot); REFACTOR=$(echo "$J" | field 'd["identity"]'); REFACTOR_RUN=$(echo "$J" | field 'd["run_id"]')
echo "docs-bot acts as     $DOCS"
echo "refactor-bot acts as $REFACTOR"
run "$DOCS"     0 workspace create docs     --dir "$WORK/ws-docs"
run "$REFACTOR" 0 workspace create refactor --dir "$WORK/ws-refactor"

section "4. docs-bot edits documentation: inside its autonomy envelope, no human needed"
printf '# webapp\n\nA small demo.\n\n## Usage\n\nRun it.\n' > "$WORK/ws-docs/docs/README.md"
run "$DOCS" 0 snapshot -m "Document usage" --workspace docs
PD=$(quiet "$DOCS" --json propose -t "Document usage" @docs | field 'd["id"]')
run "$DOCS" 0 provenance attach @docs --agent docs-bot --model demo-model --rationale "usage section was missing"
ci "$PD"
run "$DOCS" 0 integrate "$PD"

section "5. refactor-bot changes code: blocked until CI, provenance, a human and its lease allow it"
run "$REFACTOR" 0 lease acquire 'src/**' --exclusive --ttl 30m
TOKEN=$(quiet "$REFACTOR" --json lease list | field 'd[0]["token"]')
cat > "$WORK/ws-refactor/src/main.rs" <<'RUST'
fn greeting() -> &'static str {
    "hello, world"
}

fn main() {
    println!("{}", greeting());
}
RUST
run "$REFACTOR" 0 snapshot -m "Friendlier greeting" --workspace refactor
PR=$(quiet "$REFACTOR" --json propose -t "Friendlier greeting" @refactor | field 'd["id"]')
run "$REFACTOR" 4 integrate "$PR"            # agents may not integrate code on their own
run user:alice  3 integrate "$PR"            # blocked: no CI, no provenance, no approval, no token
ci "$PR"
run "$REFACTOR" 0 provenance attach @refactor --agent refactor-bot --model demo-model --check "cargo test"
run user:bob    0 proposal review "$PR" --approve -m "reviewed, looks good"
run user:alice  0 integrate "$PR" --fencing-token "$TOKEN"
run "$REFACTOR" 0 agent finish "$REFACTOR_RUN"   # finishing a run releases its leases

section "6. A concurrent edit conflicts; the stream is never touched; the conflict is resolved and lands"
run user:bob 0 workspace create bob --dir "$WORK/ws-bob"   # starts from the *new* head, so edit and race from there
cat > "$WORK/ws-bob/src/main.rs" <<'RUST'
fn greeting() -> &'static str {
    "hello, bob"
}

fn main() {
    println!("{}", greeting());
}
RUST
run user:bob 0 snapshot -m "Greet bob" --workspace bob
PB=$(quiet user:bob --json propose -t "Greet bob" @bob | field 'd["id"]')
# meanwhile alice syncs to the new head and lands another edit to the same line
run user:alice 0 sync
cat > src/main.rs <<'RUST'
fn greeting() -> &'static str {
    "hello, alice"
}

fn main() {
    println!("{}", greeting());
}
RUST
run user:alice 0 snapshot -m "Greet alice"
PA=$(quiet user:alice --json propose -t "Greet alice" | field 'd["id"]')
ci "$PA"
run user:alice 0 integrate "$PA"
run user:bob 4 preview "$PB"                 # conflict: reported before anything happens
run user:bob 4 sync --workspace bob          # conflict markers are written for bob to resolve
cat > "$WORK/ws-bob/src/main.rs" <<'RUST'
fn greeting() -> &'static str {
    "hello, alice and bob"
}

fn main() {
    println!("{}", greeting());
}
RUST
run user:bob 0 snapshot -m "Greet both" --workspace bob
run user:bob 0 proposal update "$PB" @bob
ci "$PB"
run user:alice 0 integrate "$PB"

section "7. Everything is auditable and verifiable"
run user:alice 0 log main -n 8
run user:alice 0 trace "$REFACTOR_RUN" -n 12
run user:alice 0 verify
run user:alice 0 audit
printf '\nDemo finished: all steps behaved as expected.\n'
