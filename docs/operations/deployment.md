# Deploying `ag-server`

`ag-server` is one process: AGWP over gRPC and HTTP/JSON on a single port, plus `/healthz`, `/readyz`,
`/metrics` and `/openapi.json`. Everything lives under one data directory (a SQLite *control database*
with credentials, role bindings and the repository registry; per-repository data unless you configure
PostgreSQL and S3 — see [postgres-and-s3.md](postgres-and-s3.md)). To try a server on your machine first, with
roles, credentials, a policy and an agent, follow [Run a team server](../tutorials/team-server.md).

Three ready-made ways to run it, all under [`deploy/`](../../deploy):

| | For | Files |
|---|---|---|
| **Binary** | trying it, small teams | `ag-server serve --config ag-server.toml` (the release archives contain it; there is no ready-made systemd unit — write one around that command) |
| **Docker Compose** | one host with PostgreSQL and SeaweedFS (S3) | [`deploy/compose/`](../../deploy/compose) |
| **Kubernetes** | a cluster | [`deploy/kubernetes/`](../../deploy/kubernetes) (kustomize) |

## What has been verified

Be clear about what "ready-made" means here.

| Checked in CI on every change (`ci.yml`: `scripts/check_deploy.py --strict`, `cargo test -p agentgit-server --test deploy_configs`) | **Not verified — treat as a starting point** |
|---|---|
| Every YAML/JSON file parses | The alert thresholds are reasoned defaults, not tuned on real traffic |
| `kustomize build` of every kustomization (base, `ci`, `prometheus-operator`, `ha`); `kubeconform --strict` against Kubernetes 1.30 schemas (and the Prometheus Operator CRDs) | The `prometheus-operator` overlay has never been applied to a cluster (the Operator's CRDs are only schema-checked) |
| `promtool check rules` / `check config` | Behaviour under load; upgrades and failover of the *cluster*. The server's multi-replica behaviour (kill -9, rolling restart, no lost update) is proved by process tests ([High availability](high-availability.md)); the `ha` overlay has not been applied to a cluster |
| `docker compose config` with and without secrets (it refuses to start without them) | A cluster other than kind: ingress, TLS termination, a real storage class, a CNI that enforces the NetworkPolicy |
| Every shipped `ag-server.toml` loads and validates with this server | Published images: the `v0.2.0-rc.1` run pushed `ghcr.io/paffin/agentgit-server:0.2.0-rc.1` (multi-arch, anonymously pullable; checked through the registry API, never started from there) but that tag produced no release; the default is now `0.2.0-rc.2`, which resolves once that tag is pushed; until then build the image from `deploy/docker/Dockerfile` (the `ci` overlay shows how to use it in kind). The default tag is the workspace version, which is what `release.yml` pushes; `check_deploy.py` keeps the three defaults equal to it |
| Every metric used by alerts and the dashboard is one the server exports | The release workflow has only been rehearsed without publishing; the first tag is its first real publish ([releasing.md](releasing.md#what-is-not-proven)) |
| `ag-server healthcheck`, bearer-protected `/metrics` and trace export are covered by integration tests | |

| Run for real in the [`Containers`](../../.github/workflows/containers.yml) workflow (every change that can alter an image or `deploy/`) | |
|---|---|
| `deploy/docker/Dockerfile` builds both targets; the images are under 100 MiB, run as uid 65532 with no shell and a health check, and report the commit they were built from; the server image embeds the Web UI (the log says so, `/` serves the app, its scripts are served pre-compressed) | |
| The server image runs with a read-only root and no capabilities and passes the end-to-end smoke (`scripts/e2e_smoke.py`: repository, roles, credentials, `ag clone`, snapshot, `ag push`, integrate, read back over HTTP, metrics), again after a restart | |
| A rebuild on a fresh runner restores the dependency, UI and compile layers from the cache | |
| `docker compose up` of `deploy/compose` (server, PostgreSQL, SeaweedFS as the S3 store): the same smoke, rows in the repository's PostgreSQL schema and objects in the bucket, a restart that loses nothing, and the `observability` profile (Prometheus scrapes with the token file, rules load, Grafana and the collector answer) | |
| `deploy/kubernetes` (base through the `ci` overlay) applied to a kind cluster (Kubernetes 1.33): `restricted` Pod Security, no restarts, the same smoke through a port-forward, a rollout restart that keeps the data | |

If something here fails on your cluster, that is a bug in these files — please report it.

## Secrets

| Variable | Purpose |
|---|---|
| `AG_BOOTSTRAP_TOKEN` | the first administrator (use 32 or more random characters; the server refuses to start with fewer than 16). Use it to create users and roles, then mint scoped credentials and stop using it |
| `AG_METRICS_TOKEN` | bearer token for `/metrics`. **Set it**: without one the server logs a warning when it listens on a non-loopback address, because anyone who can reach the port can read the metrics |
| `AG_PG_URL`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN` | only with PostgreSQL / S3 storage |
| `OTEL_EXPORTER_OTLP_HEADERS` | only if the trace collector needs headers (`authorization=Bearer …`) |

None of them belongs in a config file, an image or Git. The configuration names the *environment
variables*; the values come from your secret manager, a Kubernetes Secret or Compose's `.env`.

## TLS

The server refuses to listen on a non-loopback address without TLS unless `allow_insecure_remote = true`.
Choose one:

* **Terminate TLS in front** (ingress, load balancer, mesh, reverse proxy) and set
  `allow_insecure_remote = true` — what the shipped container configs do. Keep the port off the public
  network (the Compose file publishes on `127.0.0.1` only; the Kubernetes NetworkPolicy admits only the
  namespace). Make sure the proxy speaks HTTP/2 to the backend, or gRPC clients will not work.
* **TLS in the server**: set `tls_cert` and `tls_key` under `[server]`. gRPC and HTTPS then share the
  certificate.

## Docker Compose

```sh
cd deploy/compose
cp .env.example .env            # fill in every value (openssl rand -hex 32)
docker compose up -d --build
curl -fsS http://127.0.0.1:8080/readyz
```

This starts the server, PostgreSQL 16 and SeaweedFS (an S3-compatible store) and creates the bucket. Repositories created on this server
keep **metadata in their own PostgreSQL schema** and **objects under their own prefix of the bucket**.
Create the first user:

```sh
docker compose exec ag-server /usr/local/bin/ag-server admin --data-dir /data grant \
    --subject user:alice --repo '*' --role admin
docker compose exec ag-server /usr/local/bin/ag-server admin --data-dir /data mint \
    --subject user:alice --ttl 86400
```

(`admin` works on the data directory directly, also while the server is stopped.)

The image and tag are `AG_IMAGE` / `AG_VERSION` in `.env` (default `ghcr.io/paffin/agentgit-server` / the workspace version). The container health check calls `ag-server healthcheck`, whose URL is `AG_HEALTHCHECK_URL` (default `http://127.0.0.1:8080`): set it in the container environment when `server.listen` or TLS differ.

Add dashboards and tracing with `docker compose --profile observability up -d` after writing the metrics
token to `secrets/metrics_token` — see [observability.md](observability.md).

## Kubernetes

```sh
kubectl apply -f deploy/kubernetes/base/namespace.yaml
kubectl -n agentgit create secret generic agentgit \
    --from-literal=AG_BOOTSTRAP_TOKEN="$(openssl rand -hex 32)" \
    --from-literal=AG_METRICS_TOKEN="$(openssl rand -hex 32)"
kubectl apply -k deploy/kubernetes/base           # or overlays/prometheus-operator
kubectl -n agentgit rollout status deploy/ag-server
```

The image defaults to the workspace version (`0.2.0-rc.2`, the `X.Y.Z` tag `release.yml` pushes; `:latest` exists only for a stable release and moves). Override it with `images:` in your own kustomization (`newName`/`newTag`, for a private registry or another release), and expose the Service through
your ingress/gateway with TLS. The pod runs as uid 65532 with a read-only root filesystem, no capabilities,
the `RuntimeDefault` seccomp profile and no service-account token, under the `restricted` Pod Security
level.

The `prometheus-operator` overlay also adds an ingress rule to the NetworkPolicy for the namespace `monitoring`
(a patch in its `kustomization.yaml`): change the namespace there to where your Prometheus runs, and add a rule for
your ingress controller (the base file shows an example). Its ServiceMonitor sets `jobLabel`, so the series carry
`job="agentgit"`, which is what `alerts.yml` selects; `check_deploy.py` verifies that.

### Scaling

**The default is exactly one replica.** The control database is a SQLite file on the volume, and without
PostgreSQL/S3 repositories are files on the volume too, so two pods cannot share state (`strategy: Recreate` with a
`ReadWriteOnce` volume enforces this). Plan for a restart taking seconds (the startup probe allows about a minute)
and size the single pod for your peak: the server sheds load with `503 unavailable` + `Retry-After` when
`limits.max_concurrent_requests` is reached, and clients retry.

**Several replicas** are supported when the control database is on PostgreSQL (`[control] url_env`) and the
repositories are on PostgreSQL + S3: no shared disk, rolling upgrades, a replica can be killed at any time. That
topology, its guarantees (each one a multi-process test), its failure modes, the move from a single server
(`ag-server admin migrate-control`) and its limits (PostgreSQL stays a single point of failure unless it is itself
highly available; rate limits are per replica) are in [High availability](high-availability.md). The Kubernetes
overlay is `deploy/kubernetes/overlays/ha`; the Compose stack stays single-host.

### Backups

* Repositories on local storage or PostgreSQL/S3: see [durability.md](durability.md) and
  [postgres-and-s3.md](postgres-and-s3.md#operating-it).
* The control database (`/data/control.db`) holds credentials' *hashes*, role bindings and the repository
  registry. Back up the volume with snapshots, or copy the file while the server is stopped. Losing it
  means re-creating users and roles; it does not lose any repository's history. With the control database on
  PostgreSQL, `ag-server admin backup-control --out FILE` writes a logical dump (credential hashes included: mode 0600)
  that works for either backend, and `restore-control` reads it; `pg_dump` of the control schema is the other way
  ([High availability](high-availability.md#moving-from-one-server-to-replicas)).

## Stale agent runs

An agent that crashes leaves its run `running`, its task `in_progress` and its leases held. The server ends such
runs only when it is told to: an operator (`delegator` or `admin`) calls `POST /v1/repos/{owner}/{name}/runs/reap`
(the Go orchestrator does it for the runs of sessions it lost), or you let the server do it for every repository:

```toml
[agents]
run_stale_after_secs = 900   # a running run without a heartbeat for this long is abandoned
min_stale_after_secs = 30    # a reap request below this is refused (a typo must not end every live run)
reap_interval_secs = 0       # > 0: reap every repository this often, audited as `system:reaper`; 0 = on request only
```

`ag-server admin reap-runs <owner/name> [--stale-after-secs N] [--dry-run]` does the same offline. A reap releases
the leases of the ended runs, revokes the credentials bound to them and writes `run.abandon` / `lease.reap` audit
entries; the counter is `agwp_agent_runs_reaped_total` ([observability](observability.md)). Several replicas may
reap at once: a reap that finds nothing stale changes nothing. See the
[agents guide](../guides/agents.md#8-against-a-server-credentials-delegation-and-operators).

## Upgrading

On-disk formats are versioned; a build refuses to open data written by a newer version, and older data is
migrated forward. Take a backup, deploy the new image, watch `/readyz`. Roll back by redeploying the old
image **only if you have not run a version that migrated the data**; otherwise restore the backup.

The control database moved to schema **3** with run-bound credentials ([ADR-0010](../adr/0010-run-bound-credentials.md)): two
additive columns on `credentials`. A build that only knows schema 2 refuses the migrated database instead of reading a
run-bound credential as an ordinary one, so across replicas start a new build first and roll back from a backup.
