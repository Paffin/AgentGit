# ADR-0007: Several `ag-server` replicas on one PostgreSQL and one bucket

- Status: accepted and implemented: the PostgreSQL control plane, cluster notices, readiness/drain, control backup and
  migration. The invariants listed under "Invariants" are **proved** by multi-process tests that start real `ag-server`
  binaries on a real PostgreSQL and S3 server (`crates/ag-server/tests/multi_replica.rs`, run by the `backends` CI job
  against moto and against SeaweedFS). What remains a **design goal** and is not proved is listed in "Design goals not
  yet proved". The operator's guide is [high-availability.md](../operations/high-availability.md).
- Date: 2026-10-17
- Amends: [ADR-0004](0004-postgres-and-s3-backends.md) (which left "the server's own control database" on SQLite)

## Context

ADR-0004 made *repository* data shareable: metadata in a PostgreSQL schema per repository, objects under an S3
prefix per repository, writers of one repository serialized by an advisory lock and the stream compare-and-swap.
The *server* was still one process, because its **control database** (credentials, role bindings, the repository
registry, idempotency keys, quota counters) is one SQLite file next to it. This ADR makes two or more replicas
behind a load balancer a supported, tested topology, without changing the engine, the wire protocol or any format.

## Inventory — every piece of server-side state

| State | Lives in | Before | After |
|---|---|---|---|
| Credentials (hashes), role bindings, repository registry, idempotency keys, `kv`, `object_bytes` | control DB | one SQLite file | SQLite (default) **or** a PostgreSQL schema |
| Streams, reflog, CAS, proposals, attestations, policies, agent keys, runs, leases, integration queue | per-repository metadata | shared when the repository is on PostgreSQL | unchanged: shared; the engine serializes writers |
| **Audit log = event feed** (`events/*`, SSE) | per-repository metadata | shared; subscribers poll it every 500 ms | unchanged (it is the durable outbox); `NOTIFY` makes it prompt |
| **Job queue and leases** (`jobs`, `claimed_by/until`) | per-repository metadata | shared; claim = one write transaction (`FOR UPDATE SKIP LOCKED` under the advisory lock) | unchanged |
| Objects | `.ag/store` or S3 | shared only on S3 | unchanged; replicas need S3 |
| Marker `<data_dir>/repos/<r>/.ag/config.json` | local disk | written by `create` | **materialized on demand** from the registry (`repos.config_json`) |
| Role-binding cache | process memory, 5 s TTL | per process | TTL **+ cluster invalidation** |
| Credential lookup | control DB per request (no cache) | — | unchanged, so revocation is immediate |
| Wake-ups of SSE / head long-polls / job claimers (`Notify`) | process memory | in-process only | + `NOTIFY` fan-out; polling stays the floor |
| Job hint set (`job_repos`) | process memory | rebuilt by scan | + `NOTIFY`; scan every 5 s |
| Token buckets (rate limits) | process memory | per process | **per replica** (documented, not shared) |
| Open repository pool, JWKS cache, trace exporter, metrics | process memory | per process | per replica |
| Readiness / shutdown | process | `/readyz` = control reachable | + draining, replica id, schema version |

## Decisions

1. **Same SQL layer, second backend.** The control plane uses the dialect-neutral seam of ADR-0004 (`?N`
   placeholders, `ON CONFLICT`, `BIGINT`), now exposed as `ag_storage::sqldb::Database` with caller-supplied migrations.
   `[control] url` / `url_env` selects PostgreSQL; absent, SQLite as before (same file, same `user_version`, same
   behaviour). Control migrations live in `migrations/control/{sqlite,postgres}/`, are numbered, compiled in, and
   listed in a test that fails on drift. SQLite keeps `PRAGMA user_version`; PostgreSQL keeps the version in its
   `meta` table. Rejected: a trait per control operation (a second implementation of ~30 methods for no gain).
2. **Control writes are single statements.** Every control mutation is one atomic statement (`INSERT … ON
   CONFLICT`, `UPDATE … WHERE`), so no cluster-wide write lock is taken and no read-modify-write exists; two
   replicas cannot lose each other's update. Multi-statement work (restore/migrate) uses `Database::write`.
3. **Startup is race-free and forward-only.** Migrations run in one transaction under a PostgreSQL advisory lock
   (n replicas starting together migrate once); a schema newer than the binary is refused. Control migrations are
   **additive** (expand/contract) so replicas of adjacent versions coexist during a rolling update. A
   replica that is already running does not re-check the schema.
4. **Durable state is the source of truth; `LISTEN/NOTIFY` only makes it prompt.** SSE and long-polls read the
   per-repository audit log (ordered, durable, `seq`-addressed), so *replay by event id works against any
   replica* by construction. Each replica holds one `LISTEN ag_cluster` connection; a mutating request publishes
   `events`/`jobs`/`bindings` notices that wake local waiters. A lost notice (listener reconnecting, `NOTIFY` is
   not durable) costs latency, never correctness: SSE polls every 500 ms, job claimers rescan every 5 s, the
   binding cache expires after 5 s, and a listener (re)connect triggers a full resync. Rejected: an outbox table
   in the control DB (duplicates the audit log), notify-only delivery (lossy, 8 kB payloads, breaks behind
   transaction-pooling proxies), Redis/NATS (a new dependency for what PostgreSQL already provides).
5. **Exactly-one job claim comes from the repository's own transaction**, not from the server: a claim is one
   `Meta::write` (advisory lock per repository schema + `SKIP LOCKED`), so replicas, workers and retries cannot
   double-claim; leases expire by wall-clock `claimed_until` stored in the database.
