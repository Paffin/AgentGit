# Changelog

All notable changes are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres to
[Semantic Versioning](https://semver.org/) — while `0.x`, minor versions may change behaviour, but
on-disk formats are versioned and refuse to open when newer than the build understands.

## [Unreleased]

### Added

- **`ag-sandbox`**: isolated execution of untrusted commands behind one `Sandbox` trait — a rootless Linux
  namespace sandbox (user, mount, PID, IPC, UTS and network namespaces; private read-only root; dropped
  capabilities; `no_new_privs`; a seccomp deny-list; hardened `/proc`; CPU, memory, process, file and output
  limits; optional cgroup v2), an OCI sandbox (`docker`/`podman`, digest-pinned images) and a limits-only
  process sandbox that reports itself as *not isolation*. Every run returns an `IsolationReport` that claims
  only what was set up, and an environment digest for binding attestations to it. See
  [the sandbox page](docs/architecture/sandbox.md) for what each mechanism does and does not stop.
- **`ag-worker`**: the execution-plane worker. Claims validation jobs from `ag-server`, verifies it has the
  exact candidate tree, runs the policy's command in a sandbox, signs an attestation with its own key and
  completes the job; keeps its lease by heartbeat; fails the job (not the tree) when the worker is at fault.
  See [the workers guide](docs/guides/workers.md).

- **`ag-server`**: the network server. AGWP v1 over gRPC and HTTP/JSON on one port (the protobuf file is the
  single source of truth; the HTTP/JSON API is described by a generated, drift-checked `openapi.json`);
  OIDC providers, scoped expiring credentials and the bootstrap token; per-repository RBAC; request
  quotas and rate limits on every operation; idempotency keys for repositories, objects, proposals,
  integrations, tasks, runs and leases; SSE events with replay and resume; `/healthz`, `/readyz`, bearer-protected
  `/metrics`; OTLP/HTTP trace export; graceful shutdown; the execution-plane job API; `ag-server healthcheck`
  and offline `admin` commands. See [the AGWP reference](docs/protocol/agwp-v1.md).
- **PostgreSQL metadata and S3-compatible object storage**, selectable per repository
  ([guide](docs/operations/postgres-and-s3.md), [ADR-0004](docs/adr/0004-postgres-and-s3-backends.md)).
- **Remote workflow in `ag`**: `clone`, `fetch`, `pull`, `push` and `ag remote …` over gRPC (new crate
  `ag-client`); credentials never stored in the repository's configuration
  ([guide](docs/guides/remote.md)).
- **Rename-aware merge** (`ag-merge/2`): a file moved on one side and edited on the other merges at its new
  path; `RenameRename` and `RenameDelete` conflicts ([ADR-0005](docs/adr/0005-rename-aware-merge.md)).
- **`ag blame`** (local, `ag remote blame`, and `history.blame` over the API): line-level, follows renames,
  reports truncation honestly; renames are shown by `ag show` ([guide](docs/guides/history.md)).
- **Web UI** (`apps/web`, Vue 3 + TypeScript): live board, proposals with four independent verdicts, history with
  blame, agents, administration; types generated from the OpenAPI document; unit tests and a Playwright
  suite that runs the built UI against a real server, with axe-core audits ([guide](docs/guides/web-ui.md)).
- **Client SDKs** for TypeScript (`@agentgit/client`) and Python (`agentgit-client`) covering all 91 operations,
  with idempotency-aware retries, deadlines, pagination, resumable SSE and Ed25519 provenance signing
  ([guide](docs/guides/sdks.md)). Not yet published to npm or PyPI.
- **Agent platform** (`services/agent-orchestrator`, Go): task scheduler with lease- and prediction-aware
  placement, agent sessions with short-lived scoped credentials, execution scheduler, conflict advisor,
  deterministic reviewers, incremental semantic index, admin API; it proposes and never integrates
  ([guide](docs/guides/orchestrator.md)).
- **Deployment and observability files** (`deploy/`): Dockerfile, Compose (PostgreSQL + MinIO), Kubernetes
  (kustomize, Prometheus Operator overlay), Prometheus rules, a Grafana dashboard, an OpenTelemetry
  collector config, a release workflow; checked by `scripts/check_deploy.py`
  ([deployment](docs/operations/deployment.md), [observability](docs/operations/observability.md),
  [releasing](docs/operations/releasing.md)).

### Changed

- Decoding a cached tree manifest is about 4× faster and a one-file three-way merge about 12× faster at 100 000
  files (change-driven merge; the previous algorithm is kept as the reference and as the fallback). Measurements
  and the conditions for hierarchical trees: [ADR-0006](docs/adr/0006-flat-trees-measured.md).
- `ag_merge::ALGORITHM_VERSION` is `ag-merge/2+ag-diff/1` (recorded in every merge changeset).
- The wire name of a file entry's type is `regular` or `symlink` (it said `file` on reads before the server
  was released); writes still accept `file`.

