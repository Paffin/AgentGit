#!/usr/bin/env bash
# Builds a compatibility fixture for the SERVER: a data directory written by a real `ag-server` - the control
# database (credentials, role bindings, repositories, idempotency records) and the per-repository stores - by
# driving that server's HTTP API.
#
# Writes into the output directory:
#   control-<label>.tar.gz           the data directory as the server left it (control.db and repos/)
#   control-<label>.expected.json    the credentials' tokens, the bindings and the repositories to expect, the
#                                    control database's schema version and the server version that wrote it
#
# Like the repository fixtures these are append-only (docs/architecture/compatibility.md): never regenerate or edit
# an existing one. Make a new one when the control database schema changes (a new migrations/control/*/NNNN_*.sql),
# with a server built from the commit BEFORE that change if you want a fixture that proves the upgrade, or from the
# new code if you want one that pins the new layout. The tokens inside are throw-away secrets of the fixture.
#
#   tests/compat/generate_server.sh <label> [output-dir]
#   AG_SERVER=/path/to/ag-server tests/compat/generate_server.sh schema3
#
# FIXTURE_RUN_CREDENTIALS=1 also creates an agent, a run and a credential bound to that run (ADR-0010, schema 3):
# set it only for a server that has the `delegator` role and run-bound credentials.
#
# FIXTURE_BUILT_FROM=<commit> records which commit the server was built from in the manifest (a server built from
# an exported source tree does not know it).
#
# Copy the binary out of a shared cargo target directory first - another build may overwrite it while this runs.
set -euo pipefail

LABEL="${1:?usage: generate_server.sh <label> [output-dir]}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OUT="${2:-$HERE/fixtures}"
SERVER="${AG_SERVER:-}"
if [ -z "$SERVER" ]; then
  if [ -x "$ROOT/target/release/ag-server" ]; then SERVER="$ROOT/target/release/ag-server"
  elif [ -x "$ROOT/target/debug/ag-server" ]; then SERVER="$ROOT/target/debug/ag-server"
  else echo "ag-server not found: cargo build -p agentgit-server, or set AG_SERVER=" >&2; exit 1; fi
fi
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"

WORK="$(mktemp -d)"
PID=""
cleanup() { [ -n "$PID" ] && kill "$PID" 2>/dev/null || true; rm -rf "$WORK"; }
trap cleanup EXIT
DATA="$WORK/data"
BOOTSTRAP="compat-bootstrap-token-0123456789abcdef"
# A valid Ed25519 public key (the one of RFC 8032, test vector 1), registered for the agent below.
AGENT_KEY="d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
BASE="http://127.0.0.1:$PORT"

AG_BOOTSTRAP_TOKEN="$BOOTSTRAP" "$SERVER" serve --listen "127.0.0.1:$PORT" --data-dir "$DATA" >"$WORK/server.log" 2>&1 &
PID=$!
for _ in $(seq 1 100); do
  curl -fsS "$BASE/readyz" >/dev/null 2>&1 && break
  kill -0 "$PID" 2>/dev/null || { cat "$WORK/server.log" >&2; echo "server exited" >&2; exit 1; }
  sleep 0.1
done
curl -fsS "$BASE/readyz" >/dev/null

