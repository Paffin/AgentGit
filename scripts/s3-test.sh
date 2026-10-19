#!/usr/bin/env bash
# Run a command against a throwaway S3-compatible server.
#
#   scripts/s3-test.sh cargo test --workspace                       # moto (the default)
#   AG_S3_SERVER=seaweedfs scripts/s3-test.sh cargo test -p agentgit-storage -p agentgit-integrate
#   AG_REQUIRE_BACKENDS=1 scripts/pg-test.sh scripts/s3-test.sh cargo test --workspace
#
# Two servers, chosen by AG_S3_SERVER; the object-store suites (crates/ag-storage/tests/s3.rs and every test that
# moves its objects to S3 when AG_TEST_S3_ENDPOINT is set) are meant to pass on both:
#
#   moto        a Python re-implementation of the S3 API (conditional PUT, CopyObject, ListObjectsV2 pagination).
#               Installed into a private virtualenv. With IAM enforcement on it recomputes every request's SigV4
#               signature, which makes it an independent check of our signer.
#   seaweedfs   the real thing: the Go SeaweedFS server that the Compose stack (deploy/compose) runs, started as a
#               single `weed server -s3` process on a pinned release whose download is checked against a pinned
#               SHA-256. A root identity is configured (`-s3.config`), so the S3 gateway verifies every request's
#               SigV4 signature; the script refuses to go on if an unsigned request is answered. The gateway also
#               listens on HTTPS with a certificate from a private CA created here (needs `openssl`), exported as
#               AG_TEST_S3_TLS_ENDPOINT and AG_TEST_S3_CA for the TLS tests of the client.
#
# Either way the script creates the bucket and credentials, exports AG_TEST_S3_* for the command, and stops the server
# afterwards. With no command it prints the settings and keeps the server up until Enter.
#
# Needs python3 (free ports, random credentials). moto also needs venv + pip and network access the first time (to
# install moto into $AG_S3_VENV, default ~/.cache/agentgit/s3-venv; point it at a directory that already has
# `moto[server]` to work offline). SeaweedFS needs curl, tar and network access the first time (the binary is cached
# under $AG_S3_CACHE, default ~/.cache/agentgit); set AG_SEAWEEDFS_BIN to a `weed` binary to skip the download
# (its version is then yours to know: the pinned one is $SEAWEEDFS_VERSION below).
#
# Environment:
#   AG_S3_SERVER      moto (default) or seaweedfs
#   AG_S3_VENV        virtualenv with moto[server] (created on demand)
#   AG_S3_CACHE       where the SeaweedFS binary is cached
#   AG_SEAWEEDFS_BIN  a `weed` binary to use instead of the pinned download
#   AG_S3_BUCKET      bucket name to create (default ag-test)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVER_KIND="${AG_S3_SERVER:-moto}"
BUCKET="${AG_S3_BUCKET:-ag-test}"
REGION="us-east-1"

MOTO_VERSION="5.2.4"
VENV="${AG_S3_VENV:-${XDG_CACHE_HOME:-$HOME/.cache}/agentgit/s3-venv}"

# The SeaweedFS release the Compose stack pins (deploy/compose/compose.yaml); the SHA-256 of each release asset.
SEAWEEDFS_VERSION="4.48"
CACHE="${AG_S3_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/agentgit}"

die() { echo "s3-test.sh: $*" >&2; exit 2; }

case "$SERVER_KIND" in
    moto|seaweedfs) ;;
    *) die "AG_S3_SERVER must be moto or seaweedfs (got '$SERVER_KIND')" ;;
esac

# The test server is local: never route it through a proxy.
export NO_PROXY="127.0.0.1,localhost${NO_PROXY:+,$NO_PROXY}"
export no_proxy="$NO_PROXY"

