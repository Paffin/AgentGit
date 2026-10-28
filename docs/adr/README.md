# Architecture decision records

Why the system is the way it is. An ADR records one decision with its context, the alternatives that were
rejected and the consequences; its status (`accepted`, with caveats where something is not yet verified) is the
first thing in each record. A change to a byte format, to merge semantics or to the wire protocol needs a new ADR
(see [CONTRIBUTING](https://github.com/Paffin/AgentGit/blob/main/CONTRIBUTING.md)). Which decision is exercised by
which test: [ADR traceability](../architecture/adr-traceability.md).

| ADR | Decision |
|---|---|
| [0001](0001-final-architecture.md) | The overall architecture after the critical audit (in Russian, the historical record) |
| [0002](0002-embedded-engine-first.md) | Embedded engine first; SQLite as the first metadata backend |
| [0003](0003-sparse-derived-tree-cache.md) | Sparse derived-tree cache (keyframes) |
| [0004](0004-postgres-and-s3-backends.md) | PostgreSQL metadata and S3-compatible objects behind the existing seams |
| [0005](0005-rename-aware-merge.md) | Rename-aware tree merge |
| [0006](0006-flat-trees-measured.md) | Flat trees: measured costs, optimizations, and when to go hierarchical |
| [0007](0007-multi-replica-server.md) | Several `ag-server` replicas on one PostgreSQL and one bucket |
| [0008](0008-mcp-server.md) | `ag-mcp`: the remote workflow as a Model Context Protocol server |
| [0009](0009-identity-names-and-principals.md) | An identity's name cannot start with the run separator |
| [0010](0010-run-bound-credentials.md) | Credentials bound to a run, a delegator role, operator calls on tasks and runs |
| [0011](0011-proposal-paging-indexes-and-background-leases.md) | Proposal paging indexes (metadata schema 3), dated idempotency records, a lease for background work |
| [0012](0012-job-claim-fencing.md) | A fencing token for job claims (an optional request field, no schema change) |
| [0013](0013-directory-renames.md) | Directory renames in the tree merge (`ag-merge/3`); copy detection for blame and show |
| [0014](0014-review-head-precondition.md) | Optional expected-head precondition for atomically recording reviews |
| [0015](0015-bounded-revision-code-search.md) | Bounded literal code search tied to an immutable revision |
| [0016](0016-repository-issues.md) | Durable repository issues and atomic handoff to scoped agent tasks |
| [0017](0017-task-execution-gate.md) | Persisted operator gate for new task-bound runs, enforced by the agent engine |
| [0018](0018-permanent-run-pages.md) | Exact historical run lookup and permanent browser pages tied to session identity |
| [0019](0019-queued-task-assignment.md) | Atomic queued-agent reservation and assignment-aware dispatch |
| [0020](0020-executor-connectivity.md) | Expiring executor connection reports and pre-run identity preparation |
| [0021](0021-browser-executor-setup.md) | Browser-prepared scoped executor setup with separate private token files |
| [0022](0022-saved-executor-output.md) | Opt-in bounded run output with redaction and deletion tombstones |
| [0023](0023-task-history-pagination.md) | Task-filtered history and keyset continuation for agent runs |
| [0024](0024-proposal-discussion-comments.md) | Head-bound discussion comments without approval privileges |
| [0025](0025-line-review-threads.md) | Revision-bound line discussion, paged replies and optimistic resolution |
