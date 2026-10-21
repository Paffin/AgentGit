#!/usr/bin/env bash
# Run the multi-replica tests: real `ag-server` processes on one PostgreSQL and one S3-compatible server.
#
#   scripts/ha-test.sh                          # all of them
#   scripts/ha-test.sh a_rolling_restart        # tests whose name contains this
#
# A thin wrapper over the harnesses CI's "backends" job uses (scripts/pg-test.sh starts a throwaway PostgreSQL
# cluster, scripts/s3-test.sh a throwaway moto server). The tests start 2-5 copies of the real binary,
# kill -9 and restart some of them, and need nothing else. AG_REQUIRE_COMPONENTS=postgres,s3 makes a missing
# backend a failure instead of a skip.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export AG_REQUIRE_COMPONENTS="postgres,s3${AG_REQUIRE_COMPONENTS:+,$AG_REQUIRE_COMPONENTS}"
exec "$HERE/pg-test.sh" "$HERE/s3-test.sh" cargo test -p agentgit-server --test multi_replica -- --nocapture --test-threads=1 "$@"