# Prints N free local ports, each with its +10000 neighbour free as well (SeaweedFS puts gRPC there).
free_ports() {
    python3 - "$1" <<'PY'
import socket, sys

def free(p):
    s = socket.socket()
    try:
        s.bind(("127.0.0.1", p))
        return True
    except OSError:
        return False
    finally:
        s.close()

found = []
while len(found) < int(sys.argv[1]):
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    if p + 10000 < 65000 and p not in found and free(p + 10000):
        found.append(p)
print(" ".join(map(str, found)))
PY
}

LOG="$(mktemp "${TMPDIR:-/tmp}/ag-s3-test.XXXXXX")"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/ag-s3-work.XXXXXX")"
SERVER=""
cleanup() {
    local status=$?
    if [ -n "$SERVER" ]; then
        kill "$SERVER" >/dev/null 2>&1 || true
        # A server that does not stop within ten seconds is killed: a stuck shutdown must not hang the test run.
        for _ in $(seq 1 50); do
            kill -0 "$SERVER" >/dev/null 2>&1 || break
            sleep 0.2
        done
        kill -9 "$SERVER" >/dev/null 2>&1 || true
        wait "$SERVER" >/dev/null 2>&1 || true
    fi
    if [ "$status" -ne 0 ] && [ -s "$LOG" ]; then
        echo "s3-test.sh: server log tail ($LOG):" >&2
        tail -n 30 "$LOG" >&2
    else
        rm -f "$LOG"
    fi
    rm -rf "$WORK"
    exit "$status"
}
trap cleanup EXIT

# ---- moto ------------------------------------------------------------------------------------------------

start_moto() {
    if [ ! -x "$VENV/bin/moto_server" ]; then
        echo "s3-test.sh: installing moto $MOTO_VERSION into $VENV (first run only)" >&2
        python3 -m venv "$VENV"
        "$VENV/bin/pip" install --quiet "moto[server]==$MOTO_VERSION"
    fi
    local port
    port="$(free_ports 1)"
    ENDPOINT="http://127.0.0.1:$port"
    "$VENV/bin/moto_server" -H 127.0.0.1 -p "$port" >"$LOG" 2>&1 &
    SERVER=$!
    for _ in $(seq 1 100); do
        if python3 - "$ENDPOINT" <<'PY' 2>/dev/null
import sys, urllib.request
urllib.request.urlopen(sys.argv[1] + "/moto-api/", timeout=1).read()
PY
        then break; fi
        sleep 0.1
    done
    local creds
    creds="$("$VENV/bin/python" "$HERE/s3_test_setup.py" "$ENDPOINT" "$BUCKET")"
    # shellcheck disable=SC2046
    export $creds
}

# ---- SeaweedFS -------------------------------------------------------------------------------------------

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# Prints the path of a `weed` binary: $AG_SEAWEEDFS_BIN, or the pinned release, downloaded once and verified.
seaweedfs_binary() {
    if [ -n "${AG_SEAWEEDFS_BIN:-}" ]; then
        [ -x "$AG_SEAWEEDFS_BIN" ] || die "AG_SEAWEEDFS_BIN is not an executable file: $AG_SEAWEEDFS_BIN"
        echo "$AG_SEAWEEDFS_BIN"
        return
    fi
    local asset sha
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64)
            asset=linux_amd64; sha=4a7d108384d044d95212d1342cdda9533fa55842c1c9b41f606ca3c8a9561124 ;;
        Linux-aarch64|Linux-arm64)
            asset=linux_arm64; sha=557e92c38c5c7d180748eb8e2b2b45e426f8e5282a4caca8e098a90d09f03a07 ;;
        Darwin-arm64)
            asset=darwin_arm64; sha=faa194e72a346755dc4404288b0561eb3c260351702901d8363024d5099bdd7d ;;
        Darwin-x86_64)
            asset=darwin_amd64; sha=425ef61d2ee773d5d5175003f38bd6cc891070ae6319370354aaf5378c45d5b3 ;;
        *) die "no pinned SeaweedFS release for $(uname -s)-$(uname -m); set AG_SEAWEEDFS_BIN" ;;
    esac
    local dir="$CACHE/seaweedfs-$SEAWEEDFS_VERSION-$asset"
    if [ ! -x "$dir/weed" ]; then
        echo "s3-test.sh: downloading SeaweedFS $SEAWEEDFS_VERSION ($asset) into $dir (first run only)" >&2
        mkdir -p "$dir"
        local archive="$WORK/seaweedfs.tar.gz"
        curl -fsSL --retry 3 -o "$archive" \
            "https://github.com/seaweedfs/seaweedfs/releases/download/$SEAWEEDFS_VERSION/$asset.tar.gz"
        [ "$(sha256_of "$archive")" = "$sha" ] || die "SHA-256 of the SeaweedFS download does not match the pinned value"
        tar -xzf "$archive" -C "$dir" weed
        rm -f "$archive"
    fi
    echo "$dir/weed"
}

