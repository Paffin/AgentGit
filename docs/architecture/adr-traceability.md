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
| Unsafe agents → sandboxed executors without direct storage access | 🟡 | agents cannot integrate by default and hold no storage rights; a real **sandbox** is not built (`ag validate run` is explicitly unsandboxed) |
| Forged CI → signed attestations bound to the exact state | ✅ | `ag-policy`; policy tests |
| Undefined consistency → strong metadata, eventual derived indexes | ✅ | `Meta` (SQLite) + outbox-driven derivation |
| Premature microservices → modular monolith + separate execution plane | 🟡 | modular crates, in-process engine ✓; separate execution plane 🔜 |
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
| 6 | The agent is not a trusted party | ✅ policy, leases, signed provenance; 🟡 no sandbox |
| 7 | Git is an external adapter | ✅ `ag-git` |
| 8 | Every step is observable | ✅ reflog, audit chain, outbox, `ag trace`; 🔜 OpenTelemetry/Prometheus |

## §3–§5 — system, model, publication

| Item | Status | Notes |
|---|---|---|
| SCM engine in Rust, deterministic | ✅ | |
| Object store, changeset DAG, tree resolver, diff/merge, integration/CAS, integrity/GC | ✅ | |
| PostgreSQL metadata | ↔ 🔜 | SQLite first; same transactional surface |
| S3 / MinIO object storage | 🔜 | `ObjectStore` trait is the seam; filesystem backend ships |
| Derived workers: trees ✅; path history ✅ (path index); blame 🔜 | 🟡 | |
| Outbox dispatcher | ✅ | at-least-once; in-process consumer (tree derivation) |
| Isolated execution plane | 🔜 | |
| Agent Platform in Go (orchestrator, sessions, advisor, semantic index) | 🔜 | the coordination primitives it needs exist in `ag-agent` |
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
| OIDC users; scoped short-lived agent credentials | 🔜 (server) |
| RBAC/ABAC by repo/stream/operation | 🟡 policy lists for integrators/admins/authors/approvers; no authentication in local mode |
| Sandbox with CPU/RAM/network/FS limits | 🔜 |
| No standing secrets for untrusted code | 🟡 `validate run` clears the environment; no isolation |
| Signed CI attestations | ✅ |
| Prompt-injection hardening | ✅ data is never interpreted; terminal output sanitized |
| Limits on blobs, ops, DAG depth, requests | ✅ (requests 🔜) |
| Independent, tamper-evident audit | ✅ hash chain (not yet externally anchored) |
| Path traversal, symlink escape, zip bombs, object substitution | ✅ (no archive handling exists) |
| Separate powers for agent, reviewer, admin | ✅ |

## §9 — AGWP v1

🔜. See the [protocol status and mapping](../protocol/README.md).

## §10–§11 — stack and UI

Rust core ✅ · CLI (clap) ✅ · Vue Web UI 🔜 · Go orchestrator 🔜 · SDKs 🔜 · OpenTelemetry/Prometheus 🔜 ·
NATS 🔜 · Docker Compose/Kubernetes 🔜. The ADR's text is cut off at §11 (Web UI); the rest was never received.
