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

Each item is described, with what is and is not verified, in the [changelog](CHANGELOG.md) and the linked page.

- **The network layer.** `ag-server`: AGWP v1 over gRPC and HTTP/JSON on one port ([reference](docs/protocol/agwp-v1.md),
  generated [OpenAPI](docs/protocol/openapi.json)); OIDC and scoped, expiring credentials; per-repository RBAC;
  quotas (object bytes atomic on every write path) and rate limits; live events (SSE) with replay; Prometheus
  metrics and OpenTelemetry traces.
- **Server hardening.** Credentials bound to one run, the `delegator` role, operator finish and reap of tasks and
  runs, a read-consistent `integration.prepare`, real repository counts, fail-closed control rows
  ([ADR-0010](docs/adr/0010-run-bound-credentials.md)); the audit of mocks, stubs and hardcoded values
  (`dev/audit/`) triaged and fixed by the work packages (see `dev/handoff/`).
- **Several replicas.** Control plane on PostgreSQL, tested with real processes against PostgreSQL and S3
  ([ADR-0007](docs/adr/0007-multi-replica-server.md), [guide](docs/operations/high-availability.md)), Kubernetes `ha` overlay.
- **PostgreSQL metadata and S3-compatible objects**, per repository, tested against a real S3 server (SeaweedFS) and moto.
- **Execution plane**: `ag-worker` claims validation jobs, runs the policy's command in an `ag-sandbox` sandbox and
  submits signed attestations; the namespace and OCI sandboxes are proven in CI against real Docker, Podman and a
  delegated cgroup v2.
- **Remote workflow in the CLI**: `clone`, `fetch`, `pull`, `push`, `ag remote …`, offline use; `ag doctor`,
  help topics, completions and man pages.
- **History**: rename-aware merge ([ADR-0005](docs/adr/0005-rename-aware-merge.md)), line-level blame that follows
  renames.
- **Clients**: the Web UI (served by `ag-server`; Chromium, Firefox, WebKit, tablet and phone; OIDC against a real
  Keycloak), TypeScript and Python SDKs, `ag-mcp` for agents ([ADR-0008](docs/adr/0008-mcp-server.md)), and the Go agent
  platform (no simulated agent; optional model-backed advisor, reviewers and index).
- **Operations and distribution**: Dockerfile, Compose, Kubernetes, Prometheus / Grafana / OpenTelemetry files run in
  CI (`Containers`), a release workflow rehearsed end to end (nothing published), install script, Homebrew and Scoop
  templates, Windows builds and tests in CI.
- **Measurements**: engine benchmarks and a server load driver (`ag-bench`), numbers published with their conditions
  ([engine](docs/architecture/performance.md), [server](docs/operations/performance.md)); scale
  ([ADR-0006](docs/adr/0006-flat-trees-measured.md)).
- **Security and quality gates**: supply-chain scans, CodeQL, fuzzing, coverage floors, mutation testing,
  compatibility fixtures ([Testing](docs/architecture/testing.md)).
- **Documentation**: a strict-built MkDocs site, tutorials executed in CI, FAQ, glossary, comparison, generated
  reference pages, doc and no-stub checks in CI.

## Next

Only what is not done. Ordered by what unblocks the most.

- **Owner actions that only the repository owner can take.** Enable GitHub Pages (Settings, Pages, Source: GitHub
  Actions) so the docs site deploys for the first time; switch on the dependency graph and private vulnerability
  reporting; triage the Dependabot pull requests and dismiss fixed CodeQL alerts; delete leftover remote branches;
  run the release rehearsal from `main` in the Actions UI; decide the crates.io naming scheme.
- **Cut a first `-rc` tag** and fix what it finds. Nothing is published yet: images, archives, the SDKs (npm, PyPI)
  and the crates. Add a Windows archive to the release matrix.
- **Prove what is only argued.** The `ha` and Prometheus Operator overlays and the images on a real cluster (ingress,
  storage class); mixed builds during a rolling upgrade; a PostgreSQL failover; measure PostgreSQL, S3, TLS and several
  replicas under load; first results of the nightly fuzz and the weekly mutation runs; the opt-in live model step in CI.
- **Finish server hardening.**
  - Require `agent.operate` for acting on another agent's run (today a writer can; it is attributed, not prevented).
  - Atomic open-stream and open-proposal quotas (needs an engine hook), SQL filters and keyset paging for
    `list_proposals`, a global in-flight byte budget, trusted-proxy handling for `remote`, a TTL for idempotency records.
  - A remote `ag agent reap`; the periodic reaper and several-replica reaping beyond a single-server test.
  - Show `derive_status` and the `dangling_*` kinds in `ag doctor`, `/readyz` and metrics; queue fencing tokens
    (needs a schema and protocol change, hence an ADR).
- **Performance.** Batch the writes of one integration and an in-process cache of decoded trees with a memory budget;
  fewer `aborted` answers under same-stream contention; a `Retry-After` header that matches `retry_after_ms`; run
  `perf.yml` on a GitHub runner.
- **Execution plane**: microVM isolation (Firecracker / gVisor, Kata) for hostile multi-tenant use; a sandbox on macOS
  and Windows; other runtimes and images, SELinux hosts; worker pools and labels; per-job log storage; a scheduled run
  of the runtime workflow to notice new runner images.
- **Scale** (when the triggers in ADR-0006 are met): hierarchical Merkle trees, packed objects, incremental GC marking,
  streaming blobs.
- **Merge and history**: directory renames and copy detection.
- **Clients and agents.** Run `ag-mcp` inside the real hosts and with more models; other MCP transports; a second
  OIDC provider (Entra, Okta, Auth0) and a screen-reader and real-device pass for the Web UI; model-backed
  advisors with other models and providers, and the Docker image of the orchestrator against a server in a container.
- **Documentation.** Tutorials on Windows, an external-link checker, completions and man pages in the release
  archives, the first real Pages deploy.
- **Security gates.** Mutation-test `ag-agent` and the server's authorisation code; fuzz the HTTP layer and the CLI
  parser; explain the 46.6% coverage of `ag-mcp`; measure `llvm-cov` where the PostgreSQL and S3 backends run.
- **Reproducible builds**: demonstrate bit-for-bit reproducibility across machines.

## Ideas we are deliberately not doing

Replacing Git for existing human workflows tomorrow; letting a model decide anything the deterministic core
guarantees; a global repository lock; hiding policy decisions inside prompts.

## How to influence this

Open an issue using the feature template and say which guarantee your idea touches; changes to formats,
hashing, merge semantics or the publication protocol need an ADR ([CONTRIBUTING](CONTRIBUTING.md)).