random_token() { # LENGTH CHARSET
    python3 -c 'import secrets, sys; print("".join(secrets.choice(sys.argv[2]) for _ in range(int(sys.argv[1]))))' "$1" "$2"
}

start_seaweedfs() {
    local weed master volume filer s3 s3_tls
    weed="$(seaweedfs_binary)"
    read -r master volume filer s3 s3_tls <<<"$(free_ports 5)"
    ENDPOINT="http://127.0.0.1:$s3"

    # A private CA and a server certificate for localhost / 127.0.0.1, as scripts/pg-test.sh makes for PostgreSQL.
    command -v openssl >/dev/null 2>&1 || die "openssl is needed to give the SeaweedFS gateway a TLS certificate"
    (
        cd "$WORK"
        openssl req -x509 -newkey rsa:2048 -nodes -keyout ca.key -out ca.pem -days 2 -subj "/CN=agentgit-test-ca" >/dev/null 2>&1
        openssl req -newkey rsa:2048 -nodes -keyout server.key -out server.csr -subj "/CN=localhost" >/dev/null 2>&1
        printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n' >server.ext
        openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out server.pem -days 2 -extfile server.ext >/dev/null 2>&1
    ) || die "could not create the test certificate authority"

    local access secret
    access="AKIA$(random_token 16 ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789)"
    secret="$(random_token 40 abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789)"
    # With at least one identity configured the gateway authenticates every request (SigV4) instead of being open.
    cat >"$WORK/s3.json" <<JSON
{"identities":[{"name":"ag-test","credentials":[{"accessKey":"$access","secretKey":"$secret"}],"actions":["Admin","Read","Write","List","Tagging"]}]}
JSON
    mkdir -p "$WORK/data"
    # One process: master + volume server + filer + S3 gateway, everything on loopback. The small volume size keeps
    # the data directory small; -volume.max=0 sizes the number of volumes from the free disk. By default the volume
    # server turns read-only when less than 1 % of the *file system* is free, which on a big, nearly full disk is
    # gigabytes; every PUT then waits for capacity that never comes and fails minutes later. A throwaway store needs
    # an absolute margin instead. The gateway's extra
    # listeners (Iceberg, Lance) have fixed default ports that would collide between runs: off. Credentials from the
    # caller's AWS_* variables would add a second identity to the gateway: removed, so the only identity is ours.
    env -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY -u AWS_SESSION_TOKEN \
        "$weed" server -dir="$WORK/data" -ip=127.0.0.1 -ip.bind=127.0.0.1 \
        -master.port="$master" -volume.port="$volume" -filer.port="$filer" \
        -s3 -s3.port="$s3" -s3.ip.bind=127.0.0.1 -s3.config="$WORK/s3.json" \
        -s3.port.https="$s3_tls" -s3.cert.file="$WORK/server.pem" -s3.key.file="$WORK/server.key" \
        -s3.port.iceberg=0 -s3.port.lance=0 \
        -master.volumeSizeLimitMB=64 -volume.max=0 -volume.minFreeSpace=100MiB >"$LOG" 2>&1 &
    SERVER=$!

    local ready=""
    for _ in $(seq 1 300); do
        kill -0 "$SERVER" 2>/dev/null || die "the SeaweedFS server exited during start-up"
        if [ "$(curl -s -o /dev/null -w '%{http_code}' "$ENDPOINT/status" || true)" = "200" ]; then ready=1; break; fi
        sleep 0.1
    done
    [ -n "$ready" ] || die "the SeaweedFS S3 gateway did not come up"

    # The bucket is created the way the Compose stack does it. The master may need a moment after the gateway answers.
    local out="" created=""
    for _ in $(seq 1 100); do
        out="$(echo "s3.bucket.create -name $BUCKET" | "$weed" shell -master="127.0.0.1:$master" 2>&1 || true)"
        case "$out" in *"created bucket"*|*"already exists"*) created=1; break ;; esac
        sleep 0.2
    done
    [ -n "$created" ] || die "could not create bucket $BUCKET: $out"

    # The gateway and the bucket exist before the volume server has registered capacity with the master, and an object
    # can only be stored once it has. Ask the master for a file id: that succeeds when (and only when) a PUT would.
    # (When there is no capacity the master itself waits up to 30 s for some before it answers: bound each attempt.)
    local assigned="" assign_out=""
    for _ in $(seq 1 12); do
        assign_out="$(curl -s -m 10 "http://127.0.0.1:$master/dir/assign?collection=$BUCKET" || true)"
        case "$assign_out" in *'"fid"'*) assigned=1; break ;; esac
        sleep 0.5
    done
    [ -n "$assigned" ] || die "SeaweedFS cannot store data (the master offers no volume capacity: $assign_out); look at the free disk space and at the server log"

    # Signature verification must really be on, or a green run would say nothing about our signer.
    local anonymous
    anonymous="$(curl -s -o /dev/null -w '%{http_code}' "$ENDPOINT/$BUCKET/" || true)"
    [ "$anonymous" = "403" ] || die "an unsigned request to the bucket was answered with HTTP $anonymous (expected 403): the server is not authenticating requests"

    # TLS must really be served, with the private CA's certificate (the client is made to trust that CA only).
    local tls_code
    tls_code="$(curl -s --cacert "$WORK/ca.pem" -o /dev/null -w '%{http_code}' "https://localhost:$s3_tls/$BUCKET/" || true)"
    [ "$tls_code" = "403" ] || die "the HTTPS endpoint (port $s3_tls) did not answer an unsigned request with 403 (got '$tls_code')"

    export AG_TEST_S3_ACCESS_KEY_ID="$access"
    export AG_TEST_S3_SECRET_ACCESS_KEY="$secret"
    export AG_TEST_S3_TLS_ENDPOINT="https://localhost:$s3_tls"
    export AG_TEST_S3_CA="$WORK/ca.pem"
}

# ---- run -------------------------------------------------------------------------------------------------

ENDPOINT=""
case "$SERVER_KIND" in
    moto) start_moto ;;
    seaweedfs) start_seaweedfs ;;
esac

export AG_TEST_S3_ENDPOINT="$ENDPOINT"
export AG_TEST_S3_BUCKET="$BUCKET"
export AG_TEST_S3_REGION="$REGION"
echo "s3-test.sh: $SERVER_KIND ready at $AG_TEST_S3_ENDPOINT (bucket $BUCKET, signatures verified)" >&2

if [ "$#" -eq 0 ]; then
    echo "AG_TEST_S3_ENDPOINT=$AG_TEST_S3_ENDPOINT AG_TEST_S3_BUCKET=$BUCKET AG_TEST_S3_REGION=$REGION"
    echo "AG_TEST_S3_ACCESS_KEY_ID=$AG_TEST_S3_ACCESS_KEY_ID AG_TEST_S3_SECRET_ACCESS_KEY=$AG_TEST_S3_SECRET_ACCESS_KEY"
    echo "s3-test.sh: server is up; press Enter to stop it" >&2
    read -r _
    exit 0
fi

# Not `exec`: the trap must run afterwards.
"$@"
