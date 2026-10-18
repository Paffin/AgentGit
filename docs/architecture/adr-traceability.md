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
| Unsafe agents → sandboxed executors without direct storage access | 🟡 | agents cannot integrate by default and hold no storage rights; checks run in an [`ag-sandbox`](sandbox.md) sandbox through `ag-worker` (rootless namespaces, tested from inside; OCI backend untested against a real runtime; not a VM); `ag validate run` stays explicitly unsandboxed |
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
| 8 | Every step is observable | ✅ reflog, audit chain, outbox, `ag trace`; 🔜 OpenTelemetry/Prometheus |

## §3–§5 — system, model, publication

| Item | Status | Notes |
|---|---|---|
| SCM engine in Rust, deterministic | ✅ | |
| Object store, changeset DAG, tree resolver, diff/merge, integration/CAS, integrity/GC | ✅ | |
| PostgreSQL metadata | ✅ | per repository, same transactional surface as SQLite; the whole suite runs on both ([guide](../operations/postgres-and-s3.md), [ADR-0004](../adr/0004-postgres-and-s3-backends.md)). The server's own control database is still one SQLite file |
| S3 / MinIO object storage | ✅ | `S3ObjectStore` behind the same trait; tested against a signature-checking S3 server (moto), not against AWS or MinIO |
| Derived workers: trees ✅; path history ✅ (path index); blame ✅ (computed on demand, follows renames) | ✅ | [History guide](../guides/history.md); renames in merge: [ADR-0005](../adr/0005-rename-aware-merge.md) |
| Outbox dispatcher | ✅ | at-least-once; in-process consumer (tree derivation) |
| Isolated execution plane | 🟡 | `ag-worker` + [`ag-sandbox`](sandbox.md): claims jobs, verifies the exact tree, runs the check isolated, signs; [guide](../guides/workers.md). Not yet: VM-level isolation, a verified OCI backend, worker pools/labels |
| Agent Platform in Go (orchestrator, sessions, advisor, semantic index) | ✅ | `services/agent-orchestrator`, tested against the real server ([guide](../guides/orchestrator.md)). Trust boundary (§3): it holds a scoped credential, and `TestOrchestratorNeverIntegrates` fails on any call to integrate, set policy, grant roles, create repositories or approve. Heuristic, not model-backed, advisors |
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
| Virtual workspace (isolated, no full checkout required) | 🟡 | workspaces are directory-backed; API-only virtual workspaces 🔜 |
| Dependency awareness | ✅ | task dependencies, cycle-free, enforced at run start |
| Conflict prediction | ✅ | `ag predict` (path level) |
| Integration queue | ✅ | `ag queue` |
| Provenance | ✅ | signed, separate from changesets |
| Autonomous review | 🔜 | reviews by agents are possible; policy decides whether they count |
| Policy-bound autonomy | ✅ | `agent_autonomy` |
| Replay & trace | 🟡 | `ag trace`, audit chain; event replay API 🔜 |
| Soft leases; fencing tokens for exclusivity | ✅ | |

## §8 — security

| Measure | Status |
|---|---|
| OIDC users; scoped short-lived agent credentials | ✅ `ag-server` (`security.rs`: OIDC, expiring/revocable/scoped credentials that can never exceed their minter). Credentials bind to an agent name, not a run — [roadmap](../../ROADMAP.md) |
| RBAC/ABAC by repo/stream/operation | ✅ roles per repository pattern × credential scope (repos, streams, operations) on the server; policy lists inside the engine; no authentication in local mode |
| Sandbox with CPU/RAM/network/FS limits | 🟡 `ag-sandbox`: all five enforced and tested from inside the namespace sandbox; memory/task limits are sampled (not kernel-exact) unless a cgroup is delegated; see [sandbox](sandbox.md) |
| No standing secrets for untrusted code | 🟡 the worker's key and credential are never visible to the sandboxed check (tested); `validate run` only clears the environment |
| Signed CI attestations | ✅ |
| Prompt-injection hardening | ✅ data is never interpreted; terminal output sanitized |
| Limits on blobs, ops, DAG depth, requests | ✅ request size (HTTP and gRPC alike), concurrency, per-credential rate limits and repository quotas on the server: total bytes charged on every write path (`quota.rs`; `quotas.rs`, `api_honesty.rs`), bytes per object, streams, open proposals. Stream and proposal counts are checked just before the insert, so concurrent creates can overshoot by the number in flight |
| Independent, tamper-evident audit | ✅ hash chain (not yet externally anchored) |
| Path traversal, symlink escape, zip bombs, object substitution | ✅ (no archive handling exists) |
| Separate powers for agent, reviewer, admin | ✅ |

## §9 — AGWP v1

✅ gRPC and HTTP/JSON on one port, WebSocket not needed (SSE for events): [reference](../protocol/agwp-v1.md), [overview and mapping](../protocol/README.md), generated [OpenAPI](../protocol/openapi.json) checked against the router in CI.

## §10–§11 — stack and UI

Rust core ✅ · CLI (clap) ✅ · Vue Web UI ✅ ([guide](../guides/web-ui.md)) · Go orchestrator ✅ · SDKs ✅ (TypeScript, Python; unpublished) · OpenTelemetry (traces) / Prometheus ✅ ·
NATS 🔜 (events use SSE and the audit log) · Docker Compose/Kubernetes 🟡 (files validated in CI; not run end to end — [deployment](../operations/deployment.md)). The ADR's text is cut off at §11 (Web UI); the rest was never received.
