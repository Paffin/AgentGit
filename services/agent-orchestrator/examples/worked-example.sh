#!/bin/sh
# The worked example of docs/guides/orchestrator.md, runnable end to end.
#
#   go build -o /tmp/orchestrator ./cmd/orchestrator          (from services/agent-orchestrator)
#   ORCHESTRATOR_BIN=/tmp/orchestrator AG_SERVER_BIN=/path/to/ag-server sh examples/worked-example.sh
#
# It starts a throw-away ag-server (127.0.0.1:18090) in a temporary directory, sets up a repository, people and
# service identities with `ag-server admin`, runs the orchestrator (admin API on 127.0.0.1:19090), plays the part
# of a developer, a reviewer and an integrator with curl, and stops everything again. Needs sh, curl, base64,
# python3 (for reading JSON). Set WORKDIR to keep the files, otherwise a temporary directory is used and removed.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ORCH=${ORCHESTRATOR_BIN:?set ORCHESTRATOR_BIN to the orchestrator binary}
SRV=${AG_SERVER_BIN:-ag-server}
if [ -n "${WORKDIR:-}" ]; then W=$WORKDIR; mkdir -p "$W"; KEEP=1; else W=$(mktemp -d); KEEP=0; fi
URL=http://127.0.0.1:18090
ADMIN=http://127.0.0.1:19090
BOOT=worked-example-bootstrap-token-0123456789abcdef
CONF=$W/orchestrator.yaml

say() { printf '\n$ %s\n' "$*"; }
jget() { python3 -c 'import sys,json; d=json.load(sys.stdin); print(eval("d"+sys.argv[1]))' "$1"; }

rm -rf "$W/data" "$W/state" "$W/logs" "$W/server.pid" "$W/orch.pid"
mkdir -p "$W/logs"
sed -e "s|/var/lib/orchestrator|$W/state|" -e "s|/opt/orchestrator|$W|" "$HERE/orchestrator.yaml" > "$CONF"
cp "$HERE/shell-bot.sh" "$W/shell-bot.sh"
export AG_BOOTSTRAP_TOKEN=$BOOT
cleanup() {
  for p in server orch; do [ -f "$W/$p.pid" ] && kill "$(cat "$W/$p.pid")" 2>/dev/null || true; done
  [ "$KEEP" = 1 ] || rm -rf "$W"
}
trap cleanup EXIT

A="$SRV admin --data-dir $W/data"
say "ag-server admin create-repo acme/web"
$A create-repo acme/web
for pair in "user:alice writer" "user:bob reviewer" "user:carol integrator" "service:orchestrator admin" \
            "service:reviewer reviewer" "service:advisor reviewer" "agent:docs-bot writer" "agent:shell-bot writer"; do
  set -- $pair
  say "ag-server admin grant --subject $1 --repo acme/web --role $2"
  $A grant --subject "$1" --repo acme/web --role "$2"
done

say "orchestrator ops -config orchestrator.yaml -for orchestrator -flags"
$ORCH ops -config "$CONF" -for orchestrator -flags

# Credentials are shown once by `ag-server admin mint`; they are kept in shell variables here.
token() { $A mint "$@" 2>/dev/null | tail -n 1; }
ALICE=$(token --subject user:alice --ttl 86400)
BOB=$(token --subject user:bob --ttl 86400)
CAROL=$(token --subject user:carol --ttl 86400)
say "ag-server admin mint --subject service:orchestrator --ttl 2592000 --repo acme/web \$(orchestrator ops -for orchestrator -flags)"
export AG_ORCH_TOKEN=$(token --subject service:orchestrator --ttl 2592000 --repo acme/web $($ORCH ops -config "$CONF" -for orchestrator -flags))
export AG_REVIEWER_TOKEN=$(token --subject service:reviewer --ttl 2592000 --repo acme/web $($ORCH ops -config "$CONF" -for reviewer -flags))
export AG_ADVISOR_TOKEN=$(token --subject service:advisor --ttl 2592000 --repo acme/web $($ORCH ops -config "$CONF" -for advisor -flags))
echo "(three service credentials minted; not shown)"

say "ag-server serve --data-dir data --listen 127.0.0.1:18090 &"
"$SRV" serve --data-dir "$W/data" --listen 127.0.0.1:18090 > "$W/logs/server.log" 2>&1 &
echo $! > "$W/server.pid"
for i in $(seq 1 100); do curl -fsS "$URL/v1/negotiate" >/dev/null 2>&1 && break; sleep 0.1; done

api() { tok=$1; shift; curl -sS -H "authorization: Bearer $tok" -H 'content-type: application/json' "$@"; }

