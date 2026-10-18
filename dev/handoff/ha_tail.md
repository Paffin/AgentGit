# Handoff: ha_tail (the multi-replica server, finished)

Design: [ADR-0007](../../docs/adr/0007-multi-replica-server.md). Operator's guide: [high-availability.md](../../docs/operations/high-availability.md).
Part 1 (control plane on PostgreSQL, ADR-0007) landed earlier; its handoff is folded into this one and the ADR.

## State

Landed on `main` (commits below). The multi-process tests from `wip/ha-track-rebased` are on main, rebased over the
current server, and **pass** locally on moto and on SeaweedFS; CI runs them in the `backends` job.

* `crates/ag-server/tests/multi_replica.rs` (10 tests, one process-level scenario each) and `scripts/ha-test.sh`:
  no lost update across replicas (6 clients x 5 changes through 3 replicas: one contiguous reflog, audit chain verifies,
  `fsck` clean, all replicas agree); `kill -9` of a replica while clients are mid-flight (the test asserts the kill hit
  between the 6th and the 30th change); idempotency keys honoured across replicas; exactly-once job claim (4 workers on 2
  replicas, jobs queued through a third; `attempts == 1`); SSE event seen across replicas + replay + `Last-Event-ID` on a
  third; bounded revocation (credential 15-20 ms, role < 50 ms, open stream cut after 4.5 s); rolling restart under reads
  and writes (SIGTERM, drain, exit 0); five replicas migrating one schema + refusal of a newer schema; readiness follows the
  control database; `migrate-control` with real data.
* `migrate_control_turns_a_single_server_into_replicas_without_losing_credentials_roles_repos_or_agent_keys`: a real
  single server (SQLite control plane, repositories on PostgreSQL + S3) with six credentials (one revoked, one scoped to a
  repository), five role bindings, a repository with history and an agent with a registered key is stopped;
  `ag-server admin migrate-control --from sqlite --to postgres` (same-backend refused, second run refused, `--replace`
  works, source untouched); two replicas on empty data directories then authenticate the old tokens as the same
  identities, still reject the revoked one, keep the scope, list the same credentials and roles, serve the repository and
  the agent key, and continue the history.
* Tests are serialised inside the binary (`serial()`), so `cargo test --workspace` in the backends job is as reliable
  as `scripts/ha-test.sh` (`--test-threads=1`). A failing test stops its helper threads instead of hanging
  (`StopOnDrop`).
* **Server bug found and fixed**: a PostgreSQL that refuses a connection attempt (login not permitted SQLSTATE 28000,
  too many connections 53300, ...) was classified as an opaque statement error, so a control database that locked the
  replicas out gave `500 internal` instead of retryable `503 unavailable`. `ag-storage` `meta/pg/pool.rs`: errors at
  connect time are `ConnectionLost`. Test: `a_server_that_refuses_the_login_is_an_unavailable_database_not_an_internal_error`
  (`crates/ag-storage/tests/postgres.rs`) and the readiness test.
* Test fixes needed to make the old branch pass (all test-side): concurrent clients need a seeded stream (otherwise each
  client writes its own root commit and the histories are rightly refused as unrelated), role revocation is a query-string
  `DELETE`, audit verification needs an operator token, SSE events carry `payload.details` (not `payload_json`), the
  schema version is read from the code, credential listing is per subject.
* `docs/operations/high-availability.md` (topology, guarantees with test names, failure modes, move from one server,
  Kubernetes, upgrade, operating, honest limits, not verified); ADR-0007 now says which invariants are proved and keeps a
  "Design goals not yet proved" section; `deployment.md`, `adr-traceability.md`, `security.md`, FAQ, comparison, docs
  index, mkdocs nav updated.
* `deploy/kubernetes/overlays/ha`: 3 replicas, RollingUpdate `maxUnavailable: 0`, PodDisruptionBudget, preferred
  anti-affinity + zone spread, readiness every 3 s, `terminationGracePeriodSeconds: 60`, `drain_secs = 10`, `emptyDir`
  data, ConfigMap with `[control]` and the storage template. `scripts/check_deploy.py --strict` (kustomize + kubeconform
  1.30, promtool) passes locally and is part of the `deployment files` job; `deploy_configs.rs` loads its
  configuration with this server and checks `drain_secs < terminationGracePeriodSeconds`.
