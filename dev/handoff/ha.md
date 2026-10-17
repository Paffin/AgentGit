# Handoff: multi-replica ag-server (ADR-0007)

Design: `docs/adr/0007-multi-replica-server.md` (state inventory, decisions, invariants). Read it first.

## Implemented (compiles; unit tests pass on SQLite and PostgreSQL via `scripts/pg-test.sh`)
- `ag_storage::sqldb` (crates/ag-storage/src/sqldb.rs): dialect-neutral `Database` over SQLite/PostgreSQL with caller-supplied
  migrations, single-statement autocommit (`execute`/`query`, stale pooled connections probed), `read`/`write` transactions,
  `notify`/`listen` (PostgreSQL LISTEN/NOTIFY). `PgBackend::open_with` takes a migration list; `Migration` is public.
- Control plane (`crates/ag-server/src/control/`): `Control` now runs on `Database`; `[control] url|url_env` selects PostgreSQL,
  absent = SQLite `control.db` unchanged (old v1 files adopted, migrated to v2). Migrations: `migrations/control/{sqlite,postgres}/`.
  v2 adds `repos.config_json` (registry stores `.ag/config.json`; `RepoManager::ensure_local_marker` materializes it on another
  replica, only for PostgreSQL metadata + S3 objects). `control/dump.rs`: logical `ControlDump` export/import.
- `crates/ag-server/src/cluster.rs`: `Bus` (publisher + LISTEN threads), `Local` waiters; `App::notify/invalidate_bindings/announce_jobs`
  publish to other replicas; listener (re)connect = `Resync`. Polling stays the floor (SSE 500 ms, jobs 5 s, bindings TTL).
- Ops: `/readyz` = not draining AND control DB answers (2 s), `/healthz` unchanged; `server.drain_secs` drain on SIGTERM;
  job long-polls end on shutdown; SSE re-checks the credential every 5 s (`Authenticator::still_active`); metrics `replica_info`,
  `control_schema_version`, `draining`, `event_delivery_lag_seconds`, `cluster_*`; `replica_id` config (default $HOSTNAME).
- `ag-server admin` gained `--config`, `backup-control --out`, `restore-control --from [--replace]`,
  `migrate-control --from sqlite --to postgres [--replace]`.
- `error.rs`: MetaError Retriable/ConnectionLost map to `unavailable` (retry) / `deadline_exceeded` (unknown commit outcome).
- All control DB access from async code goes through `spawn_blocking` (`App::repo_row`); the sync postgres driver panics inside a runtime.

## Not done / unverified
- `crates/ag-server/tests/multi_replica.rs` (+ `scripts/ha-test.sh`): written, compiles, NOT yet run to green (kept off main,
  on branch `wip/ha-track`). Run `scripts/ha-test.sh` (needs PG 16 binaries + moto:
  `python3 -m venv ~/.cache/agentgit/s3-venv && ~/.cache/agentgit/s3-venv/bin/pip install 'moto[server]==5.2.4'`).
  Expect to debug readiness, SSE parsing, rolling-restart timings. Copy the ag-server binary into the temp dir before spawning
  (the uplifted `target/debug/ag-server` is shared with other agents and can be replaced mid-test).
- Not written: a `control_migration` test (sqlite -> postgres through the CLI with real data; unit coverage exists in
  `control/dump.rs`), docs/operations/high-availability.md, updates to deployment.md / adr-traceability.md / security.md /
  docs/README.md index, `deploy/kubernetes/overlays/ha` (replicas 3, PDB, anti-affinity, RollingUpdate, emptyDir data, ConfigMap
  with `[control]`, `drain_secs`, terminationGracePeriodSeconds 60), Compose profile, CI step
  (`AG_REQUIRE_BACKENDS=1 scripts/pg-test.sh scripts/s3-test.sh cargo test -p ag-server --test multi_replica`; the backends job's
  `cargo test --workspace` already runs it once the file is on main).
- Config validation tests for `[control]`; remove unused `Replica.dir`/`term` in the test file.
- Limits to document: rate limits are per replica; a crash between repository init and registration leaves an unregistered
  schema; PostgreSQL connection budget = replicas x repos.max_open x repos.pool_size; clock skew affects the lag metric and leases.

## Environment notes
- Shared cargo target dir: concurrent agents overwrite each other's artifacts. A local, untracked `.cargo/config.toml` with
  `[profile.dev.package.ag-storage] codegen-units = 251` gives private hashes. Disk is very tight; delete only your own artifacts.
