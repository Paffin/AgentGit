# Concepts

## The core idea

Git stores *snapshots* and infers history; it assumes one human at a keyboard. AgentGit assumes many
actors — people, agents, CI — changing one codebase concurrently and not all equally trusted. So it
makes four things explicit that other systems leave implicit:

1. **Who** did something (identities for people, agents and each agent *run*).
2. **What exactly was checked** (validation is bound to the *tree* — the resulting state — not to a branch name).
3. **What must hold before code lands** (policy, evaluated atomically with the update).
4. **What happened** (an append-only reflog and a tamper-evident audit chain).

## Vocabulary

**Blob** — immutable file content, addressed by `SHA-256` of `(type, format version, bytes)`.

**Changeset** — the unit of history. An ordered list of parents, author, message, and a *delta* of file
operations that turns the state of its **first parent** into its own state. A merge has several parents
but still exactly one resulting state. Changesets are immutable; their ID covers every field.
([Format](architecture/object-format.md).)

**Tree** — the resulting state of a changeset: a sorted `path → (blob, type, executable)` manifest.
Trees are *derived* from history and can be rebuilt at any time; their ID names an exact repository
state, which is what validations are bound to.

**Stream** — a named line of integration (what other systems call a branch). A stream has a head and a
version; updating it is a compare-and-swap on `(head, version)`. Every update is recorded in the stream's
append-only **reflog**.

**Workspace** — an isolated line of work, optionally backed by a directory. It has a **base** (the stream
head it last synced with) and a **head** (its latest **snapshot**). Snapshots are stored but not yet
published.

**Publication** — making a changeset and its history visible: all objects are re-read and verified
(hashes, sizes, parents, that every operation list applies cleanly), and only then registered in one
metadata transaction. Readers never see a head whose objects are missing.

**Proposal** — a request to integrate a published changeset into a stream. Reviews attach to the exact
head they looked at; if the head changes, earlier approvals stop counting.

**Integration** — computing the candidate state (fast-forward, or a deterministic three-way merge with the
current head as first parent), evaluating policy, and moving the stream — all-or-nothing.

**Intent / operation** — every mutation is recorded as an operation with an `operation_id` and an
idempotency key. Replays return the recorded result; interrupted operations resume; a client that lost a
response asks `ag op show` instead of retrying blindly.

**Policy** — a versioned JSON document per stream: trusted issuers, required validations, approvals,
protected paths, agent autonomy limits. Evaluated as a pure function of facts read in the same
transaction that commits. ([Reference](guides/policy.md).)

**Attestation** — a signed statement "check X passed/failed on *this exact tree*, under *this policy
version*, in *this environment*", issued by a trusted execution plane. An agent signing its own result
proves nothing; trust comes from the issuer's identity and key being listed in the policy.

**Provenance** — a signed record of how an agent produced a change (model, prompt digest, rationale…),
stored *beside* the immutable changeset, not inside it.

**Lease** — a claim on a path scope. *Soft* leases are advisory signals; *exclusive* leases are enforced at
integration with a monotonically increasing **fencing token**, so a stale holder cannot land changes into
a scope somebody else now holds.

## Four states that are deliberately independent

A change passes through four checks, and **none implies another**:

| State | Question | Decided by |
|---|---|---|
| **Mergeable** | Is there one unambiguous result tree? | the merge engine (`ag preview`) |
| **Validated** | Did trusted checks pass on *that tree*? | signed attestations |
| **Approved** | Did enough eligible reviewers approve *that head*? | reviews + policy |
| **Integrated** | Was the stream atomically updated? | the CAS transaction |

A clean merge does not mean the code works. A validation of yesterday's candidate does not cover today's
(the stream moved, so the candidate tree changed). Different files merging cleanly says nothing about
behaviour. `ag integrate` therefore reports *which* of these is missing (exit code 3 for "provide it", 4
for "refused").

## Identities

`kind:name`, where kind is `user`, `agent`, `service` or `system` — for example `user:alice`,
`agent:refactor-bot`, `service:ci`. An agent *run* adds a suffix: `agent:refactor-bot#run_7f3a…`. Rules
that compare people (self-approval, ownership) compare the **principal** — the part before `#` — so an
agent cannot approve its own work by starting a new run.

The acting identity comes from `--as`, `AG_IDENTITY`, or `user:$USER`. In local mode this is a
convention, not authentication; see [Security](architecture/security.md).

## Revisions

Anywhere a command takes a revision you can use: `@` (head of the default workspace), `@name`
(head of workspace *name*), a full 64-hex changeset ID, a unique hex prefix (≥ 6), a proposal ID
(`prop_…`), or a stream name — optionally followed by `~N` (N-th first-parent ancestor) and `^N`
(N-th parent): `main~2`, `prop_8f3a^2`. `REV:PATH` addresses a file (`ag cat main:src/lib.rs`).
