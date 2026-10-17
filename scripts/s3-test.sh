#!/usr/bin/env bash
# Run a command against a throwaway S3-compatible server.
#
#   scripts/s3-test.sh cargo test --workspace
#   AG_REQUIRE_BACKENDS=1 scripts/pg-test.sh scripts/s3-test.sh cargo test --workspace
#
# Starts moto (a Python re-implementation of the S3 API, with conditional PUT,
# CopyObject and ListObjectsV2 pagination) in a private virtualenv, creates the
# bucket and a user, switches signature verification ON (moto then recomputes
# every request's SigV4 signature, which makes it an independent check of our
# signer), exports AG_TEST_S3_* for the command, and stops the server afterwards.
# With no command it prints the settings and keeps the server up until Enter.
#
# Needs python3 with venv + pip and network access the first time (to install
# moto into $AG_S3_VENV, default ~/.cache/agentgit/s3-venv). Set AG_S3_VENV to a
# directory that already has `moto[server]` installed to work offline.
#
# Environment:
#   AG_S3_VENV    virtualenv with moto[server] (created on demand)
#   AG_S3_BUCKET  bucket name to create (default ag-test)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VENV="${AG_S3_VENV:-${XDG_CACHE_HOME:-$HOME/.cache}/agentgit/s3-venv}"
BUCKET="${AG_S3_BUCKET:-ag-test}"
MOTO_VERSION="5.2.4"

if [ ! -x "$VENV/bin/moto_server" ]; then
    echo "s3-test.sh: installing moto $MOTO_VERSION into $VENV (first run only)" >&2
    python3 -m venv "$VENV"
    "$VENV/bin/pip" install --quiet "moto[server]==$MOTO_VERSION"
fi

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
ENDPOINT="http://127.0.0.1:$PORT"
LOG="$(mktemp "${TMPDIR:-/tmp}/ag-s3-test.XXXXXX.log")"

# The test server is local: never route it through a proxy.
export NO_PROXY="127.0.0.1,localhost${NO_PROXY:+,$NO_PROXY}"
export no_proxy="$NO_PROXY"

"$VENV/bin/moto_server" -H 127.0.0.1 -p "$PORT" >"$LOG" 2>&1 &
SERVER=$!
cleanup() {
    local status=$?
    kill "$SERVER" >/dev/null 2>&1 || true
    wait "$SERVER" >/dev/null 2>&1 || true
    if [ "$status" -ne 0 ] && [ -s "$LOG" ]; then
        echo "s3-test.sh: server log tail ($LOG):" >&2
        tail -n 20 "$LOG" >&2
    else
        rm -f "$LOG"
    fi
    exit "$status"
}
trap cleanup EXIT

for _ in $(seq 1 100); do
    if python3 - "$ENDPOINT" <<'PY' 2>/dev/null
import sys, urllib.request
urllib.request.urlopen(sys.argv[1] + "/moto-api/", timeout=1).read()
PY
    then break; fi
    sleep 0.1
done

CREDS="$("$VENV/bin/python" "$HERE/s3_test_setup.py" "$ENDPOINT" "$BUCKET")"
# shellcheck disable=SC2046
export $CREDS
export AG_TEST_S3_ENDPOINT="$ENDPOINT"
export AG_TEST_S3_BUCKET="$BUCKET"
export AG_TEST_S3_REGION="us-east-1"
echo "s3-test.sh: S3 server ready at $AG_TEST_S3_ENDPOINT (bucket $BUCKET, signatures verified)" >&2

if [ "$#" -eq 0 ]; then
    echo "AG_TEST_S3_ENDPOINT=$AG_TEST_S3_ENDPOINT AG_TEST_S3_BUCKET=$BUCKET"
    echo "s3-test.sh: server is up; press Enter to stop it" >&2
    read -r _
    exit 0
fi

# Not `exec`: the trap must run afterwards.
"$@"