* `[control]`/replica settings validation test (`config::tests::the_control_and_replica_settings_are_validated`).

## Evidence

* Local, before pushing: `scripts/ha-test.sh` 10/10 on moto (three runs; the last after rebasing) and 10/10 with
  `AG_S3_SERVER=seaweedfs`; `scripts/pg-test.sh cargo test -p ag-storage --test postgres` 18/18;
  `cargo test -p ag-server -p ag-storage` green; clippy `-D warnings` on both; `cargo fmt --all --check`;
  `scripts/check_docs.py`; `scripts/check_deploy.py --strict` with kustomize, kubeconform, promtool.
* CI: see the section "CI" at the end (filled in after the runs finished).

## Not verified

* The `ha` overlay has not been applied to a cluster (manifests are built and schema-validated only); no load balancer was
  in front of the processes in the tests (the test clients do the failover themselves).
* Mixed builds during a rolling upgrade, rollback after a control migration (documented as a restore), PostgreSQL primary
  failover or partition (the tests lock out logins and terminate backends), slow rather than dead dependencies.
* No load numbers; connection budget under many open repositories is arithmetic in the doc, not measured.
* No Compose profile for replicas (not cheap to do honestly: the single `ag-server` service publishes the port and mounts
  the SQLite configuration; the Compose stack is documented as single-host). Replicas on one machine: `scripts/ha-test.sh`.
* Timing assertions (event lag < 2 s, role revocation < `bindings_ttl_ms` + 1 s, reads never stall > 10 s) have margins
  of an order of magnitude over what was measured, but a very slow runner could in principle trip them.

## Audit triage (areas: server HA tail)

* Confirmed items about `control.rs` (fail-closed `scope_json`/`quota_json`, `all_bindings` without a row cap) and
  `main.rs` (`admin` honouring `--config`) were already fixed on main by `server_audit` (re-read: `credential_from` and
  `repo_from` return `Corrupt`; `admin --config` exists and is exercised by `admin_config.rs` and the migration test).
* Confirmed, **not mine and left for `clients_ops`** (owner of `deploy/`): `deploy/kubernetes/base/networkpolicy.yaml`
  admits only the same namespace (Prometheus in another namespace is dropped), and the generated PrometheusRule hardcodes
  `job="agentgit"` while a ServiceMonitor labels `job="ag-server"` (add `jobLabel: app.kubernetes.io/part-of`). The `ha`
  overlay inherits both. Contested: Compose `.env`/secret duplication (`deploy/compose/compose.yaml:102`): not touched.
* Nothing contested in the HA code paths.

## Next steps

1. Apply `overlays/ha` to a kind cluster with a real PostgreSQL (and S3) in the Containers workflow and run the multi-replica
   smoke through the Service (that also covers the load balancer gap).
2. A mixed-build rolling-upgrade test once there is a second released build (or a test build with a higher control
   schema that adds a column).
3. A PostgreSQL standby promotion test (Patroni or `pg_ctl promote`) for the failover claim.
4. Replica load test and published numbers; measure the connection budget with hundreds of open repositories.

## CI

Commit `4831dd1` (the tests, fix, docs, overlay, CI wiring), all runs green:
CI https://github.com/Paffin/AgentGit/actions/runs/39124631598 (job "PostgreSQL + S3 backends": the workspace run on
moto, the SeaweedFS run, and the two new steps "Multi-replica ag-server on PostgreSQL + S3" on moto and on SeaweedFS,
all success; job "deployment files": `check_deploy.py --strict` with the `ha` overlay and `deploy_configs`, success),
Containers https://github.com/Paffin/AgentGit/actions/runs/39124631602, Windows https://github.com/Paffin/AgentGit/actions/runs/39124631585,
Docs https://github.com/Paffin/AgentGit/actions/runs/39124631559, Security, Quality, CodeQL, Fuzz, Web UI, Packaging: success.
The last commit (this handoff and the `[control]` validation test) is listed in the report.
