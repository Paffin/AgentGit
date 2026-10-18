# High availability: several `ag-server` replicas

Two or more `ag-server` processes can serve one installation at the same time, behind a load balancer, with no
shared disk. The design is [ADR-0007](../adr/0007-multi-replica-server.md); this page is the operator's view: what the
topology is, what is guaranteed (and by which test), what happens when something fails, how to move to it and how
to upgrade it, and where the limits are.

**What "verified" means here.** The guarantees below are exercised by
[`crates/ag-server/tests/multi_replica.rs`](https://github.com/Paffin/AgentGit/blob/main/crates/ag-server/tests/multi_replica.rs):
every test starts 2 to 5 copies of the real `ag-server` binary, with real sockets, a real PostgreSQL and a real S3
implementation (moto in the main CI run, SeaweedFS in a second one; the same binary and protocol you deploy), and
kills or restarts some of them. CI runs them in the
[`backends` job](https://github.com/Paffin/AgentGit/blob/main/.github/workflows/ci.yml); locally,
`scripts/ha-test.sh` (it starts a throwaway PostgreSQL and S3 server). What the tests do **not** cover is listed in
[Not verified](#not-verified).

## Topology

```text
                       ┌────────────────────┐
   clients, SDKs,      │   load balancer    │   health: GET /readyz
   ag-worker, ag-mcp ─▶│ (Ingress / Service)│   no stickiness needed
                       └─────────┬──────────┘
                ┌────────────────┼────────────────┐
                ▼                ▼                ▼
          ag-server #1     ag-server #2     ag-server #3     identical, stateless (data_dir = scratch)
                │                │                │
        ┌───────┴────────────────┴────────────────┴───────┐
        │ PostgreSQL                                       │  control schema (credentials, roles, registry)
        │   LISTEN/NOTIFY wakes replicas; polling is the   │  + one schema per repository (streams, reflog,
        │   floor, so a lost notice costs latency only     │    proposals, audit log = event feed, job queue)
        └──────────────────────────────────────────────────┘
        ┌──────────────────────────────────────────────────┐
        │ S3-compatible bucket: objects, one prefix/repo   │
        └──────────────────────────────────────────────────┘
```

Requirements, all of them:

* **Control plane on PostgreSQL**: `[control] url_env = "AG_PG_URL"` (and `schema`). Without it the control
  database is the SQLite file `<data_dir>/control.db`, which only one process may use.
* **Repositories on PostgreSQL metadata and S3 objects** (`[repos.storage]`, see [PostgreSQL and
  S3](postgres-and-s3.md)). A repository on local files is served only by the host that has the files; a replica never
  materialises it elsewhere (that would silently create an empty one). `migrate-control` lists such repositories.
* **A load balancer that probes `/readyz`** (not `/healthz`) and lets long responses through: SSE streams send a
  keep-alive every 25 s, job claims and head watches long-poll for up to their `wait_ms`. gRPC and HTTP/JSON share the
  port (h2c). Clients need no session affinity: an SSE reconnect with `Last-Event-ID` can go to any replica.
* **Synchronised clocks** (NTP): job leases, credential expiry and `agwp_event_delivery_lag_seconds` compare
  timestamps written by different replicas.

A minimal configuration is [`deploy/kubernetes/overlays/ha/ag-server.toml`](https://github.com/Paffin/AgentGit/blob/main/deploy/kubernetes/overlays/ha/ag-server.toml);
the settings that matter:

| Setting | Default | For replicas |
|---|---|---|
| `[control] url_env` / `url` | unset (SQLite) | set: this is what makes it a cluster |
| `[control] schema` | `ag_control` | any lowercase identifier; replicas that share it share the control plane |
| `[control] pool_size` | 16 | connections per replica to the control schema |
| `[control] bindings_ttl_ms` | 5000 | upper bound of role-revocation visibility on another replica (usually it is immediate) |
| `[control] startup_timeout_secs` | 30 | how long a starting replica retries an unreachable database before it exits |
| `[server] drain_secs` | 0 | seconds a terminating replica keeps serving after `/readyz` turns 503; set it above the balancer's probe period and below the orchestrator's grace period |
| `[server] replica_id` | `$HOSTNAME` | name in logs and `agwp_replica_info` |

## What is guaranteed

Each row is a test that starts real processes and fails if the guarantee does not hold.

| Guarantee | How | Test (`multi_replica.rs`) |
|---|---|---|
| **No lost update across replicas.** Concurrent integrations through different replicas produce one contiguous reflog (entry *n+1* starts where entry *n* ended, versions 1…*n*), every acknowledged change is in the final tree, no head appears twice, the audit hash chain verifies, `fsck` is clean, and every replica reports the same history | the stream compare-and-swap and the per-repository advisory lock live in PostgreSQL, not in the server | `concurrent_integrations_through_three_replicas_never_lose_an_update` (6 clients × 5 changes through 3 replicas) |
| **`kill -9` of a replica in flight loses nothing and applies nothing twice.** Clients move to another replica and retry with their idempotency keys; the chain is still exactly the acknowledged changes; the killed replica rejoins and serves the same history | idempotency keys and operations are rows in PostgreSQL | `killing_a_replica_mid_flight_loses_nothing_and_applies_nothing_twice` (the kill lands after at least 6 and before all 30 changes) |
| **An idempotency key means the same on every replica.** A retry that lands elsewhere gets the original answer; the same key with another request is refused everywhere | the key table is in the control schema | `an_idempotency_key_used_on_one_replica_is_honoured_by_the_others` |
| **A job is claimed exactly once.** Jobs queued through replica A are claimed by workers connected to B and C, each job once (`attempts == 1`), none lost | a claim is one write transaction on the repository's metadata | `a_job_queued_through_one_replica_is_claimed_exactly_once_by_workers_on_the_others` |
| **Events reach subscribers on other replicas, and replay works anywhere.** An SSE client on A sees an integration made through B within the polling bound, the event ids agree, `events/replay` returns the same events from every replica, and an SSE reconnect with `Last-Event-ID` to a third replica resumes right after the id | the event feed is a view of the durable audit log; `NOTIFY` only makes it prompt | `an_event_caused_through_one_replica_reaches_a_subscriber_on_another_and_replays_anywhere` |
| **Revocation is bounded.** A revoked credential is rejected by every replica at once (measured: 15 to 20 ms after the call returned); an open event stream on another replica is cut (measured: 4.5 s, the stream re-checks its credential every 5 s); a revoked role is effective on every replica (measured: under 50 ms; bound `bindings_ttl_ms`) | credentials are looked up per request, role bindings are cached for `bindings_ttl_ms` and dropped on a cluster notice | `a_revoked_credential_and_a_revoked_role_stop_working_on_every_replica` |
| **A rolling restart is invisible to clients.** Each replica in turn gets SIGTERM: `/readyz` turns 503 within the drain, `/healthz` stays 200, the process exits 0, it comes back; meanwhile readers never see an error through a failing-over client (longest gap between successes in CI: 0.1 to 0.3 s) and three writers lose no change | drain + client retry | `a_rolling_restart_never_makes_the_service_unavailable_and_loses_no_write` |
| **Replicas that start together migrate the control schema once, and refuse a schema newer than themselves** (the log says which version) | advisory-locked migration; version in the `meta` table | `replicas_started_together_migrate_once_and_refuse_a_newer_schema` |
| **Readiness follows the control database; liveness does not.** When PostgreSQL refuses the replicas, `/readyz` turns 503 (`control database unavailable`), `/healthz` stays 200, requests answer `503 unavailable` with `retryable: true` (not an opaque `internal`), and the replica recovers by itself when the database returns | pool reconnects; connection-level failures map to `unavailable` | `readiness_follows_the_control_database_and_liveness_does_not`, and in `ag-storage` `a_server_that_refuses_the_login_is_an_unavailable_database_not_an_internal_error` |
| **A single server becomes replicas without losing credentials, roles, repositories or agent keys** (below) | logical dump and import | `migrate_control_turns_a_single_server_into_replicas_without_losing_credentials_roles_repos_or_agent_keys` |

## Failure modes

| What fails | What clients see | What the operator does |
|---|---|---|
| One replica crashes or is killed | requests in flight on it fail at the transport; a retrying client (or the load balancer) uses another replica. Nothing acknowledged is lost, nothing is applied twice if the client retried with its idempotency key | nothing; the orchestrator restarts it. It needs no recovery step: it holds no state |
| One replica is slow or partitioned from PostgreSQL | its `/readyz` fails after the control probe times out (2 s); the balancer stops sending to it. In flight work may end `deadline_exceeded` (the outcome is unknown: retry with the same key) | find out why it cannot reach the database |
| PostgreSQL is down or refuses logins | every replica: `/readyz` 503 and every request that needs the database answers `503 unavailable` (`retryable: true`); repositories on PostgreSQL cannot be read or written meanwhile. Liveness stays green so nothing is restarted pointlessly. A client that retries `unavailable` carries on when the database is back | restore the database. Replicas reconnect on their own, the event listener resyncs |
| The S3 bucket is down | operations that read or write objects fail; which metadata-only operations still answer was not tested | restore the bucket |
| `LISTEN/NOTIFY` is lost (listener reconnecting, PgBouncer in transaction mode) | latency only: SSE polls every 500 ms, job claimers rescan every 5 s, role bindings expire after `bindings_ttl_ms`. `agwp_cluster_listener_up` is 0 while it is down | fix the connection; use a session-mode or direct connection for the listener |
| Two replicas start with different binaries during an upgrade | the older build refuses a control schema newer than it knows ([Upgrading](#upgrading)) | upgrade in order, below |
| A crash between creating a repository's schema and registering it | an unregistered PostgreSQL schema stays behind (harmless, no repository points at it) | drop it by hand; the repository can be created again |

## Moving from one server to replicas

Starting point: a single server whose control database is `<data_dir>/control.db`, repositories on PostgreSQL + S3
(the Compose stack, or `deploy/kubernetes` with the optional storage). The move is tested end to end
(`migrate_control_turns_a_single_server_into_replicas_without_losing_credentials_roles_repos_or_agent_keys`: credentials
including a revoked one and a repository-scoped one, role bindings, a repository with history, an agent with a registered
key; two fresh replicas on empty data directories serve all of it).

1. Add `[control] url_env = "AG_PG_URL"` (and a `schema`) to the configuration. Do not start any replica yet.
2. Stop the single server (a clean stop; the move reads its SQLite file).
3. Move the control plane, using that configuration and the old `data_dir`:

   ```bash
   ag-server admin --config /etc/agentgit/ag-server.toml backup-control --out /secure/control-before.json   # optional safety copy
   ag-server admin --config /etc/agentgit/ag-server.toml migrate-control --from sqlite --to postgres
   ```

   It prints how many credentials, role bindings and repositories it moved, refuses a target that already holds data
   unless you add `--replace`, and warns about repositories whose data is on the old host's disk (they cannot be
   served by replicas). The SQLite file is left untouched: **rolling back is starting the old configuration again**,
   as long as nothing was changed through the replicas meanwhile.
4. Start the replicas (any number, in any order, on fresh data directories). `/readyz` turns 200 when the control
   schema is migrated and reachable.
5. Check `agwp_control_schema_version` and `agwp_replica_info` on every replica, and `GET /v1/whoami` with an existing
   token through each.

`backup-control` and `restore-control` work on either backend and write a logical JSON dump (mode 0600; it contains
credential hashes: treat it like the database). The PostgreSQL schemas themselves are backed up with `pg_dump` (see
[PostgreSQL and S3](postgres-and-s3.md#operating-it)).

## Kubernetes

[`deploy/kubernetes/overlays/ha`](https://github.com/Paffin/AgentGit/tree/main/deploy/kubernetes/overlays/ha):
three replicas, `RollingUpdate` with `maxUnavailable: 0` and `maxSurge: 1`, a `PodDisruptionBudget`
(`maxUnavailable: 1`), preferred anti-affinity by node and a topology spread by zone, readiness on `/readyz` every 3 s,
`terminationGracePeriodSeconds: 60` with `drain_secs = 10`, no volume claim (the data directory is an `emptyDir`), and a
ConfigMap with `[control]` and the storage template. You provide PostgreSQL, the bucket and the `agentgit` Secret
(`AG_PG_URL`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` besides the two tokens), and edit the bucket in
`ag-server.toml`. CI builds the overlay with kustomize and validates it with `kubeconform --strict`
(`scripts/check_deploy.py`, job `deployment files`), and a test checks that the configuration loads with this server and
that `drain_secs` fits the grace period (`deploy_configs.rs`). **It has not been applied to a cluster**; the
multi-process tests above are the proof of the server's behaviour, not of the manifests. There is no Compose profile:
Compose is a single-host stack (see [Deployment](deployment.md#scaling)); to try replicas on one machine run
`scripts/ha-test.sh` or start several `ag-server serve --config` processes on one PostgreSQL.

## Upgrading

* **Order.** Control schema migrations are additive and run at startup under a PostgreSQL advisory lock. Deploy the new
  build with the rolling update above: the first new replica migrates; replicas already running keep serving (a running
  replica does not re-check the schema); an *old* build that starts afterwards refuses the newer schema with a clear
  message instead of misreading it. Check the release notes for the schema version (`agwp_control_schema_version`).
* **Rollback after a migration is a restore**, not a redeploy: take `backup-control` (and a `pg_dump`) first. Rolling
  back before any replica of the new build started needs nothing.
* **Not tested:** two *different* builds serving at once during the roll. The rolling-restart test restarts the same
  build. Additive migrations are designed for it (ADR-0007, decision 3) and the one in the current release adds
  columns that a build that is already running never reads, but that is reasoning, not a test; do the first upgrade of a production cluster
  in a window you can afford.
* Repository schemas (per-repository metadata) migrate when a repository is opened; the same advisory lock serialises
  replicas.

## Operating it

* **Metrics** (per replica; scrape each pod): `agwp_replica_info{replica,…}`, `agwp_control_schema_version`,
  `agwp_draining`, `agwp_cluster_listener_up` (alert if 0 for minutes), `agwp_cluster_notices_total`,
  `agwp_cluster_resyncs_total`, `agwp_event_delivery_lag_seconds` ([Observability](observability.md)). `/readyz` is the
  balancer's signal; `/healthz` is for restarts.
* **Connection budget.** Each replica may hold `control.pool_size` connections plus, for each repository it has open,
  up to `repos.pool_size`, for at most `repos.max_open` repositories (defaults 16, 4 and 256). The worst case is
  `replicas × (control.pool_size + repos.max_open × repos.pool_size)`, far above a default PostgreSQL
  `max_connections`. Connections open on demand, so a typical replica holds few; size `max_connections` for your
  active-repository count or lower `repos.max_open` / `repos.pool_size`. PgBouncer in *transaction* mode does not suit
  the notice listener (session mode or a direct connection does). The multi-replica tests run with `pool_size`
  2 to 6; the worst case was not measured under load.
* **Rate limits are per replica.** The effective limit is `replicas × limits.requests_per_second` (likewise
  `repo_requests_per_second` and `max_concurrent_requests`); set the per-replica value to the share you want. A shared
  limiter would add a database write to every request, so it does not exist.

## Honest limits

* **PostgreSQL is a single point of failure** unless you run it highly available (a managed service with failover,
  Patroni, an operator). Replicas multiply the application tier, not the database; so is the S3 bucket.
* **One writer at a time per repository.** Writers of one repository are serialised by its advisory lock. Replicas
  spread reads, connections and unrelated repositories, not one hot stream.
* **A failover of PostgreSQL** is an outage for the seconds it takes plus the time clients need to retry; in-flight
  writes may report `deadline_exceeded` (unknown outcome) and must be retried with their idempotency keys. This
  has been tested by cutting the replicas' logins, not by promoting a standby.
* Role changes on another replica are usually effective at once and at worst after `bindings_ttl_ms`; an SSE stream
  notices a revoked credential within 5 s.
* Quota counters (`object_bytes`) are atomic across replicas; stream and proposal counts are checked just before the
  insert, so concurrent creates through different replicas can overshoot by the number in flight.
* Clock skew between replicas distorts the delivery-lag metric and shortens or lengthens job leases and credential
  lifetimes by the skew.

## Not verified

* The Kubernetes overlay and any load balancer in front of it: validated as manifests only.
* A PostgreSQL primary failover, a network partition between a replica and the database (the tests lock out the login
  and terminate backends), a database or bucket that is slow rather than down.
* Mixed-build rolling upgrades (above) and a rollback after migration.
* Load: the tests prove correctness under concurrency (up to 6 writer clients, 3 replicas), not throughput; there are no
  published numbers.
* Amazon S3 itself, a managed PostgreSQL, PgBouncer and other proxies.