### Known limitations

- The sandboxes share the host kernel; they are not virtual machines. The OCI sandbox has only been tested
  against a fake runtime script, the cgroup v2 limits only against a fake directory, and there is no
  namespace sandbox on macOS.
- Memory and process limits are enforced by sampling (milliseconds apart) unless a cgroup is delegated, and a
  root worker without `--sandbox-uid` mapping loses the kernel's process limit.
- `ag-server` runs as a single process (the control database is one SQLite file); credentials are bound to an
  agent name rather than to a run; the `Idempotency-Key` is ignored by operations other than the creating
  ones listed in the AGWP reference.
- Nothing in `deploy/`, the container images or the release workflow has been run end to end by the author
  (see the deployment page for what is checked in CI); the SDKs are unpublished; the Web UI is tested in
  Chromium at desktop width only.
- Rename detection is per file (no directory renames or copies); blame follows renames but not copies, and
  for merge parents beyond the first only the same path.


## [0.1.0] — 2026-10-17

First public alpha.

### Added

- **Deterministic core**: SHA-256 domain-separated content IDs, strict canonical binary encoding,
  `ChangesetV1`, derived trees with delta/first-parent semantics, DAG algorithms (ancestors, merge bases),
  Myers diff, deterministic three-way merge for text and trees with typed conflicts. Formats verified
  against an independent Python implementation (`tests/conformance/`).
- **Storage**: filesystem object store (fsync + atomic rename, verified on read, self-healing, GC-safe
  deletes) and a SQLite metadata store (WAL, `synchronous=FULL`) with changeset index, streams + reflog,
  intents, transactional outbox, hash-chained audit log, policies, proposals, reviews, attestations, pins,
  workspaces and agent state.
- **Engine** (`ag-integrate`): verified publication; integration with candidate computation outside the
  lock and policy + compare-and-swap inside one transaction; idempotent, resumable operations with fault
  injection at every step; integration queue; garbage collection that fails closed and cannot lose a
  concurrently re-referenced object; `ag verify`; rebuildable, sparse (keyframe) derived tree cache;
  consistent verified backups.
- **Policy**: versioned per-stream JSON policy; trusted issuers; signed attestations bound to tree, policy
  version and environment; approvals bound to the reviewed head; protected paths; agent autonomy envelope;
  provenance requirements.
- **Agent-native**: agent identities and runs, tasks with dependencies, soft and exclusive leases with
  fencing tokens, signed provenance stored beside immutable changesets, path-level conflict prediction.
- **Workspaces**: isolated directories, snapshots, sync with conflict markers and resolution.
- **Git bridge**: import and export through git plumbing; trees round-trip with identical SHAs.
- **CLI** `ag` with stable exit codes (0 ok, 1 error, 2 usage, 3 blocked, 4 refused) and `--json`.
- **Docs and tooling**: architecture, guides, operations, generated CLI reference, ADR traceability,
  documentation consistency checks, runnable end-to-end demo, CI.

### Known limitations

Local mode trusts the local user; no sandbox for `ag validate run`; flat tree manifests; no rename
detection; no network server. See [ROADMAP.md](ROADMAP.md) and
[docs/architecture/security.md](docs/architecture/security.md).

[Unreleased]: https://github.com/Paffin/AgentGit/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Paffin/AgentGit/releases/tag/v0.1.0
