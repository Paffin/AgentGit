#!/usr/bin/env bash
# Run a command against a throwaway PostgreSQL cluster.
#
#   scripts/pg-test.sh cargo test --workspace
#   AG_REQUIRE_BACKENDS=1 scripts/pg-test.sh scripts/s3-test.sh cargo test --workspace
#
# Starts a private cluster (trust authentication, localhost only, a free port,
# durability settings relaxed for speed), exports AG_TEST_POSTGRES_URL for the
# command, and tears the cluster down afterwards whatever the command's exit
# status. With no command it prints the URL and keeps the cluster up until Enter.
#
# Needs the PostgreSQL server binaries (initdb, pg_ctl, postgres). They are
# looked up in $PG_BIN_DIR, then `pg_config --bindir`, then
# /usr/lib/postgresql/<newest>/bin, then $PATH. PostgreSQL refuses to run as
# root, so as root the cluster is created by the `postgres` OS user.
#
# Environment:
#   PG_BIN_DIR    directory with the server binaries
#   PG_TEST_KEEP  set to 1 to keep the data directory (it is printed at the end)
set -euo pipefail

find_bin_dir() {
    if [ -n "${PG_BIN_DIR:-}" ]; then echo "$PG_BIN_DIR"; return; fi
    if command -v pg_config >/dev/null 2>&1; then
        local d
        d="$(pg_config --bindir 2>/dev/null || true)"
        if [ -n "$d" ] && [ -x "$d/initdb" ]; then echo "$d"; return; fi
    fi
    local newest
    newest="$(ls -d /usr/lib/postgresql/*/bin 2>/dev/null | sort -V | tail -n 1 || true)"
    if [ -n "$newest" ] && [ -x "$newest/initdb" ]; then echo "$newest"; return; fi
    if command -v initdb >/dev/null 2>&1; then dirname "$(command -v initdb)"; return; fi
    return 1
}

BIN="$(find_bin_dir)" || { echo "pg-test.sh: cannot find the PostgreSQL server binaries (set PG_BIN_DIR)" >&2; exit 2; }

# Run a PostgreSQL tool as the right OS user.
as_pg() {
    if [ "$(id -u)" = 0 ]; then
        runuser -u postgres -- "$@"
    else
        "$@"
    fi
}

ROOT="$(mktemp -d "${TMPDIR:-/tmp}/ag-pg-test.XXXXXX")"
DATA="$ROOT/data"
SOCK="$ROOT/sock"
mkdir -p "$SOCK"
if [ "$(id -u)" = 0 ]; then chown -R postgres "$ROOT"; fi
chmod 755 "$ROOT"

free_port() {
    python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}
PORT="$(free_port)"

STARTED=0
cleanup() {
    local status=$?
    if [ "$STARTED" = 1 ]; then
        as_pg "$BIN/pg_ctl" -D "$DATA" -m immediate -w stop >/dev/null 2>&1 || true
    fi
    if [ "${PG_TEST_KEEP:-0}" = 1 ]; then
        echo "pg-test.sh: kept $ROOT" >&2
    else
        rm -rf "$ROOT"
    fi
    exit "$status"
}
trap cleanup EXIT

as_pg "$BIN/initdb" -D "$DATA" -A trust -U postgres -E UTF8 --locale=C.UTF-8 >"$ROOT/initdb.log" 2>&1 \
    || as_pg "$BIN/initdb" -D "$DATA" -A trust -U postgres -E UTF8 --locale=C >"$ROOT/initdb.log" 2>&1 \
    || { cat "$ROOT/initdb.log" >&2; exit 2; }

# A private CA and a server certificate for localhost / 127.0.0.1, so the TLS
# code paths (sslmode=require, verify-full + sslrootcert) are tested for real.
SSL_OPTS=""
if command -v openssl >/dev/null 2>&1; then
    (
        cd "$ROOT"
        openssl req -x509 -newkey rsa:2048 -nodes -keyout ca.key -out ca.pem -days 2 -subj "/CN=agentgit-test-ca" >/dev/null 2>&1
        openssl req -newkey rsa:2048 -nodes -keyout server.key -out server.csr -subj "/CN=localhost" >/dev/null 2>&1
        printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n' >server.ext
        openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out server.pem -days 2 -extfile server.ext >/dev/null 2>&1
    ) && {
        chmod 600 "$ROOT/server.key"
        chmod 644 "$ROOT/ca.pem" "$ROOT/server.pem"
        if [ "$(id -u)" = 0 ]; then chown postgres "$ROOT/server.key" "$ROOT/server.pem" "$ROOT/ca.pem"; fi
        SSL_OPTS="-c ssl=on -c ssl_cert_file=$ROOT/server.pem -c ssl_key_file=$ROOT/server.key"
        export AG_TEST_POSTGRES_CA="$ROOT/ca.pem"
    }
fi

# Test-only settings: durability is irrelevant for a throwaway cluster, and the
# suites open many connections and create/drop many schemas.
as_pg "$BIN/pg_ctl" -D "$DATA" -l "$ROOT/postgres.log" -w start -o "\
 -p $PORT -k $SOCK -c listen_addresses=127.0.0.1 \
 -c fsync=off -c synchronous_commit=off -c full_page_writes=off \
 -c max_connections=300 -c max_locks_per_transaction=512 \
 -c log_min_messages=warning $SSL_OPTS" >/dev/null \
    || { cat "$ROOT/postgres.log" >&2; exit 2; }
STARTED=1

export AG_TEST_POSTGRES_URL="postgres://postgres@127.0.0.1:$PORT/postgres?sslmode=disable"
echo "pg-test.sh: PostgreSQL ready at $AG_TEST_POSTGRES_URL" >&2

if [ "$#" -eq 0 ]; then
    echo "AG_TEST_POSTGRES_URL=$AG_TEST_POSTGRES_URL"
    echo "pg-test.sh: cluster is up; press Enter to stop it" >&2
    read -r _
    exit 0
fi

# Not `exec`: the trap must run afterwards.
"$@"
