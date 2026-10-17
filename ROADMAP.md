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

## Next — the network layer (0.2)

- **Server** exposing the engine: HTTP/JSON and gRPC (AGWP v1; see the [protocol mapping](docs/protocol/README.md)).
- **Authentication and authorization**: OIDC for users, short-lived scoped credentials for agents, RBAC/ABAC
  per repository, stream and operation. This is what turns policy from "enforced by a cooperative engine"
  into "enforced against an uncooperative client".
- **PostgreSQL metadata backend** and **S3-compatible object store** behind the existing seams.
- **Remote execution plane**: isolated workers (OCI/VM) that run checks on a candidate tree and submit
  signed attestations over the network. Until then `ag validate run` is explicitly unsandboxed.
- Quotas and rate limits; OpenTelemetry traces and Prometheus metrics.

## Then

- **Web UI**, designed around live agent work rather than a branch list: who is working where (runs,
  leases), what overlaps (prediction), what is blocked and on what, the audit timeline. Read-only first.
- **Scale**: hierarchical Merkle trees (O(changed directories) instead of O(files)), packed objects, incremental
  GC marking, streaming blobs instead of whole-file reads.
- **Merge**: rename detection; optional language-aware advisors that *propose* resolutions (never part of
  the correctness core).
- **History tooling**: blame (derived index), path history views, event subscription/replay API.
- **Agent platform**: orchestrator service (Go) with sessions, dependency scheduling, review agents and a
  semantic index, built on the coordination primitives in `ag-agent`.
- **SDKs**: TypeScript and Python clients for AGWP.
- **Offline mode**: local object store, publication queue and reconciliation against a server.
- **Operations**: Docker Compose, then Kubernetes manifests; signed releases; reproducible builds.

## Ideas we are deliberately not doing

Replacing Git for existing human workflows tomorrow; letting a model decide anything the deterministic core
guarantees; a global repository lock; hiding policy decisions inside prompts.

## How to influence this

Open an issue using the feature template and say which guarantee your idea touches; changes to formats,
hashing, merge semantics or the publication protocol need an ADR ([CONTRIBUTING](CONTRIBUTING.md)).
