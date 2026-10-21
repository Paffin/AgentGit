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

## Built since 0.1 (in the `v0.2.0-rc.2` pre-release and on `main`)

Each item is described, with what is and is not verified, in the [changelog](CHANGELOG.md) and the linked page.

- **The network layer.** `ag-server`: AGWP v1 over gRPC and HTTP/JSON on one port ([reference](docs/protocol/agwp-v1.md),
  generated [OpenAPI](docs/protocol/openapi.json)); OIDC and scoped, expiring credentials; per-repository RBAC;
  quotas (object bytes atomic on every write path) and rate limits; live events (SSE) with replay; Prometheus
  metrics and OpenTelemetry traces.
- **Server hardening.** Credentials bound to one run, the `delegator` role, operator finish and reap of tasks and
  runs (locally and `ag agent reap --remote`, with a leased periodic reaper), `agent.operate` for acting on another
  agent's run, atomic stream and proposal quotas, keyset paging with indexes, trusted proxies and a limit on failed
  logins, idempotency records that expire, a global in-flight byte budget, derive status and dangling references in
  `ag doctor`, `/readyz` and metrics, a fencing token for job claims, fail-closed control rows
  ([ADR-0010](docs/adr/0010-run-bound-credentials.md),
  [ADR-0011](docs/adr/0011-proposal-paging-indexes-and-background-leases.md),
  [ADR-0012](docs/adr/0012-job-claim-fencing.md)); the audit of mocks, stubs and hardcoded values (`dev/audit/`)
  triaged and fixed by the work packages (see `dev/handoff/`).
- **Performance work**: an integration costs 3 durable commits instead of 7-8, a decoded-tree cache with a memory
  budget, a bounded retry for same-stream contention
  ([numbers and conditions](docs/architecture/performance.md)).
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
  CI (`Containers`), a release workflow that published the pre-release `v0.2.0-rc.2` (archives, SBOMs, checksums,
  multi-arch images, attestations; what was verified: [releasing](docs/operations/releasing.md#evidence)), install
  script, Homebrew and Scoop templates, Windows builds and tests in CI.
- **Measurements**: engine benchmarks and a server load driver (`ag-bench`), numbers published with their conditions
  ([engine](docs/architecture/performance.md), [server](docs/operations/performance.md)); scale
  ([ADR-0006](docs/adr/0006-flat-trees-measured.md)).
- **Security and quality gates**: supply-chain scans, CodeQL, fuzzing, coverage floors, mutation testing,
  compatibility fixtures ([Testing](docs/architecture/testing.md)).
- **Documentation**: a strict-built MkDocs site, tutorials executed in CI, FAQ, glossary, comparison, generated
  reference pages, doc and no-stub checks in CI.

## Next

Only what is not done. The order comes from the first outside validation
([report](dev/feedback/validation-2026-10-21.md), 2026-10-21): the engine works, the product is hard to pick up. Verdict
of the reviewer: about 7/10 in convenience, good for an agent pilot, not yet a replacement for GitLab in daily team work.
Hurdles named: setup needs Rust, a build, UI dependencies and hand-made tokens; too many new words (proposal, attestation,
provenance, lease, fencing); daily work stays in the CLI; the Web UI is not self-explanatory; orchestration needs
configuration. The first four items answer that, in this order.

1. **A Web UI a new user can use without the docs.**
   A written heuristic review of the live UI against five jobs (start a repository, give an agent work, review a change,
   unblock a change, land it); navigation by task instead of by object; a home page that says what needs attention now;
   plain labels with the AgentGit word in a tooltip; every blocker states the reason and offers the fix as a button
   (request validation, ask a reviewer, take the lease); guided flows with a progress checklist; a policy editor with
   presets instead of raw JSON; editing files and proposing a change from the browser, conflict resolution included;
   empty states that say what to do. Proven by task-based end-to-end tests: a fresh user finishes each of the five jobs
   in the browser using only what the screen says.
2. **Running in five minutes without Rust, Node or hand-made tokens.**
   A zero-configuration start (`ag-server dev`: data directory, bootstrap administrator, demo repository, a login link
   that signs in), prebuilt binaries and a Docker one-liner in the first lines of the README, a setup that makes the first
   agent credential in one click, helper scripts that run on the system Python 3.9.
3. **Fewer concepts up front.** Policy presets (solo, team, agents), `ag explain <proposal>` that turns a refusal into
   the next commands, a "Git to AgentGit" cheat sheet, concepts introduced only when the screen needs them.
4. **Orchestration that works out of the box.** The orchestrator in the release archives and as an image, a ready
   template "task, agent, proposal" with a real model executor verified live, "give this task to an agent" in the UI.
5. **Tests that cannot pass by doing nothing.** An external-component test that cannot reach its component (a container
   runtime, Keycloak, a model, PostgreSQL, the server binary for the Go suite) must fail in CI unless it is on a
   reviewed skip list; a skip report in every job; the Go suite fails without `AG_SERVER_BIN`.
6. **Finish the release.** Verify what rc.2 left unverified (archive attestations, macOS and aarch64 archives outside CI,
   the Homebrew formula); add an SBOM for `ag-mcp` and a Windows archive; cut a stable tag (until then `install.sh` needs
   `--version`); publish the 21 `agentgit-<x>` crates and the SDKs. Only the owner can push tags and publish.
7. **Prove what is only argued.** The `ha` and Prometheus Operator overlays and the images on a real cluster (ingress,
   storage class); mixed builds during a rolling upgrade (metadata schema 3 needs the new build first); a PostgreSQL
   failover; the periodic reaper with real replicas on PostgreSQL and S3; load on PostgreSQL, S3, TLS and several
   replicas, and the tree cache on the disk backend; first results of the nightly fuzz and the weekly mutation runs;
   `perf.yml` on a GitHub runner; a second OIDC provider (Entra, Okta, Auth0); a screen-reader and a real-phone pass.
8. **Server follow-ups.** A server option that requires the job-claim fencing token; the resolved client address in the
   hash-chained audit entries; `Arc<Tree>` in the tree cache; a per-stream in-process queue so same-stream writers take
   turns instead of retrying; merge notes on the AGWP wire (and so in the Web UI); copy search over the server and MCP.
9. **Scale and isolation, when needed.** Hierarchical Merkle trees, packed objects, incremental GC marking, streaming
   blobs (triggers in [ADR-0006](docs/adr/0006-flat-trees-measured.md)); microVM isolation (Firecracker, gVisor, Kata)
   for hostile multi-tenant use; a sandbox on macOS and Windows; worker pools and labels; per-job log storage.

## Ideas we are deliberately not doing

Replacing Git for existing human workflows tomorrow; letting a model decide anything the deterministic core
guarantees; a global repository lock; hiding policy decisions inside prompts.

## How to influence this

Open an issue using the feature template and say which guarantee your idea touches; changes to formats,
hashing, merge semantics or the publication protocol need an ADR ([CONTRIBUTING](CONTRIBUTING.md)).