api() { # api METHOD PATH [JSON-BODY] [TOKEN] -> response body; fails (showing the body) on HTTP errors
  local method=$1 path=$2 body=${3:-} token=${4:-$BOOTSTRAP} out code
  out="$WORK/reply.out"
  if [ -n "$body" ]; then
    code=$(curl -sS -o "$out" -w '%{http_code}' -X "$method" -H "authorization: Bearer $token" -H 'content-type: application/json' -d "$body" "$BASE$path")
  else
    code=$(curl -sS -o "$out" -w '%{http_code}' -X "$method" -H "authorization: Bearer $token" "$BASE$path")
  fi
  if [ "${code:0:1}" != 2 ]; then echo "$method $path -> HTTP $code: $(cat "$out")" >&2; exit 1; fi
  cat "$out"
}
field() { python3 -c "import sys,json; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }

# repositories (one with an idempotency key, one with a quota)
api POST /v1/repos '{"name":"acme/web","default_stream":"main","description":"the web app","idempotency_key":"compat-create-web"}' >/dev/null
api POST /v1/repos '{"name":"acme/docs","default_stream":"trunk","description":"the documentation"}' >/dev/null
api PATCH /v1/repos/acme/docs '{"quota":{"max_open_proposals":5,"max_streams":3}}' >/dev/null

# credentials: unrestricted, scoped to one repository and stream, and one that is revoked afterwards
mint() { api POST /v1/credentials "$1" "${2:-$BOOTSTRAP}"; }
ALICE=$(mint '{"subject":"user:alice","ttl_secs":86400,"description":"alice, unrestricted","scope":{}}')
BOT=$(mint '{"subject":"agent:bot","ttl_secs":3600,"description":"a bot, one repository and stream","scope":{"repos":["acme/web"],"streams":["main"],"operations":["objects.write","objects.read","publish","stream.read","stream.cas","history.read","tree.read","repo.read"]}}')
GONE=$(mint '{"subject":"user:mallory","ttl_secs":86400,"description":"revoked","scope":{}}')
GONE_ID=$(echo "$GONE" | field 'd["credential"]["id"]')
api DELETE "/v1/credentials/$GONE_ID" >/dev/null

# role bindings
for b in '{"subject":"user:alice","repo":"acme/*","role":"writer"}' \
         '{"subject":"agent:bot","repo":"acme/web","role":"writer"}' \
         '{"subject":"user:bob","repo":"acme/web","role":"reader"}' \
         '{"subject":"group:platform","repo":"*","role":"admin"}'; do
  api POST /v1/roles "$b" >/dev/null
done

mkdir -p "$WORK/json"
if [ "${FIXTURE_RUN_CREDENTIALS:-0}" = 1 ]; then
  # an orchestrator (delegator) hands an agent's session a credential bound to one run of that agent
  api POST /v1/roles '{"subject":"service:orchestrator","repo":"acme/*","role":"delegator"}' >/dev/null
  ORCH=$(mint '{"subject":"service:orchestrator","ttl_secs":86400,"description":"the orchestrator","scope":{}}')
  ORCH_TOKEN=$(echo "$ORCH" | field 'd["token"]')
  BOT_FULL=$(mint '{"subject":"agent:bot","ttl_secs":3600,"description":"the agent itself, unrestricted","scope":{}}')
  BOT_TOKEN=$(echo "$BOT_FULL" | field 'd["token"]')
  api POST /v1/repos/acme/web/agents "{\"name\":\"bot\",\"public_key\":\"$AGENT_KEY\"}" "$BOT_TOKEN" >/dev/null
  RUN=$(api POST /v1/repos/acme/web/runs '{"agent":"bot"}' "$BOT_TOKEN")
  RUN_ID=$(echo "$RUN" | field 'd["run_id"]')
  BOUND=$(mint "{\"subject\":\"agent:bot\",\"ttl_secs\":3600,\"description\":\"bound to one run\",\"scope\":{\"repos\":[\"acme/web\"],\"run_id\":\"$RUN_ID\"}}" "$ORCH_TOKEN")
  echo "$BOUND" > "$WORK/json/mint_bound.json"
  echo "$ORCH" > "$WORK/json/mint_orch.json"
  echo "$RUN_ID" > "$WORK/json/run_id.txt"
  api GET /v1/whoami '' "$(echo "$BOUND" | field 'd["token"]')" > "$WORK/json/whoami_bound.json"
fi

# what the API says now - the fixture's expectation
api GET /v1/repos            > "$WORK/json/repos.json"
api GET /v1/roles            > "$WORK/json/roles.json"
api GET /v1/credentials      > "$WORK/json/credentials.json"
api GET /v1/whoami           > "$WORK/json/whoami_bootstrap.json"
api GET /v1/whoami '' "$(echo "$ALICE" | field 'd["token"]')" > "$WORK/json/whoami_alice.json"
api GET /v1/whoami '' "$(echo "$BOT" | field 'd["token"]')"   > "$WORK/json/whoami_bot.json"
echo "$ALICE" > "$WORK/json/mint_alice.json"
echo "$BOT"   > "$WORK/json/mint_bot.json"
echo "$GONE"  > "$WORK/json/mint_gone.json"

# stop the server cleanly before the directory is archived, then fold the write-ahead log into the database so the
# fixture is the single file control.db
kill -TERM "$PID"; wait "$PID" 2>/dev/null || true; PID=""
python3 - "$DATA/control.db" <<'PY'
import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
db.close()
PY
rm -f "$DATA/control.db-wal" "$DATA/control.db-shm"

SCHEMA="$(python3 -c "import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute('PRAGMA user_version').fetchone()[0])" "$DATA/control.db")"
SERVER_VERSION="$("$SERVER" --version)"
python3 - "$WORK/json" "$BOOTSTRAP" "$SCHEMA" "$SERVER_VERSION" "${FIXTURE_BUILT_FROM:-}" > "$WORK/expected.json" <<'PY'
import json, pathlib, sys
d = pathlib.Path(sys.argv[1])
load = lambda n: json.loads((d / f"{n}.json").read_text())
def items(doc):  # list responses are wrapped in an object with one array field
    return doc if isinstance(doc, list) else next(v for v in doc.values() if isinstance(v, list))
out = {
    "format": "agentgit-compat-server-expected-v1",
    "server": sys.argv[4],
    "built_from": sys.argv[5],
    "control_schema_version": int(sys.argv[3]),
    "bootstrap_token": sys.argv[2],
    "tokens": {"alice": load("mint_alice")["token"], "bot": load("mint_bot")["token"], "revoked": load("mint_gone")["token"]},
    "credential_ids": {"alice": load("mint_alice")["credential"]["id"], "bot": load("mint_bot")["credential"]["id"], "revoked": load("mint_gone")["credential"]["id"]},
    "repos": sorted([r["name"], r["default_stream"], r["namespace"]] for r in items(load("repos"))),
    "roles": sorted([r["subject"], r["repo"], r["role"]] for r in items(load("roles"))),
    "whoami": {"alice": load("whoami_alice"), "bot": load("whoami_bot"), "bootstrap": load("whoami_bootstrap")},
    "created_at_ms": min(load(n)["credential"]["created_at_ms"] for n in ("mint_alice", "mint_bot", "mint_gone")),
}
if (d / "mint_bound.json").exists():
    out["run_bound"] = {
        "token": load("mint_bound")["token"],
        "credential_id": load("mint_bound")["credential"]["id"],
        "run_id": (d / "run_id.txt").read_text().strip(),
        "repo": "acme/web",
        "agent": "bot",
        "whoami": load("whoami_bound"),
    }
print(json.dumps(out, indent=1, sort_keys=True))
PY

TAR="$OUT/control-$LABEL.tar.gz"
( cd "$WORK" && tar --sort=name --owner=0 --group=0 --numeric-owner --mtime='2026-01-01 00:00:00' -czf "$TAR" data )
cp "$WORK/expected.json" "$OUT/control-$LABEL.expected.json"
echo "wrote $TAR ($(du -h "$TAR" | cut -f1)), control schema $SCHEMA, from $SERVER_VERSION"
