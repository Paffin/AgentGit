# ADR traceability

[ADR-0001](../adr/0001-final-architecture.md) (in Russian) is the architecture this project is built to.
This page maps its promises to the code and tests and says plainly what is not built yet.
Legend: ✅ implemented and tested · 🟡 partly · 🔜 planned (see the [roadmap](../../ROADMAP.md)) · ↔ intentionally different (see [ADR-0002](../adr/0002-embedded-engine-first.md)).

## §1 — what was fixed

| Problem → decision | Status | Evidence |
|---|---|---|
| Tree vs Delta → canonical changeset DAG, trees are verifiable projections | ✅ | `ag-changeset`, `ag-tree`; `derived_trees_are_disposable_and_reindex_rebuilds_them` |
| No strict atomicity → durable staging + transactional CAS | ✅ | [Publication & integration](publication-and-integration.md) |
| Objects unavailable after publish → durable-readiness check before publish | ✅ | `publish_verify`; `crash_at_every_step_recovers_to_exactly_one_integration` |
| Incomplete merge model → first-parent / resulting-tree semantics | ✅ | [Merge semantics](merge-semantics.md) |
| Lost changes in races → CAS + mandatory revalidation | ✅ | `attestation_cannot_be_reused_after_the_stream_moves`, concurrency tests |
| Unreliable GC → reachability + grace + retention roots | ✅ | `gc.rs`, GC tests |
| Unsafe agents → sandboxed executors without direct storage access | 🟡 | agents cannot integrate by default and hold no storage rights; checks run in an [`ag-sandbox`](sandbox.md) sandbox through `ag-worker` (rootless namespaces, tested from inside on a Linux runner and on a delegated cgroup v2; the OCI backend tested from inside the container against real Docker and Podman in CI; not a VM, and not a microVM either); `ag validate run` stays explicitly unsandboxed |
| Forged CI → signed attestations bound to the exact state | ✅ | `ag-policy`; policy tests |
| Undefined consistency → strong metadata, eventual derived indexes | ✅ | `Meta` (SQLite) + outbox-driven derivation |
| Premature microservices → modular monolith + separate execution plane | ✅ | modular crates, in-process engine ✓; the execution plane is a separate process (`ag-worker`) that talks to `ag-server` over AGWP |
| Unclear offline mode → online-first MVP | ↔ | local-first embedded engine instead (ADR-0002) |
| Overpromising → SLOs after benchmarks | ✅ | no SLOs are claimed; [indicative numbers](storage.md#scaling-notes-indicative) only |

## §2 — principles

| # | Principle | Status |
|---|---|---|
| 1 | VCS works without an LLM | ✅ nothing in the repository depends on a model |
| 2 | History is immutable | ✅ content-addressed, no update path |
| 3 | Publication is atomic for readers | ✅ |
| 4 | No global repository lock | ✅ per-stream CAS; SQLite serializes only the short commit |
| 5 | Every merge is reproducible | ✅ versioned algorithm, deterministic, tested |
| 6 | The agent is not a trusted party | ✅ policy, leases, signed provenance; 🟡 sandboxed checks share the host kernel ([details](sandbox.md)) |
| 7 | Git is an external adapter | ✅ `ag-git` |
| 8 | Every step is observable | ✅ reflog, audit chain, outbox, `ag trace`, OpenTelemetry traces and Prometheus metrics ([observability](../operations/observability.md)) |

## §3–§5 — system, model, publication

| Item | Status | Notes |
|---|---|---|
| SCM engine in Rust, deterministic | ✅ | |
| Object store, changeset DAG, tree resolver, diff/merge, integration/CAS, integrity/GC | ✅ | |
| PostgreSQL metadata | ✅ | per repository, same transactional surface as SQLite; the whole suite runs on both ([guide](../operations/postgres-and-s3.md), [ADR-0004](../adr/0004-postgres-and-s3-backends.md)). The server's control database is SQLite by default, or a PostgreSQL schema shared by several replicas ([ADR-0007](../adr/0007-multi-replica-server.md), [high availability](../operations/high-availability.md)) |
| S3 / MinIO object storage | ✅ | `S3ObjectStore` behind the same trait; the store's suite runs in CI against a real S3 server (SeaweedFS, signature-checking) and against moto (a signature-checking re-implementation), not against Amazon S3 or MinIO itself |
| Several `ag-server` replicas behind a load balancer | ✅ | control plane on PostgreSQL, cluster notices, drain, `/readyz`; six invariants proved by tests that start real server processes on a real PostgreSQL and S3 server (no lost update, `kill -9`, exactly-once job claim, events and replay across replicas, bounded revocation, rolling restart) in the `backends` CI job ([ADR-0007](../adr/0007-multi-replica-server.md), [guide](../operations/high-availability.md)). Not proved: mixed builds during a roll, a PostgreSQL failover, load, the Kubernetes overlay on a cluster |
| Derived workers: trees ✅; path history ✅ (path index); blame ✅ (computed on demand, follows renames) | ✅ | [History guide](../guides/history.md); renames in merge: [ADR-0005](../adr/0005-rename-aware-merge.md), directory renames: [ADR-0013](../adr/0013-directory-renames.md) |
| Outbox | ✅ | at-least-once; the one in-process consumer is tree derivation, run inline after each commit (no background dispatcher; failures are recorded and retried by the next pass, and shown by `ag doctor`, `/readyz` and the `agwp_derive_*` metrics) |
| Isolated execution plane | 🟡 | `ag-worker` + [`ag-sandbox`](sandbox.md): claims jobs, verifies the exact tree, runs the check isolated, signs; [guide](../guides/workers.md). The OCI backend is checked against real Docker and Podman in CI. Not yet: VM-level isolation, Windows workers, worker pools/labels |
| Agent Platform in Go (orchestrator, sessions, advisor, semantic index) | ✅ | `services/agent-orchestrator`, tested against the real server ([guide](../guides/orchestrator.md)). Trust boundary (§3): it holds a scoped credential, and `TestOrchestratorNeverIntegrates` fails on any call to integrate, set policy, grant roles, create repositories or approve. Advisor, reviewers and semantic index are deterministic heuristics by default and can be switched to a model (any OpenAI-compatible endpoint you configure); models only propose comments. No simulated agent ships; the optional live path was run once, by hand, with one model |
| MCP server for agents (`ag-mcp`) | ✅ | stdio, role-gated tools, `integrate` only behind `--enable-integrate`, no administration; protocol, CLI and end-to-end suites against a real in-process server ([ADR-0008](../adr/0008-mcp-server.md), [guide](../guides/mcp.md)). Not run inside Claude Code, Claude Desktop or the Agent SDKs |
| SHA-256 domain-separated IDs; custom canonical binary format; protobuf is transport only | ✅ | + independent-implementation vectors |
| Crash recovery, object availability, unknown commit outcome | ✅ | tests above |
| GC by generations | 🟡 | age-based grace period + roots; no separate generations |
| Disaster recovery with consistent DB/object snapshots and reconciliation | ✅ | `ag backup` (DB first, GC held off, verified), `ag verify` |

## §6 — merge and concurrency

| Item | Status |
|---|---|
| Different files / different regions → automatic; same region → conflict | ✅ |
| API-signature / behaviour analysis, security-policy changes get stronger approval | 🟡 protected paths with extra approvals ✅; API analysis 🔜 |
| Mergeable / Validated / Approved / Integrated as independent states | ✅ |
| Integration pseudo-code (merge → validate → transactional CAS) | ✅ as specified |

## §7 — agent-native features

| Feature | Status | Where |
|---|---|---|
| Agent identity (per agent and per run) | ✅ | `ag agent register/start` |
| Task ownership | ✅ | `ag task`, runs claim tasks |
| Virtual workspace (isolated, no full checkout required) | ✅ | `ag` workspaces are directory-backed; over the server, virtual workspaces need no checkout (`POST …/workspaces`, `agents_flow` and `history_api` tests) |
| Dependency awareness | ✅ | task dependencies, cycle-free, enforced at run start |
| Conflict prediction | ✅ | `ag predict` (path level) |
| Integration queue | ✅ | `ag queue` |
| Background work with several replicas | ✅ | the periodic run reaper takes a `run-reaper` lease in the control database, so one replica reaps (`only_the_replica_holding_the_reaper_lease_reaps`, `replicas_racing_for_an_expired_lease_get_exactly_one_winner`, SQLite and PostgreSQL); `ag agent reap --remote` does it on demand. Time-based lease; not tested with real replicas ([ADR-0011](../adr/0011-proposal-paging-indexes-and-background-leases.md)) |
| Proposal listing at scale | ✅ | keyset paging with SQL filters; metadata schema 3 adds the indexes (`every_proposal_page_is_read_in_index_order_without_sorting` on SQLite and PostgreSQL); older builds refuse schema 3 ([ADR-0011](../adr/0011-proposal-paging-indexes-and-background-leases.md)) |
| Provenance | ✅ | signed, separate from changesets |
| Autonomous review | 🟡 | reviews by agents are possible and policy decides whether they count; the orchestrator's reviewers (heuristic or model-backed) only post comments, never approve |
| Policy-bound autonomy | ✅ | `agent_autonomy` |
| Replay & trace | 🟡 | `ag trace`, audit chain, `events.replay` and the SSE subscription (`api_contract` tests) |
| Soft leases; fencing tokens for exclusivity | ✅ | exclusive file leases carry fencing tokens (`fencing_tokens_are_strictly_increasing_and_reject_stale_holders`); execution-plane job claims carry an optional one, the claim number, so a paused worker cannot act on a claim handed out again ([ADR-0012](../adr/0012-job-claim-fencing.md); `a_claim_number_fences_a_stale_claimant_of_the_same_name`, `a_stale_claim_of_the_same_worker_is_fenced_off`). Opt-in per request: no server option requires it yet |

## §8 — security

| Measure | Status |
|---|---|
| OIDC users; scoped short-lived agent credentials | ✅ `ag-server` (`security.rs`: OIDC, expiring/revocable/scoped credentials that can never exceed their minter). A credential can be bound to one run (`scope.run_id`: it acts only as that run and dies with it; the orchestrator and `ag-mcp` bind every run), the `delegator` role mints credentials for `agent:*` only, and operators finish tasks and reap dead runs with audit entries; starting, heartbeating or finishing another agent's run needs `agent.operate` (`acting_on_another_agents_run_needs_agent_operate_and_a_writer_does_not_have_it`) — [ADR-0010](../adr/0010-run-bound-credentials.md); a credential minted without the binding is still the agent for every run |
| RBAC/ABAC by repo/stream/operation | ✅ roles per repository pattern × credential scope (repos, streams, operations) on the server; policy lists inside the engine; no authentication in local mode |
| Sandbox with CPU/RAM/network/FS limits | 🟡 `ag-sandbox`: all five enforced and tested from inside the namespace sandbox; memory/task limits are sampled (not kernel-exact) unless a cgroup is delegated; see [sandbox](sandbox.md) |
| No standing secrets for untrusted code | 🟡 the worker's key and credential are never visible to the sandboxed check (tested); `validate run` only clears the environment |
| Signed CI attestations | ✅ |
| Prompt-injection hardening | ✅ data is never interpreted; terminal output sanitized |
| Limits on blobs, ops, DAG depth, requests | ✅ request size (HTTP and gRPC alike), concurrency, per-credential rate limits and repository quotas on the server: total bytes charged on every write path (`quota.rs`; `quotas.rs`, `api_honesty.rs`), bytes per object, streams, open proposals. Stream and proposal counts are taken in the transaction that inserts the row, so racing creates cannot pass the limit (`racing_creates_cannot_pass_the_stream_and_open_proposal_limits` on SQLite and PostgreSQL; engine tests in `ag-integrate/tests/quotas.rs`) |
| Supply-chain and testing gates | ✅ cargo-deny, npm audit, pip-audit, govulncheck, CodeQL, pinned actions, SBOMs; 20 fuzz targets, coverage floors, mutation testing of five core crates ([Testing](testing.md), [supply chain](../operations/supply-chain.md)). The nightly fuzz and weekly mutation runs have not yet run on GitHub; no external audit |
| Independent, tamper-evident audit | ✅ hash chain (not yet externally anchored) |
| Path traversal, symlink escape, zip bombs, object substitution | ✅ (no archive handling exists) |
| Separate powers for agent, reviewer, admin | ✅ (and a delegator that hands out agent credentials without administrator rights; a test fails if a new administrator-only capability is delegable) |

## §9 — AGWP v1

✅ gRPC and HTTP/JSON on one port, WebSocket not needed (SSE for events): [reference](../protocol/agwp-v1.md), [overview and mapping](../protocol/README.md), generated [OpenAPI](../protocol/openapi.json) checked against the router in CI.

## §10–§11 — stack and UI

Rust core ✅ · CLI (clap) ✅ (Linux, macOS and Windows in CI) · Vue Web UI ✅ (served by `ag-server`; Chromium, Firefox, WebKit in CI; [guide](../guides/web-ui.md)) · Go orchestrator ✅ · SDKs ✅ (TypeScript, Python; unpublished) · OpenTelemetry (traces) / Prometheus ✅ ·
NATS 🔜 (events use SSE and the audit log) · Docker Compose/Kubernetes 🟡 (the image, the Compose stack and a kind deployment run end to end in the `Containers` workflow; no real cluster, ingress or storage class, the HA overlay and the Prometheus Operator overlay are not applied to one — [deployment](../operations/deployment.md)). The ADR's text is cut off at §11 (Web UI); the rest was never received.


## ADR-0014 — reviewed-head precondition

`Repository::review_at_head` compares the optional inspected head inside the metadata write transaction. `a_review_cannot_approve_a_head_the_reviewer_did_not_see` exercises the HTTP refusal and successful current-head review. The Web UI preserves unfinished text and negotiates `proposals.review_head`; legacy callers omitting the field retain current-head behavior.


## ADR 0015 — revision-bound content search

The new read-only `tree.search` operation uses the existing `TreeRead` authorization, immutable tree and blob interfaces. Focused HTTP checks cover bounded continuation, skipped content, cursor mismatch and repository isolation; `code-search.spec.ts` exercises UI-created content, pagination and line navigation. See [ADR 0015](../adr/0015-bounded-revision-code-search.md). No index-latency or complete binary/large-file search claim is made.

## ADR0033–0035 — durable task execution and completion preconditions

`crates/ag-agent/tests/agents.rs` covers durable attempt admission, rollback and preservation
of state/audit after a stale policy update. `crates/ag-server/tests/task_attempts.rs` exercises
the wire contract, stale generation rejection and current-generation completion. The real
server/shell-executor scheduler scenarios cover restart, operator policy changes during work
and a late first session while a second run owns the task. Capability tests refuse an older
server before sending a conditional completion.

## Development review workflow — ADR-0036 through ADR-0039

These capabilities are implemented in the development branch and remain separate from beta.7's published schema8.

| Capability | Status | Evidence and remaining scope |
|---|---|---|
| Requested reviewers | ✅ | [ADR-0036](../adr/0036-requested-reviewers.md); author-only inspected-list update, exact review permissions; API/CLI/SDK/MCP/Web tests. Routing never grants approval/access. |
| Revision-pinned virtual sync | ✅ | [ADR-0037](../adr/0037-pinned-workspace-sync.md); final branch/head checks, exact blob/type/mode choices; HTTP/gRPC and browser regressions. Multi-path structural choices remain open. |
| Private account review drafts | 🟡 | [ADR-0038](../adr/0038-private-review-drafts.md); owner isolation, versioned save/clear, guarded post-submit cleanup. General comments and file-edit drafts are not covered. |
| Renamed conflict source paths | ✅ | [ADR-0039](../adr/0039-conflict-source-paths.md); both rename orientations through HTTP/gRPC, CLI and browser; older servers cannot silently lose a side. |

The [continuous checkpoint](../../dev/continuous-progress.md) records pending CI, unmerged work and the next correctness task. These rows do not close the beta's external qualification gates.

## ADR-0040 — structural sync safety

[ADR-0040](../adr/0040-structural-sync-resolutions.md) retains the full structural candidate for explicit virtual-sync resolutions. Tests preserve independent incoming additions, edits and deletions for both orientations and choices, reject invalid trees before workspace movement, and keep exact blob/type/mode metadata. Local structural sync and snapshots of legacy structural pending merges fail closed; `--force` does not bypass this protection. The bounded browser chooser uses complete candidate scopes from ADR-0041; larger and multi-target rename cases remain open.

## ADR-0042 — private proposal comment drafts

[ADR-0042](../adr/0042-private-comment-drafts.md) separates general-comment notes from review decisions. Core and storage tests cover exact actor ownership, independent slots, transactional versions, tombstones and the schema10-to11 upgrade. HTTP/gRPC tests cover permissions, closed proposals and private response handling. CLI and SDK tests exercise the same get/save/clear contract. Web component and browser scenarios cover explicit save, cross-device restore, conflicting versions and cleanup after posting. See the continuous checkpoint for validation status on the current unmerged head.

## ADR-0043 — private single-file drafts

[ADR-0043](../adr/0043-private-file-drafts.md) stores replacement bytes privately and retains the original revision as a GC root. Web, CLI, SDK and MCP implement explicit save/restore/clear; Web also lists saved paths. Existing-file scope only. Basic builds/type checks passed; extended test matrices were not run for this slice.

## ADR-0044 — cross-repository event feed

[ADR-0044](../adr/0044-cross-repository-events.md) reuses repository audit logs with a bounded, scoped cursor vector. Home, CLI and SDKs consume one stream; MCP provides bounded replay. A focused real-server check covers two-repository pagination/resume, denied scope and SSE cursor restoration. Basic builds are used for the client changes, without another broad test matrix.
