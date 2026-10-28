# Roadmap

AgentGit follows [ADR-0001](docs/adr/0001-final-architecture.md): correctness first, narrow before wide.
The next-work list is ordered by dependency, not by date. Delivery status separates shipped
capabilities from development; implementation evidence lives in the
[architecture traceability](docs/architecture/adr-traceability.md).

## 0.1 — shipped (alpha)

Deterministic core with canonical formats and independent-implementation vectors · verified publication
and atomic CAS integration with idempotent, resumable intents · deterministic three-way merge · policy,
signed attestations, approvals, protected paths · agent identities, runs, tasks, leases with fencing,
signed provenance, conflict prediction, integration queue · workspaces with conflict markers · GC, fsck,
backup, tamper-evident audit · Git import/export · the `ag` CLI.

## 0.3 public beta — migration and guided adoption

The single-server Linux/SQLite/filesystem workflow is open to early adopters. Web and CLI migration preserve complete branch/tag history with a reconciliation report and retained source bundle. Hosting metadata migration, external MCP-host evidence, independent pilots and wider storage qualification remain separate open work; see the [beta programme](docs/community/beta.md).

## Built since 0.1 (through `v0.3.0-beta.5`)

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
- **Easy to pick up (on `main`, not in rc.2).** A Web UI a first-time user can use (Home, plain labels, blockers with their
  fix as a button, guided flows, rules presets, editing in the browser; proven by a task-based end-to-end test);
  `ag-server dev`, `ag login`, policy presets, `ag explain`; and a test gate under which a test cannot pass by not
  running (required components, a reviewed skip list, a report in every job).

## Beta exit criteria

The next release is scoped around one reliable workflow, not every deployment option. The
[beta programme](docs/community/beta.md) defines the supported target, measurable acceptance gates and pilot process.
Track work in the [beta milestone](https://github.com/Paffin/AgentGit/milestone/1).

## Delivery status

Keep published capabilities separate from development code. A changelog entry or a tag alone
is not proof that a downloadable archive has passed release verification.

| Baseline | Available work | Compatibility |
|---|---|---|
| Published `v0.3.0-beta.4` | Git migration with reconciliation, code search, multi-file editing, issues → tasks → assigned executor → run → change request, saved output, task/run history | Metadata schema 5; [release](https://github.com/Paffin/AgentGit/releases/tag/v0.3.0-beta.4) |
| Published `v0.3.0-beta.5` | Revision-bound review threads, replies and resolution across Web UI/CLI/MCP; thread filters and review-context handoff | Metadata schema 6; [release and verification](docs/operations/releasing.md#beta5-publication-evidence) |
| Development `main` after beta.5's tagged commit | Per-agent new-run admission controls, durable source links for follow-up tasks, and pre-run task refinement across API and clients | Metadata schema 7; [admission controls](docs/adr/0026-agent-run-admission.md), [task sources](docs/adr/0027-task-source-links.md) |

Back up before opening existing repositories with a newer schema. Rollback means restoring
that pre-upgrade backup, not running an older binary against the upgraded database.
The [traceability record](docs/architecture/adr-traceability.md) holds implementation evidence;
the [beta programme](docs/community/beta.md) holds the remaining qualification gates.

## Next

The product direction stays the same: people give bounded work to agents, inspect the exact
result, understand policy decisions and land a verified change. Migration must let an existing
team reach that workflow without losing its history or silently discarding hosting metadata.
Work in the following order; keep each deliverable usable across the relevant clients.

1. **Deliver the next development slice.** Beta.5 is published; its Linux archive checksum,
   provenance and upgrade to development schema 7 have been verified. Admission controls and
   task sources are already merged; they need their own subsequent release and schema-7 upgrade
   notice, not a replacement beta.5 tag.
2. **Finish the daily task and review workflow.** Queued-task refinement now preserves
   historical instructions and source links. Next add review suggestions,
   requested reviewers and policy-required thread resolution. Preserve drafts, exact-head
   checks and explicit confirmation when a user changes executable work or a review decision.
   Account-backed drafts and non-text conflict resolution remain open. Home currently polls
   visible-page attention data; live updates should preserve its complete-snapshot behaviour.
3. **Make GitHub/GitLab cutover complete.** Code history, branch/tag mapping, the original bundle
   and reconciliation already move through Web UI and CLI. Next design and implement resumable
   hosting-metadata migration: issues and comments first, then pull/merge-request references,
   releases and identity/permission mapping. Preview unsupported data and require deliberate
   choices before publication. LFS payloads and CI configurations need separate transfer/setup;
   source YAML alone is not a working validation pipeline. Keep Git export and rollback visible
   in the [migration guide](docs/guides/migrate-from-git.md).
4. **Qualify the real first-use experience.** Native archives already include the orchestrator;
   browser setup prepares scoped executor access. Record an end-to-end run in a real MCP host
   ([#50](https://github.com/Paffin/AgentGit/issues/50)), clean macOS published-archive installation
   ([#52](https://github.com/Paffin/AgentGit/issues/52)), a screen-reader session
   ([#53](https://github.com/Paffin/AgentGit/issues/53)) and a physical-phone session
   ([#55](https://github.com/Paffin/AgentGit/issues/55)). Recruit and learn from five external pilots
   ([#51](https://github.com/Paffin/AgentGit/issues/51)); automated browser runs do not substitute
   for these reports. Keep the dev sign-in exchange and advanced policy-form controls on the
   onboarding backlog.
5. **Finish distribution and widen support deliberately.** Verify Homebrew installation, add
   a Windows release archive, publish crates and SDKs when registry ownership/trusted publishing
   is configured, and qualify installation outside CI. A stable tag follows the
   [beta exit evidence](docs/community/beta.md#release-gates), not a count of shipped features.
6. **Qualify deployment and server follow-ups.** Compose and kind deployment already run in CI.
   Remaining evidence includes production-like ingress/storage, HA and Prometheus Operator
   overlays, PostgreSQL failover, mixed-build upgrades, replica reaping and load on PostgreSQL,
   S3 and TLS. Server work includes required job fencing, client addresses in audit entries,
   tree-cache sharing, same-stream writer scheduling and merge notes on the wire. Literal
   content search is shipped; copy detection and MCP search remain separate work.
7. **Scale and isolation when measurements justify them.** Hierarchical Merkle trees, packed
   objects, incremental GC and streaming blobs follow the triggers in
   [ADR-0006](docs/adr/0006-flat-trees-measured.md). VM-level isolation, native macOS/Windows
   sandboxes, worker pools/labels and per-job logs remain outside the initial beta commitment.

Keep checks proportional to each change: prove the affected user workflow, preserve policy
and recovery guarantees, and maintain the existing gates. Broader fuzzing, mutation and
performance qualification belong to their scheduled tracks, not every product iteration.

## Ideas we are deliberately not doing

Replacing Git for existing human workflows tomorrow; letting a model decide anything the deterministic core
guarantees; a global repository lock; hiding policy decisions inside prompts.

## How to influence this

Open an issue using the feature template and say which guarantee your idea touches; changes to formats,
hashing, merge semantics or the publication protocol need an ADR ([CONTRIBUTING](CONTRIBUTING.md)).
