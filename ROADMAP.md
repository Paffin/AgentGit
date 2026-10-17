# Roadmap

AgentGit follows [ADR-0001](docs/adr/0001-final-architecture.md): correctness first, narrow before wide.
This list is ordered by dependency, not by date, and it only lists what is **not** built. What exists is
described in the [architecture traceability](docs/architecture/adr-traceability.md).

## 0.1 — shipped (alpha)

Deterministic core with canonical formats and independent-implementation vectors · verified publication
and atomic CAS integration with idempotent, resumable intents · deterministic three-way merge · policy,
signed attestations, approvals, protected paths · agent identities, runs, tasks, leases with fencing,
signed provenance, conflict prediction, integration queue · workspaces with conflict markers · GC, fsck,
backup, tamper-evident audit · Git import/export · the `ag` CLI.

## Built since 0.1 (on `main`, unreleased)

- **The network layer.** `ag-server`: AGWP v1 over gRPC and HTTP/JSON on one port ([reference](docs/protocol/agwp-v1.md),
  generated [OpenAPI](docs/protocol/openapi.json)); OIDC and scoped, expiring credentials; per-repository RBAC;
  quotas and rate limits; live events (SSE) with replay; Prometheus metrics and OpenTelemetry traces.
- **PostgreSQL metadata and S3-compatible objects**, per repository, behind the existing seams.
- **Execution plane**: `ag-worker` claims validation jobs, runs the policy's command in an `ag-sandbox` sandbox
  (rootless namespaces, or a container) and submits signed attestations.
- **Remote workflow in the CLI**: `clone`, `fetch`, `pull`, `push`, `ag remote …`, offline use.
- **History**: rename-aware merge ([ADR-0005](docs/adr/0005-rename-aware-merge.md)), line-level blame that follows
  renames, rename display in `ag show`.
- **Clients**: Web UI, TypeScript and Python SDKs, the Go agent platform (scheduler, sessions, execution
  scheduler, conflict advisor, reviewers, semantic index).
- **Operations**: Dockerfile, Compose, Kubernetes, Prometheus / Grafana / OpenTelemetry files, a release
  workflow, scale measurements ([ADR-0006](docs/adr/0006-flat-trees-measured.md)).

## Next

Ordered by what unblocks the most.

- **Prove the deployment story.** Build and run the images, `docker compose up`, apply the manifests to a real
  cluster, cut a first `-rc` tag through the release workflow, and fix what that finds. Publish the SDKs to
  npm / PyPI and the images to a registry.
- **Server hardening that the integration work exposed.**
  - Bind credentials to a *run* (today: to an agent name), so concurrent runs of one agent cannot act for each other.
  - A *delegator* role that may mint credentials for `agent:*` only, so the orchestrator does not need `admin`.
  - Server-side finish/reap of tasks and runs by an operator (the orchestrator currently mints short-lived
    credentials to do it).
  - `integration.prepare` read-consistent with a job completing at the same moment.
  - Repository listings with real stream / changeset counts.
- **High availability.** Move the control database off a single SQLite file so more than one `ag-server`
  can serve; this is the main gap for multi-tenant hosting.
- **Execution plane**: verify the container sandbox and cgroup v2 limits against real runtimes; microVM
  isolation (Firecracker / gVisor) for hostile multi-tenant use; a sandbox on macOS; worker pools and labels;
  per-job log storage.
- **Scale** (when the triggers in ADR-0006 are met): hierarchical Merkle trees, an in-process cache of decoded
  trees, packed objects, incremental GC marking, streaming blobs.
- **Merge and history**: directory renames and copy detection; language-aware advisors that *propose*
  resolutions (never part of the correctness core), e.g. a model-backed `Advisor` behind the interface the
  orchestrator already has.
- **Web UI**: tablet and phone layouts verified by tests, a redirect-based OIDC flow, Firefox and WebKit in the
  end-to-end matrix.
- **Reproducible builds**: demonstrate bit-for-bit reproducibility across machines.

## Ideas we are deliberately not doing

Replacing Git for existing human workflows tomorrow; letting a model decide anything the deterministic core
guarantees; a global repository lock; hiding policy decisions inside prompts.

## How to influence this

Open an issue using the feature template and say which guarantee your idea touches; changes to formats,
hashing, merge semantics or the publication protocol need an ADR ([CONTRIBUTING](CONTRIBUTING.md)).
