# Glossary

The words AgentGit uses, in alphabetical order, with a pointer to the page that explains each in depth. Where a word means
something different here than in Git, the entry says so. [Concepts](concepts.md) tells the same story in prose.

**Agent.** A software actor, such as a coding agent, with an identity (`agent:NAME`) and a signing key, registered with
`ag agent register`. An agent does its work in *runs*. By default it can propose changes but cannot integrate them.
[Running agents](guides/agents.md).

**AGWP.** The AgentGit Wire Protocol: what `ag-server` speaks, over gRPC and HTTP/JSON, with server-sent events for the live
event stream. Version 1 is described in the [reference](protocol/agwp-v1.md) and `openapi.json`.

**Approval.** A *review* with the decision `approve`. It counts toward the policy only if the reviewer is eligible and the
review is of the proposal's *current head*. [Policy](guides/policy.md#approvals).

**Attestation.** A signed statement "check X passed (or failed) on *this exact tree*, under *this policy version*, in
*this environment*", issued by a trusted issuer. See also *issuer* and *validation*. [Validation](guides/validation.md).

**Audit log.** An append-only, hash-chained record of every state-changing action, written in the same transaction as the
change. `ag audit` verifies the chain; `ag trace` shows the entries for an object, proposal, run or stream.

**Autonomy envelope.** The part of a policy (`agent_autonomy`) that says which changes an agent may integrate by itself:
listed paths, at most N files, no protected paths. Outside it, an agent-authored change needs human approvals.
[Policy](guides/policy.md#agent-autonomy).

**Base.** The stream head a workspace last synced with. A workspace's *head* is its latest snapshot.

**Blob.** Immutable file content, addressed by the SHA-256 of its type, format version and bytes.
[Object formats](architecture/object-format.md).

**Blocked.** The verdict of an integration (exit code 3) when something is missing that someone can provide: a validation, an
approval, provenance, a lease token. The stream was not changed; provide the thing and run the same command again. Compare
*denied*. [Troubleshooting](tutorials/troubleshooting-blocked-integration.md).

**Bootstrap token.** The credential of the first administrator of a server, read from the environment variable named by
`auth.bootstrap_token_env`. Use it to create users and roles, then mint scoped credentials and stop using it.
[Deployment](operations/deployment.md#secrets).

**Candidate tree.** The tree that integrating a proposal *would* produce right now. Validations are bound to it; if the stream
moves, the candidate changes and old validations stop applying.

**Changeset.** The unit of history: an ordered list of parents, an author, a message, and a delta of file operations that turns
the state of its *first parent* into its own. Immutable; its id covers every field. Roughly what Git calls a commit, but with a
delta instead of a snapshot pointer.

**Check.** A named requirement, such as `tests`, that a policy lists under `required_validations`. It is satisfied by a
passing *attestation* from a trusted issuer.

**Clone.** A local copy of a *server's* repository (`ag clone`). Its default stream mirrors the server's: `pull` only
fast-forwards it, and changes travel to the server as proposals. [Working with a server](guides/remote.md).

**Conflict.** Two edits to the same region that the merge cannot combine unambiguously. It is reported before anything changes
(exit code 4), and `ag sync` writes markers into the workspace so you can resolve it. [Merge semantics](architecture/merge-semantics.md).

**Control database.** The server's own database: credentials (as hashes), role bindings, and the registry of repositories. SQLite
by default.

**Credential.** A bearer token (`agt_…`) that a server accepts as proof of an identity. It is shown once at minting, stored only as a
hash, expires, and can be restricted to repositories, streams and operations.

**Denied.** The verdict of an integration (exit code 4) when a rule forbids it: the actor or author is not allowed, a trusted check
*failed*, too many files changed. Waiting will not help. Compare *blocked*.

**Execution plane.** The part of the system that runs checks on untrusted code: `ag-worker` claims validation jobs from the server,
runs the policy's command in a *sandbox* and signs the result. [Workers](guides/workers.md).

**Fencing token.** A number from a strictly increasing counter that comes with an *exclusive lease*. Integrating a change into a
leased scope requires presenting the current token, so a holder that stalled and lost the lease cannot land a stale change.

**Head.** The latest changeset of a stream (or of a workspace, or of a proposal). A stream's head moves only by a
compare-and-swap on its head and version.

**Idempotency key.** A caller-chosen (or derived) key that makes a request safe to repeat: the same key and request replays the
recorded result; the same key for a different request is refused.

**Identity.** `kind:name`, where kind is `user`, `agent`, `service` or `system`: `user:alice`, `agent:refactor-bot`,
`service:ci`. A run adds `#run_…`. Locally an identity is a label; against a server it comes from the credential.
[Concepts](concepts.md#identities).

**Integration.** Computing the candidate state (fast-forward or a deterministic three-way merge), evaluating the policy, and moving
the stream, all or nothing. `ag integrate`. Every integration is an *operation*.

**Issuer.** An identity whose attestations a policy trusts, pinned to a public key (`trusted_issuers`). Anything not signed by a
listed issuer, with the listed key, counts for nothing.

**Lease.** A claim on a path scope. *Soft* leases are advisory signals; *exclusive* leases are enforced at integration with a
*fencing token*. [Agents guide](guides/agents.md#3-coordination-leases-and-fencing).

**Mergeable.** One of four independent states of a change: is there one unambiguous result tree? Decided by the merge engine
(`ag preview`). The others are *validated*, *approved* and *integrated*; none implies another.

**Namespace.** The repository's name, `owner/name`, set at `ag init --namespace`. On a server it is how a repository is addressed.

**Operation.** The recorded form of any mutation: an `operation_id`, an idempotency key and a state. Replays return the recorded
result, interrupted operations resume, and a blocked integration is an operation that the next attempt resumes. `ag op list|show|resume`.

**Policy.** A versioned JSON document per stream that says what must be true before a change lands. Setting it creates a new version
and invalidates earlier attestations. [Policy](guides/policy.md), [cookbook](tutorials/policy-cookbook.md).

**Principal.** An identity without its run suffix: `agent:bot#run_1` has the principal `agent:bot`. Rules that compare people
(self-approval, lease ownership) compare principals, so a new run is not a new person.

**Protected path.** A glob in the policy (`protected_paths`) whose changes need extra approvals.

**Proposal.** A request to integrate a published changeset into a stream. Reviews attach to its exact head; if the head changes,
earlier approvals and validations stop applying. `ag propose`, `ag proposal …`.

**Provenance.** A signed record of how an agent produced a change (model, prompt digest, rationale, checks), stored beside the immutable
changeset, not inside it. A policy can require it for agent-authored changes.

**Publication.** Making a changeset and its history visible: every object is re-read and verified, and only then registered in one
transaction, so readers never see a head whose objects are missing. [Publication and integration](architecture/publication-and-integration.md).

**Reflog.** The append-only history of a stream's head movements. `ag stream reflog`.

**Review.** A reviewer's decision (`approve` or `request_changes`) on a proposal's current head. `ag proposal review`.

**Role.** What a server lets an identity do on a repository: `reader`, `writer`, `reviewer`, `integrator`, `runner`, `admin`. Roles
decide who may *ask*; the *policy* decides what must be true. [Team server tutorial](tutorials/team-server.md).

**Run.** One session of an agent, started with `ag agent start NAME`. It has its own identity (`agent:NAME#run_…`), and leases and audit
entries are attributed to it. `ag agent finish` ends it and releases its leases.

**Sandbox.** The isolation a worker runs a check in: rootless namespaces, seccomp and resource limits on Linux, or a container.
It shares the host kernel; it is not a virtual machine. [The sandbox](architecture/sandbox.md).

**Snapshot.** The working directory recorded as a new changeset of a *workspace* (`ag snapshot`). Private until you propose it.

**Stream.** A named line of integration, what other systems call a branch. It has a head and a version; updating it is a
compare-and-swap, and every move is in its *reflog*.

**Task.** A unit of work with dependencies (`ag task …`) that an agent can claim. A run can be tied to a task.

**Tree.** The resulting state of a changeset: a sorted `path → (blob, type, executable)` manifest. Derived from history, so it can
always be rebuilt; its id names an exact repository state, which is what *attestations* are bound to.

**Validated.** One of the four independent states: did trusted checks pass on *that tree*? Decided by signed attestations.

**Validation.** Running a check on the candidate tree and recording a signed attestation of the result: `ag validate run` (on your
machine, **not** sandboxed) or a worker (sandboxed). [Validation](guides/validation.md).

**Worker.** `ag-worker`: a trusted process that claims validation jobs from a server, runs them in a *sandbox* and signs the
result with its own key.

**Workspace.** An isolated line of work, usually with its own directory (`ag workspace create`). It starts at a stream's head
and has a *base* and a *head*. Roughly what a Git working tree plus a private branch would be.