6. **Any replica opens any repository.** The registry stores the repository's `config.json`; a replica that has
   no local marker writes it (atomic rename) before opening — **only** for PostgreSQL metadata + S3 objects. A
   repository on local files is never materialized elsewhere (that would silently create an empty one).
7. **Credentials**: looked up in the control DB on every request (no cache) → revoked on one replica, rejected by all
   at once. **Role bindings**: cached ≤ 5 s, plus a cluster notice → effective immediately in the normal case,
   ≤ 5 s worst case. Open SSE streams re-check their credential every 5 s.
8. **Rate limits stay per replica** (a shared limiter would add a database write to every request). The effective
   limit is `replicas × limits.requests_per_second`; set it accordingly.
9. **Drain.** SIGTERM flips `/readyz` to 503 at once, waits `server.drain_secs`, then ends streams and finishes
   in-flight requests. `/healthz` stays a pure liveness check; `/readyz` also fails when the control database is unreachable.

## Invariants (each proved by a multi-process test)

Each line is a test in `crates/ag-server/tests/multi_replica.rs` that starts 2 to 5 real `ag-server` processes.

- **No lost update across replicas**: concurrent integrations through different replicas yield one contiguous reflog
  chain (`old_head` of entry *n+1* = `new_head` of entry *n*, versions 1…*n*), no head twice, every acknowledged change in the
  final tree, the audit chain verifies, `fsck` is clean, all replicas agree.
  `concurrent_integrations_through_three_replicas_never_lose_an_update`.
- **A killed replica loses no acknowledged write and double-applies nothing**; clients retry against another replica with
  their idempotency keys, and the killed replica rejoins. `killing_a_replica_mid_flight_loses_nothing_and_applies_nothing_twice`;
  an idempotency key is honoured by every replica: `an_idempotency_key_used_on_one_replica_is_honoured_by_the_others`.
- **A job enqueued through replica A is claimed by exactly one of several workers connected to B and C** (one attempt
  each). `a_job_queued_through_one_replica_is_claimed_exactly_once_by_workers_on_the_others`.
- **An SSE client on A sees an event caused through B, and `Last-Event-ID` / replay work on any replica.**
  `an_event_caused_through_one_replica_reaches_a_subscriber_on_another_and_replays_anywhere`.
- **A credential revoked on A is rejected by B at once** (measured 15-20 ms), a **revoked role takes effect on B within
  the bound of decision 7** (measured under 50 ms; the test fails above `bindings_ttl_ms` + 1 s), and an open stream is
  cut (measured 4.5 s, bound 5 s + a poll). `a_revoked_credential_and_a_revoked_role_stop_working_on_every_replica`.
- **A rolling restart (one replica at a time) never leaves the service unavailable** and loses no write; each replica
  drains (`/readyz` 503, `/healthz` 200) and exits 0. `a_rolling_restart_never_makes_the_service_unavailable_and_loses_no_write`.
- **Replicas that start together migrate once; a newer schema is refused.**
  `replicas_started_together_migrate_once_and_refuse_a_newer_schema`.
- **`/readyz` follows the control database, `/healthz` does not; a database that refuses the replicas gives retryable
  `unavailable`, and the replica recovers by itself.** `readiness_follows_the_control_database_and_liveness_does_not`
  (it found, and the fix has its own test, that a refused login surfaced as `internal`:
  `a_server_that_refuses_the_login_is_an_unavailable_database_not_an_internal_error`).
- **`migrate-control` moves credentials (incl. revoked and scoped), roles and the repository registry to PostgreSQL;
  fresh replicas serve them together with the repositories and their agent keys.**
  `migrate_control_turns_a_single_server_into_replicas_without_losing_credentials_roles_repos_or_agent_keys`.

## Design goals not yet proved

These are intended, reasoned about, and **not** covered by a test; do not read them as claims.

- Replicas of two *different* builds serving together during a rolling upgrade (decision 3: additive control migrations).
  The rolling-restart test restarts the same build.
- Behaviour across a PostgreSQL primary failover or a network partition (the tests lock out logins and terminate
  backends, they do not promote a standby), and with a slow rather than dead database.
- Throughput and latency under load: the tests prove correctness under concurrency, not capacity.
- The shipped Kubernetes overlay (`deploy/kubernetes/overlays/ha`) is validated as manifests (kustomize, kubeconform) and
  its configuration is loaded by this server in a test, but it has not been applied to a cluster.
- Connection budget under many open repositories (the operator sizes `max_connections`).

## Consequences

- ✅ Active-active serving, rolling upgrades, no local disk as source of truth (the `data_dir` is scratch).
- ✅ SQLite remains the zero-configuration default; nothing changes for single-node installs.
- ⚠️ PostgreSQL (and S3) are now single points of failure unless the operator runs them highly available.
- ⚠️ A repository is still served by one *writer at a time* (serialized per repository); replicas scale reads,
  connections and unrelated repositories, not one hot stream.
- ⚠️ Clock skew between replicas shows up in `agwp_event_delivery_lag_seconds`, job lease expiry and credential expiry; run NTP.
- ⚠️ A crash between initializing a repository's schema and registering it leaves an unregistered schema that
  must be dropped by hand.
- ⚠️ Connections: every replica may hold up to `repos.max_open × repos.pool_size` repository handles, each with its own
  small PostgreSQL pool; size `max_connections` (or lower `repos.max_open`) accordingly.
