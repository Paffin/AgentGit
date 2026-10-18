# AgentGit compared with Git, GitHub, Gerrit and Jujutsu

A fair comparison is mostly a list of what the other tools are good at. This page tries to be that. Two ground rules:

- **What we say about AgentGit is checkable**: each claim links to the page, and where one exists the test, that backs it,
  and what is missing or unproven is listed under "What AgentGit does not have" below. AgentGit is **alpha**; the
  other four are mature or widely used, and for most teams today they are the right choice.
- **What we say about the others comes from their public documentation, as we understand it.** We keep to well-known
  behaviour, and where we are not sure we say nothing rather than guess. If something here is wrong or out of date,
  please [open an issue](https://github.com/Paffin/AgentGit/issues) and we will fix the page.

## The problem AgentGit is built around

Several actors change one codebase at the same time, and not all of them are equally trusted: people, coding agents, CI. Then
you want to be able to say, and prove afterwards: *who exactly* made a change (an agent's run, or a person), *what exactly
was checked* (this tree, not the one before somebody else merged), *what must hold before code lands* (and that nothing can
change between the check and the landing), and *what happened*. The other systems answer parts of this with conventions,
hosting features and add-ons. AgentGit makes them the data model.

## At a glance

| | Git | GitHub pull requests and merge queue | Gerrit | Jujutsu (`jj`) | AgentGit |
|---|---|---|---|---|---|
| What it is | a distributed VCS (the standard) | a hosted collaboration platform on Git | a self-hosted code review and access control system on Git | a Git-compatible VCS with a different working model | a VCS with its own formats, an integration engine and a policy engine |
| Unit you review and land | a commit or a branch (by convention) | a pull request | a change, with patch sets | a change, with a stable change id | a proposal |
| Where "what must be true to land" is enforced | hooks you write, or the host | the host: branch protection or rulesets, required checks, the merge queue | the server: access control, labels and submit requirements | not its job (the Git host's rules apply) | the stream's policy, evaluated in the transaction that moves the head |
| A passing check is bound to | nothing built in | a commit (in the merge queue, the temporary merge commit that was tested) | a patch set (a commit) | nothing built in | the exact candidate tree, the policy version and the environment, signed by an issuer pinned in the policy |
| Identity of an automated actor | the author and committer strings (unauthenticated unless signed) | accounts, apps and tokens | accounts | the configured user | an identity per agent and per run, with keys; credentials on a server |
| Rewrite history (rebase, amend) | yes | through Git | amending creates a new patch set | yes, with automatic rebase of descendants | **no**: changesets are immutable |
| Runs without a server | yes | no | no | yes | yes (a server is optional) |
| Maturity | very high | very high | high | newer and actively developed | **alpha** |

## Versus Git

**Same ideas.** Content-addressed immutable objects, history as a DAG of parents, a three-way merge, working offline.

**Different.** Git stores snapshots and infers history; a changeset in AgentGit is a delta against its first parent, and the
*tree* it produces is derived and addressable, because that is the thing validations are about ([object
formats](architecture/object-format.md)). Git has no notion of "may this land": a branch moves when someone with write access
pushes, and anything stricter is hooks or the host. In AgentGit a stream's head moves only through an integration, which
checks the policy and compare-and-swaps the head in one transaction ([publication and
integration](architecture/publication-and-integration.md)); an interrupted integration resumes and a repeated request returns
the recorded result instead of acting twice. Conflicts are typed results that are reported *before* anything changes, and merges
are a versioned deterministic function that follows renames per file ([merge semantics](architecture/merge-semantics.md)).

**Where Git wins.** Everything about ecosystem and reach: every tool, host and IDE speaks it; rebase, amend, tags, submodules,
partial and shallow clones, sparse checkout and Git LFS; years of work on very large repositories; signed commits and tags;
universal knowledge. AgentGit has none of the first group (it has a [bridge](guides/git-bridge.md) that carries history, merges,
modes and symlinks, but not tags, submodules or signatures), and its scale is measured at 100 000 files, not beyond
([ADR-0006](adr/0006-flat-trees-measured.md)).

**Use together.** The bridge imports and exports ([tutorial](tutorials/migrating-from-git.md)); a common arrangement is Git as
the system of record while agents work in AgentGit streams, or the other way round.

## Versus GitHub pull requests and the merge queue

**Same ideas.** A proposal is a pull request: a request to land a change, reviewed, checked, then merged. Approvals can go stale when the change
does (GitHub can dismiss approvals when new commits are pushed; in AgentGit they always do). Required checks and protected paths
(CODEOWNERS and rulesets there, `protected_paths` here) have counterparts. The stale-check problem, a check that ran on
yesterday's base, is addressed by "require branches to be up to date" and by the **merge queue**, which tests the combined result
before it lands. AgentGit addresses the same problem by binding a check to the candidate *tree*: when the stream moves, the tree
changes and the old result stops counting. AgentGit does not claim to be better at this; it is part of the data model rather
than a host feature.

**Different.**

- *Trust in a check.* In AgentGit an attestation is **signed**, names the tree, the policy version and the environment, and
  counts only if the issuer's identity *and public key* are listed in the policy; a trusted failure beats a pass. An agent
  signing its own result proves nothing ([policy](guides/policy.md#validation); tests such as
  `self_signed_agent_attestations_are_worthless` in the [security model](architecture/security.md)). A hosted platform's
  checks are as trustworthy as the platform and its integrations.
- *Agents as a first-class case.* Identity per agent run, provenance records, exclusive leases with fencing tokens, an autonomy
  envelope (which paths, how many files an agent may land alone) and path-overlap prediction are built in
  ([agents guide](guides/agents.md)). On a hosted platform you build these from branch rules, bot accounts and process.
- *Atomic and recoverable.* The policy is evaluated on facts read in the same transaction that moves the head, so there is no
  window between "checked" and "landed", and every step has fault-injection tests ([README](https://github.com/Paffin/AgentGit/blob/main/README.md)).
- *Self-hosted and embedded.* The engine runs in your process or on your server; there is no platform to depend on, and
  the audit log is yours (hash-chained, verified by `ag audit`).

**Where GitHub wins.** It is a platform: hosting, a mature review interface, issues, Actions, Dependabot, code search, packages,
organisations, SSO, an enormous ecosystem and integrations, and years of operation at scale. AgentGit has a small Web UI
([guide](guides/web-ui.md)) and an SDK, and nothing of the rest. If your agents already work well through pull requests
with branch rules and a merge queue, you may not need AgentGit.

**Use together.** Import from and export to Git ([bridge](guides/git-bridge.md)); AgentGit has no GitHub integration of its own.

## Versus Gerrit

**Same ideas.** Gerrit made the *change* the unit of review, with revisions of it (patch sets), labels such as Code-Review and Verified, and
server-enforced rules about who may submit; that is close to AgentGit's proposals, reviews bound to a head, and policy.

**Different.** Gerrit's rules are access control and submit requirements configured on the server, and a `Verified` label is
set by an account that has permission to set it. AgentGit's policy is a JSON document per stream that is versioned and
append-only, rejected on load if it can never be satisfied, and evaluated as a pure function; evidence is a signed
attestation rather than a label, bound to the exact tree. Gerrit sits on top of Git and keeps Git's model of history;
AgentGit has its own formats. AgentGit additionally models agents (runs, leases, provenance, autonomy) and works without a
server.

**Where Gerrit wins.** Maturity and scale (it has run very large projects for years), a review interface built for line-by-line
review at volume, fine-grained access control, plugins, and no change of storage format: it is Git underneath.

## Versus Jujutsu

**Same ideas.** An operation log as a first-class concept: every mutation in `jj` is an operation you can inspect and undo, and
in AgentGit every mutation is a recorded operation (`ag op list|show`) that is idempotent and resumable. Neither has a staging area.

**Different.** `jj` is a local-first VCS that uses Git as its storage format and adds a different workflow: a working copy that
is itself a commit, automatic rebase of descendants, stable change ids and **first-class conflicts** that can be committed and
resolved later. AgentGit does not rewrite history, records a conflict only as a reported result with markers in the
workspace, uses its own object formats, and its subject is a different one: not the individual developer's workflow, but
the *landing* of changes from many actors under a policy, with signed evidence and an audit trail.

**Where Jujutsu wins.** The individual workflow: history editing that is safe and pleasant, conflict handling, undo, and
compatibility with existing Git repositories and hosts without a bridge.

**Use together.** They operate at different layers. We have not tried pointing `jj` at a repository that AgentGit imports from or
exports to, and make no claim about it.

## What AgentGit does not have

Say it plainly, because the table above is flattering:

- **Maturity.** Alpha: not run in production by anyone but its authors, no external security audit, nothing published to a
  registry. The [honest limits in the README](https://github.com/Paffin/AgentGit/blob/main/README.md#status-and-honest-limits) are the list.
- **Local mode trusts the local user.** Identities are labels until you run a server; anyone who can write `.ag/` can bypass
  policy (tampering is detectable, not preventable). [Security model](architecture/security.md).
- **No history rewriting, tags, submodules, LFS, partial or shallow clones.** The Git bridge reports what it cannot carry.
- **High availability is as good as your PostgreSQL.** Several `ag-server` replicas are supported and tested (no lost update, `kill -9`, rolling restart), but the database is a single point of failure unless you run it highly available, and nothing is measured under load ([high availability](operations/high-availability.md), [ADR-0007](adr/0007-multi-replica-server.md)).
- **Ecosystem.** A CLI, a gRPC and HTTP/JSON protocol, TypeScript and Python SDKs (unpublished), an orchestrator, an MCP server and a
  small Web UI. No hosting, no integrations, no review interface comparable to the platforms above.
- **Scale beyond what is measured.** 100 000 files in one tree; hierarchical trees are deferred ([ADR-0006](adr/0006-flat-trees-measured.md)).
- **Sandboxing is Linux-first and partly unverified.** [The sandbox](architecture/sandbox.md) says exactly what it does and does not stop.

## Choosing

- *A solo developer or a team without agents*: use Git (or Jujutsu). AgentGit adds nothing you need.
- *A team on GitHub or Gerrit with agents that work fine through your existing review and checks*: keep what works. Look at
  AgentGit when you need landing to be provably atomic and tree-bound, per-run identities and leases, or an autonomy
  envelope, and you are willing to run alpha software for it.
- *Experimenting with policy-bound agent autonomy, or building something that needs a small deterministic core with a
  verifiable audit trail*: this is what it is for. Start with [the first tutorial](tutorials/first-agent-safe-repo.md).
