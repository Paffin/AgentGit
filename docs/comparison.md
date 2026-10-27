# AgentGit compared with Git, GitHub, GitLab, Bitbucket, Gerrit and Jujutsu

A fair comparison is mostly a list of what the other tools are good at. This page tries to be that. Two ground rules:

- **What we say about AgentGit is checkable**: each claim links to the page, and where one exists the test, that backs it,
  and what is missing or unproven is listed under "What AgentGit does not have" below. AgentGit is **early public beta**; the
  others are mature or widely used, and for most teams today they are the right choice.
- **What we say about the others comes from their public documentation, as we understand it.** We keep to well-known
  behaviour, and where we are not sure we say nothing rather than guess. Product names, plan tiers and features change;
  the [sources](#sources) at the end say what was read and when. If something here is wrong or out of date,
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
| Maturity | very high | very high | high | newer and actively developed | **early public beta** |

### Hosting platforms

GitHub, GitLab and Bitbucket are products you subscribe to or self-host, with many features beyond landing a change. The
rows below cover only how a change lands when several actors, some of them automated, write to one codebase. Plan names
are as in the vendors' documentation at the access date; "Premium" and similar mean the feature is not in the free tier.

| | GitHub | GitLab | Bitbucket (Cloud and Data Center) | AgentGit |
|---|---|---|---|---|
| Unit of review | pull request | merge request | pull request | [proposal](concepts.md) |
| Where merge rules are enforced | the platform: branch protection and rulesets | the platform: protected branches, approval rules, push rules (several need Premium) | the platform: branch permissions and merge checks (enforced on Premium in Cloud) | the stream's [policy](guides/policy.md), evaluated in the transaction that moves the head |
| A passing check is bound to the exact tree that lands | in the merge queue, which tests a temporary merge of the queue; otherwise a commit | with merged results pipelines and merge trains (Premium and Ultimate); otherwise a pipeline | required builds on the head commit; merge queues build a merge result (Cloud: paid plan with Pipelines) | always: a signed attestation names the tree, policy version and environment ([validation](guides/validation.md)) |
| Identity of automated actors | accounts, GitHub Apps, tokens | service accounts, access tokens | accounts, tokens | an identity per agent and per run; a credential can be bound to one run ([ADR-0010](adr/0010-run-bound-credentials.md)) |
| Policy-bound autonomy for agents | built from branch rules and the permissions you give a bot | built from the same pieces | built from the same pieces | built in: `agent_autonomy` allowed paths and a file limit ([policy](guides/policy.md#agent-autonomy)) |
| Tamper-evident audit | an audit log with export; its documentation makes no integrity claim | audit events, streaming on Ultimate; its documentation makes no integrity claim | an audit log; not compared here | hash-chained, checked by `ag audit` and `ag verify` ([security](architecture/security.md)) |
| Self-contained and offline | hosted; GitHub Enterprise Server for self-management | SaaS or self-managed | Cloud or Data Center | local mode needs no server; the server is one binary ([remote](guides/remote.md)) |
| Ecosystem and integrations | very large | very large; CI and security scanning are part of the platform | large; Jira and Pipelines | small: CLI, SDKs (unpublished), MCP, Web UI |
| Maturity | very high | very high | very high | early beta |

"Built from the same pieces" describes what the documentation covers, not that nothing can be done: all three have APIs and
custom checks, and teams build agent guard rails on them.

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

## Versus GitLab

**Same ideas.** A merge request is a proposal: a request to land a change that is reviewed, checked and then merged.
Protected branches decide who may push or merge. Approval rules decide how many approvals, and from whom, a request needs;
code owners of the changed files can become eligible approvers, and the author does not count as an approver by default.
"Pipelines must succeed" is a required check. The stale-check problem is addressed by **merged results pipelines** (the
pipeline runs on the merge request's changes combined with the target branch) and **merge trains**, which test each queued
request on top of the requests ahead of it, run those pipelines in parallel (up to 20 by default) and merge in order; a
failing request leaves the train and the ones behind it restart without it. This is the problem AgentGit solves by binding a
check to the candidate *tree*; the mechanisms differ.

**Different.**

- *Where the binding lives.* On GitLab a merge train is a feature of a CI pipeline you configure: the pipeline has to be set
  up for merge request pipelines, merged results must be enabled first, and the repository must be hosted in GitLab. In
  AgentGit the attestation is part of the data model: it names the tree, the policy version and the environment, is signed
  by an issuer the policy lists, and stops counting the moment the stream moves ([validation](guides/validation.md);
  `attestation_cannot_be_reused_after_the_stream_moves`). AgentGit has no queue-wide combined testing of its own:
  `ag integrate` checks one candidate against the current head, and the ordered integration queue serializes landing
  ([agents guide](guides/agents.md)).
- *Rules at landing.* Push rules (commit message and branch name patterns, secret and file-name blocks, file size, signed
  commits; Premium) act on pushes. AgentGit has nothing like them. Its policy decides whether a *proposal* may be
  integrated (required checks, approvals bound to the head, protected paths, who may integrate, the autonomy envelope) and
  is evaluated in the transaction that moves the head ([publication and
  integration](architecture/publication-and-integration.md)). The policy document is versioned and append-only.
- *Agents.* GitLab has service accounts (in every tier, authenticating with tokens) and access tokens. AgentGit identifies
  an agent *run*, scopes a credential to it, and limits what the agent may land alone through the autonomy envelope; it also
  has leases with fencing tokens and path-overlap prediction ([agents guide](guides/agents.md)). On GitLab these are
  conventions you build from protected branches, approval rules and process.
- *Audit.* GitLab's audit events can be streamed to external destinations (an Ultimate feature on self-managed). The page we
  read describes no tamper-evidence or signing. AgentGit's audit entries are written in the same transaction as the change
  and form a hash chain that `ag audit` verifies ([security model](architecture/security.md);
  `audit_chain_detects_tampering`). That detects edits; AgentGit does not stream events anywhere.
- *Self-managed.* GitLab can be run on your own infrastructure, and so can AgentGit; the difference is scope. GitLab
  self-managed is a large platform with many services. AgentGit's server is one binary over SQLite or PostgreSQL, and local
  mode needs no server at all ([deployment](operations/deployment.md)).

**Where GitLab wins.** Almost everything around the change: one application for repositories, issues, boards, CI/CD with
runners, package and container registries, security scanning, environments and deployments; review of large merge
requests; groups, permissions and compliance features at the scale of an organisation; a large user base; commercial
support. Merge trains test the combined result of many queued requests, which AgentGit does not. AgentGit has a small Web UI
and an SDK, no CI hosting and no issue tracker ([guide](guides/web-ui.md)).

**Use together.** Import from and export to Git ([bridge](guides/git-bridge.md)); AgentGit has no GitLab integration of its
own. We have not run AgentGit against a GitLab instance and make no claim about it.

## Versus Bitbucket

Bitbucket comes in two products with different documentation: Bitbucket Cloud, hosted by Atlassian, and Bitbucket Data
Center, self-managed. Where they differ, we say which one.

**Same ideas.** A pull request is a proposal. Branch permissions limit who may write to a branch or merge into it through a
pull request, and can also block deleting a branch or rewriting its history. **Merge checks** hold a pull request until it
meets conditions: a minimum number of approvals (also from default reviewers), no "changes requested", no open tasks, and
successful builds. In Cloud, merge checks that block a merge need the Premium plan; on other plans they only warn. Data
Center adds **required builds**: named builds, for example from Bamboo or Jenkins, that must pass on the latest commit before
merging into protected branches; Code Insights reports can block a merge too. Cloud can **reset approvals when the source
branch changes** on Premium, optionally keeping them if the diff is unchanged. Atlassian documents **merge queues**: pull
requests wait in a queue, a merge result commit is built for each, a pipeline runs on that commit, and the target branch is
updated only if it passes (on Cloud a paid plan with Pipelines, set up by a repository admin; Atlassian's pages and blog
disagree on whether the feature is still beta, so check them). This is the same stale-check problem AgentGit addresses by
binding a check to the tree.

**Different.**

- *What a build result is bound to.* The documented merge check is a build status on the head commit, reported through the
  build status API by whatever CI you use; the merge queue validates a merge result commit. In AgentGit the result is a signed
  attestation for the tree, and counts only if its issuer's identity and public key are listed in the policy; an agent
  signing its own result proves nothing (`self_signed_agent_attestations_are_worthless`, [security
  model](architecture/security.md)). We do not know how Bitbucket treats who reported a build status beyond what its
  documentation says, and make no claim.
- *Rules.* Bitbucket's merge checks and branch permissions are settings on the platform, set per project or repository, and
  extensible with Marketplace apps. AgentGit's policy is one document per stream that is versioned, append-only, rejected on
  load if it can never be satisfied, and evaluated as a pure function in the transaction that moves the head
  ([policy](guides/policy.md)). There is no direct equivalent of default reviewers: AgentGit's approvals are counted through
  the policy, and an agent-authored change outside its envelope needs a human approval.
- *Agents.* Bitbucket has accounts and tokens, and Pipelines for running work. AgentGit models the run: an identity, a
  credential bound to it, a lease on paths with a fencing token, signed provenance and an autonomy envelope
  ([agents guide](guides/agents.md)).
- *Cloud, Data Center and AgentGit.* Cloud is a hosted service; Data Center is self-managed. AgentGit is self-hosted by design
  (a binary, or the Compose and Kubernetes files in `deploy/`), and in local mode there is no server
  ([deployment](operations/deployment.md)). Several AgentGit replicas on PostgreSQL are supported and tested; a failover of
  PostgreSQL itself is not ([high availability](operations/high-availability.md)).

**Where Bitbucket wins.** The Atlassian ecosystem: tight Jira integration, Bitbucket Pipelines for CI/CD hosted with the
repository, Marketplace apps, a mature review interface, support, and a long operating history at enterprise scale, with
permissions at the project and workspace level. AgentGit has none of those: a small Web UI, an SDK, no CI hosting, no issue
tracker and no marketplace.

**Use together.** Import and export through the [Git bridge](guides/git-bridge.md). We have not run AgentGit against
Bitbucket.

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

- **Maturity.** Early beta: not run in production by anyone but its authors, no external security audit, nothing published to a
  registry. The [status and limits in the README](https://github.com/Paffin/AgentGit/blob/main/README.md#status-and-limits) are the list.
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
- *A team on GitHub, GitLab, Bitbucket or Gerrit with agents that work fine through your existing review and checks*: keep what works. Look at
  AgentGit when you need landing to be provably atomic and tree-bound, per-run identities and leases, or an autonomy
  envelope, and you are willing to run early beta software for it.
- *Experimenting with policy-bound agent autonomy, or building something that needs a small deterministic core with a
  verifiable audit trail*: this is what it is for. Start with [the first tutorial](tutorials/first-agent-safe-repo.md).

## Sources

What was read for the statements above about GitHub, GitLab and Bitbucket. Accessed 2026-10-19. These pages change; if one
disagrees with this page, the vendor's page is right and this page needs a fix.

GitHub

- [Managing a merge queue](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue)
- [About protected branches](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches)
- [About rulesets](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets)
- [Reviewing the audit log for your organization](https://docs.github.com/en/organizations/keeping-your-organization-secure/managing-security-settings-for-your-organization/reviewing-the-audit-log-for-your-organization)
- [About apps](https://docs.github.com/en/apps/overview)

GitLab

- [Merge trains](https://docs.gitlab.com/ci/pipelines/merge_trains/)
- [Merge request approval rules](https://docs.gitlab.com/user/project/merge_requests/approvals/rules/)
- [Protected branches](https://docs.gitlab.com/user/project/repository/branches/protected/)
- [Push rules](https://docs.gitlab.com/user/project/repository/push_rules/)
- [Auto merge and "pipelines must succeed"](https://docs.gitlab.com/user/project/merge_requests/auto_merge/)
- [Audit event streaming](https://docs.gitlab.com/ee/administration/compliance/audit_event_streaming.html)
- [Service accounts](https://docs.gitlab.com/user/profile/service_accounts/)

Bitbucket

- [Suggest or require checks before a merge (Cloud)](https://support.atlassian.com/bitbucket-cloud/docs/suggest-or-require-checks-before-a-merge/)
- [Use branch permissions (Cloud)](https://support.atlassian.com/bitbucket-cloud/docs/use-branch-permissions/)
- [Manage pull requests with merge queues (Cloud)](https://support.atlassian.com/bitbucket-cloud/docs/manage-pull-requests-with-merge-queues/)
- [Checks for merging pull requests (Data Center)](https://confluence.atlassian.com/bitbucketserver/checks-for-merging-pull-requests-776640039.html)

Statements about Git, Gerrit and Jujutsu come from their public documentation as we understand it and were not re-checked
on that date.