say "# policy: agents' changes need signed provenance and one human approval"
api "$BOOT" -X PUT "$URL/v1/repos/acme/web/policy" \
  -d '{"stream":"main","document":"{\"require_provenance_for_agents\": true, \"approvals\": {\"min\": 0, \"min_for_agents\": 1}}"}' \
  | python3 -c 'import sys,json; d=json.load(sys.stdin); print({k:d[k] for k in ("stream","version") if k in d} or d)'

say "# a person seeds the repository"
b64() { printf '%s' "$1" | base64 | tr -d '\n'; }
GO=$(b64 'package greet

// Greet says hello to name.
func Greet(name string) string { return "hello, " + name }
')
RD=$(b64 '# acme/web
')
api "$ALICE" -X POST "$URL/v1/repos/acme/web/workspaces" -d '{"name":"seed","stream":"main"}' > /dev/null
HEAD=$(api "$ALICE" -X POST "$URL/v1/repos/acme/web/workspaces/seed/changes" \
  -d "{\"message\":\"Seed\",\"changes\":[{\"path\":\"README.md\",\"op\":\"put\",\"content\":\"$RD\"},{\"path\":\"src/greet/greet.go\",\"op\":\"put\",\"content\":\"$GO\"}]}" | jget '["head"]')
PID0=$(api "$ALICE" -X POST "$URL/v1/repos/acme/web/proposals" -d "{\"stream\":\"main\",\"head\":\"$HEAD\",\"title\":\"Seed\"}" | jget '["id"]')
api "$CAROL" -X POST "$URL/v1/repos/acme/web/proposals/$PID0/integrate" -d '{}' | jget '["outcome"]["status"]'

say "# tasks: B depends on A; C is for the shell executor"
task() { api "$ALICE" -X POST "$URL/v1/repos/acme/web/tasks" -d "$1" | jget '["id"]'; }
TA=$(task '{"title":"Write the architecture overview","scope":["docs/**"],"depends_on":[],"description":"ag-script:{\"title\":\"Add the architecture overview\",\"files\":{\"docs/architecture.md\":\"# Architecture\\n\\nThe service is split into a core and an agent platform.\\n\"}}"}')
TB=$(task "{\"title\":\"Write the operations runbook\",\"scope\":[\"docs/**\"],\"depends_on\":[\"$TA\"],\"description\":\"ag-script:{\\\"title\\\":\\\"Add the runbook\\\",\\\"files\\\":{\\\"docs/runbook.md\\\":\\\"# Runbook\\\\n\\\\nRestart the service with SIGTERM.\\\\n\\\"}}\"}")
TC=$(task '{"title":"Leave a note","scope":["notes/**"],"depends_on":[],"description":"Write notes/shell-bot.md."}')
echo "A=$TA"; echo "B=$TB (depends on A)"; echo "C=$TC (routed to shell-bot)"

say "orchestrator check -config orchestrator.yaml"
$ORCH check -config "$CONF" || true

say "orchestrator run -config orchestrator.yaml &"
$ORCH run -config "$CONF" > "$W/logs/orchestrator.log" 2>&1 &
echo $! > "$W/orch.pid"
for i in $(seq 1 100); do curl -fsS "$ADMIN/readyz" >/dev/null 2>&1 && break; sleep 0.1; done
curl -sS "$ADMIN/readyz"

props() { api "$ALICE" "$URL/v1/repos/acme/web/proposals?state=open" | python3 -c '
import sys,json
for p in json.load(sys.stdin)["proposals"]:
    print(p["id"], p["author"], "task="+p["task_id"][-6:], repr(p["title"]))'; }
waitfor() { # waitfor <what> <command that succeeds when satisfied>
  n=0; while ! eval "$2" >/dev/null 2>&1; do n=$((n+1)); [ $n -gt 300 ] && { echo "timed out waiting for $1"; exit 1; }; sleep 0.2; done; }
proposal_of() { api "$ALICE" "$URL/v1/repos/acme/web/proposals?state=open" | python3 -c '
import sys,json
t=sys.argv[1]
for p in json.load(sys.stdin)["proposals"]:
    if p["task_id"]==t: print(p["id"]); break' "$1"; }

say "# the orchestrator ran task A (and C, which does not overlap); B waits for A"
waitfor "A's proposal" '[ -n "$(proposal_of $TA)" ]'
waitfor "C's proposal" '[ -n "$(proposal_of $TC)" ]'
props
PA=$(proposal_of $TA); PC=$(proposal_of $TC)

say "curl $ADMIN/status  (excerpt: scheduler, validation states of the open proposals)"
sleep 1
curl -sS "$ADMIN/status" | python3 -c '
import sys,json
d=json.load(sys.stdin)
r=d["repos"][0]
print("tasks:", r["scheduler"]["tasks"], " deferred:", [(x["title"],x["reason"]) for x in r["scheduler"]["deferred"]])
for p in r["execution_scheduler"]["open"]:
    print(p["id"], p["author"].split("#")[0], "mergeable=%s validated=%s approved=%s ready=%s" % (p["mergeable"],p["validated"],p["approved"],p["ready_to_integrate"]),
          [b["code"] for b in p.get("blockers",[])])'

say "# what the server records about A: its author and the signed provenance"
api "$ALICE" "$URL/v1/repos/acme/web/proposals/$PA" | python3 -c '
import sys,json
d=json.load(sys.stdin)
print("author:", d["proposal"]["author"])
print("reviews:", [(r["reviewer"], r["decision"]) for r in d["reviews"]])'
api "$ALICE" "$URL/v1/repos/acme/web/provenance?changeset=$(api "$ALICE" "$URL/v1/repos/acme/web/proposals/$PA" | jget '["proposal"]["head"]')" | python3 -c '
import sys,json
for e in json.load(sys.stdin)["entries"]:
    print("provenance:", e["record"]["agent"], "model="+e["record"]["model"], "verified=%s" % e["verified"])'

say "# the integrator tries to land A without an approval: the policy says no"
api "$CAROL" -X POST "$URL/v1/repos/acme/web/proposals/$PA/integrate" -d '{}' | python3 -c '
import sys,json
o=json.load(sys.stdin)["outcome"]; print(o["status"], [b["code"] for b in o["blockers"]])'

say "# bob approves, carol integrates"
api "$BOB" -X POST "$URL/v1/repos/acme/web/proposals/$PA/reviews" -d '{"decision":"approve","body":"looks good"}' | jget '["decision"]'
api "$CAROL" -X POST "$URL/v1/repos/acme/web/proposals/$PA/integrate" -d '{}' | python3 -c '
import sys,json
o=json.load(sys.stdin)["outcome"]; print(o["status"], o.get("strategy"))'

say "# A is finished by the orchestrator, B becomes ready and runs"
waitfor "A done" '[ "$(api $ALICE $URL/v1/repos/acme/web/tasks/$TA | jget "[\"state\"]")" = done ]'
waitfor "B proposal" '[ -n "$(proposal_of $TB)" ]'
PB=$(proposal_of $TB)
props
api "$BOB" -X POST "$URL/v1/repos/acme/web/proposals/$PB/reviews" -d '{"decision":"approve","body":"ok"}' > /dev/null
api "$CAROL" -X POST "$URL/v1/repos/acme/web/proposals/$PB/integrate" -d '{}' | python3 -c '
import sys,json
o=json.load(sys.stdin)["outcome"]; print(o["status"])'

say "# C came from the shell executor, which does not sign provenance: the policy blocks it"
api "$BOB" -X POST "$URL/v1/repos/acme/web/proposals/$PC/reviews" -d '{"decision":"approve","body":"ok"}' > /dev/null
api "$CAROL" -X POST "$URL/v1/repos/acme/web/proposals/$PC/integrate" -d '{}' | python3 -c '
import sys,json
o=json.load(sys.stdin)["outcome"]; print(o["status"], [(b["code"]) for b in o["blockers"]])'

say "# the semantic index follows the stream"
waitfor "index" 'curl -fsS "$ADMIN/index/symbols?q=Greet&mode=exact" | grep -q Greet'
curl -sS "$ADMIN/index/symbols?q=Greet&mode=exact" | python3 -c '
import sys,json
d=json.load(sys.stdin)
for s in d["symbols"]:
    for df in s["definitions"]: print(s["name"], df["kind"], df["path"]+":"+str(df["line"]))'
curl -sS "$ADMIN/index/status" | python3 -c '
import sys,json
d=json.load(sys.stdin)["repos"][0]; print("indexed files=%d symbols=%d" % (d["files"], d["symbols"]))'

say "# the audit trail: who moved the stream?"
api "$ALICE" "$URL/v1/repos/acme/web/events/replay?topics=stream&limit=100" | python3 -c '
import sys,json
for e in json.load(sys.stdin)["events"]:
    if e["topic"]=="stream.integrate": print(e["topic"], "by", e["payload"]["actor"])'

say "curl $ADMIN/metrics  (excerpt)"
curl -sS "$ADMIN/metrics" | grep -E '^orchestrator_(sessions_started_total|sessions_finished_total|credentials_minted_total|validations_queued_total|reviews_posted_total|tasks_deferred_total)' || true

say "kill -TERM \$(cat orch.pid)"
kill -TERM "$(cat "$W/orch.pid")"
n=0; while kill -0 "$(cat "$W/orch.pid")" 2>/dev/null; do n=$((n+1)); [ $n -gt 300 ] && { echo "did not stop"; exit 1; }; sleep 0.1; done
echo "orchestrator stopped"
echo
echo "--- orchestrator log (excerpt) ---"
grep -v "level=DEBUG" "$W/logs/orchestrator.log" | sed 's/time=[^ ]* //' | cut -c1-230
