# FAQ

Short answers, with a link to where the long one lives. Where something is not supported or not verified, the answer
says so. If a question is missing, [CONTRIBUTING](https://github.com/Paffin/AgentGit/blob/main/CONTRIBUTING.md) says how
to ask it.

## The basics

### What is AgentGit?

A source control system for repositories that people, coding agents and CI change at the same time. Its core stores and
merges content deterministically (no model, clock or randomness in any decision), and every change lands through a
**proposal** that a per-stream **policy** judges: validation of the exact tree, approvals, leases, an autonomy envelope for
agents. See the [README](https://github.com/Paffin/AgentGit/blob/main/README.md), [Concepts](concepts.md) and the
[first tutorial](tutorials/first-agent-safe-repo.md).

### Is it ready for production?

No. It is **alpha**. The core and the guarantees described in these docs are implemented and tested, and so is the
platform around it, but it has not been run in production by anyone but its authors, there has been no external security
audit, and nothing has been published to a package registry yet. The list of honest limits is in the README
("Status and honest limits"); the [roadmap](https://github.com/Paffin/AgentGit/blob/main/ROADMAP.md) says what is
planned. Keep `ag backup`s and a Git mirror of anything you cannot lose.

### Do I have to leave Git?

No. `ag git import` reads a Git repository into a stream and `ag git export` writes a stream out as a Git branch; the
bridge carries history, merges, modes and symlinks, and **not** submodules, tags, notes, signatures or the committer
([what carries over](tutorials/migrating-from-git.md#what-carries-over-and-what-does-not)). It is one-shot in each
direction, not a live mirror, and an exported branch is a parallel history with the same trees but different commit ids.
[Migrating a Git repository](tutorials/migrating-from-git.md) walks through it, including a check that nothing was lost.

### Do I need a server?

Not to start. A repository is a directory (`.ag/`) and every command works offline with an embedded engine. You need a
server, `ag-server`, when several people or agents must share a repository, or when you need identities that are
authenticated rather than labels. [Run a team server](tutorials/team-server.md) shows both.

### Does it use a language model? Does it need the internet?

The core never does: hashing, merging, policy evaluation and integration are deterministic functions and never call a model.
`ag` talks to the network only when you give it a remote; `ag-server` only to what you configure (an OIDC provider,
a telemetry collector, PostgreSQL, S3). The optional orchestrator
([guide](guides/orchestrator.md)) can run agents and has advisors on top of the core; they can only *propose*.

### What platforms does it run on?

The full test suite runs in CI on Linux (x86-64) and macOS (arm64); the release workflow also builds Linux arm64 and
macOS x86-64 archives ([Releasing](operations/releasing.md)). The Windows port builds and its tests run in CI, with the
documented exceptions in [Windows](operations/windows.md). The sandbox for untrusted checks is Linux-only.

## Day to day

### What do exit codes 3 and 4 mean?

`3` means **blocked**: something is missing that someone can provide (an approval, a validation, provenance, a lease token),
so provide it and run the same command again. `4` means **refused**: a conflict, a policy denial or an invalid request, and
waiting will not help. `1` is any other error, including "not found" and "already exists"; `2` is a usage error. Branch on
these, not on message text. `ag help exit-codes` has the table, which commands use which code, and the tests that pin it.

### Why does it say "no trusted passing attestation for this exact tree" when CI passed?

Because a validation counts only if it is signed by a trusted issuer, names **this exact candidate tree**, and was made under
**this policy version**. If the stream moved, the proposal changed or the policy was replaced since the check ran, the
result no longer applies: run it again. It is the point of the design, not a bug. [Troubleshooting a blocked
integration](tutorials/troubleshooting-blocked-integration.md) reproduces each case.

### Why did my approval stop counting?

Approvals are bound to the proposal's **head** at the time of the review. If the author updates the proposal, earlier
approvals stop counting, so nobody can get a different change landed under yesterday's "looks good". The author's own
approval never counts when the policy says `forbid_self_approval`, and approvals are compared by *principal*, so an
agent starting a new run does not become a second reviewer.

### Why can't my agent integrate its own change?

By default an agent may not integrate at all. A policy can allow it for changes that fit an **autonomy envelope** (listed
paths, at most N files, no protected paths) and still require CI and provenance. Everything outside the envelope needs a
human approval. See [Policy](guides/policy.md#agent-autonomy) and the [first tutorial](tutorials/first-agent-safe-repo.md).

### What happens when two agents edit the same file?

Three layers. **Leases** let an agent say "I am working on `src/**`": soft leases are advisory, exclusive ones are enforced
at integration with a fencing token. **`ag predict`** warns about overlapping work by path. And the **merge** itself is
deterministic: edits to different lines merge, overlapping edits are a typed *conflict* that is reported with exit code 4
before anything changes, and `ag sync` writes conflict markers into the workspace for the loser to resolve
([walkthrough](tutorials/troubleshooting-blocked-integration.md#6-conflict-the-edits-overlap)). Renames are followed per
file; directory renames and copies are not detected.

### How do I undo an integration?

There is no `revert` or `reset` command. Integrated history is immutable and the stream's head only moves forward through
integrations; `ag stream reflog` shows every move. To undo a change, put the files back (`ag cat OLD_REV:path`, or
`ag checkout`), `ag snapshot`, and propose the inverse as an ordinary change, which goes through the same policy.

### Can I rebase, squash or rewrite history?

No. Changesets are immutable and content-addressed, and a merge is a deterministic function of its inputs. Integration is a
fast-forward when possible and a merge otherwise; there is no history rewriting and no tag command.

### What is a workspace, and how is it different from a branch or a clone?

A **workspace** is an isolated line of work that starts at a stream's head, usually with its own directory
(`ag workspace create NAME --dir DIR`); its snapshots are private until you `propose` them. A **stream** is what other systems
call a branch: the line that proposals are integrated into. A **clone** (`ag clone`) copies a *server's* repository to your
machine; its default stream mirrors the server's and you never move the server's head yourself. See the
[glossary](glossary.md).

## Identity and security

### Are identities authenticated?

**Locally, no.** In a local repository `user:alice` is a label that you choose (`--as`, `AG_IDENTITY`); anyone who can write
`.ag/` can bypass policy, and tampering is *detectable* (`ag verify`, `ag audit`) but not prevented. **Against a server,
yes**: every request carries a credential, the server derives the identity from it, and policy is enforced there. If you do
not control the clients, run the server. [Security model](architecture/security.md) lists what is and is not defended.

### Is `ag validate run` safe for untrusted code?

No. It runs the command on your machine **without a sandbox**, with a scratch directory and a nearly empty environment,
and says so when it runs. Use it for trusted code and local workflows. For untrusted code use an
[`ag-worker`](guides/workers.md) with its [sandbox](architecture/sandbox.md), which shares the host kernel (it is not a VM)
and has parts that are not yet verified; read that page before relying on it.

### Where are the keys, and who can read them?

Agent and issuer private keys are files under `.ag/keys/` (or `AG_KEY_DIR`), mode 0600 on Unix, and `ag` refuses to load a
key file that group or others can read. Keep that directory out of any environment that runs untrusted code. A remote's
token is never stored in `.ag/remotes.json`; see [Configuration](reference/configuration.md).

### How do I report a vulnerability?

See [SECURITY.md](https://github.com/Paffin/AgentGit/blob/main/SECURITY.md). Please do not open a public issue.

## Operating it

### Where is my data, and how do I back it up?

In `.ag/` of the repository: a SQLite metadata database and the object store (or PostgreSQL and S3 if you configured them).
`ag backup DIR` writes a verified, consistent copy; `ag verify` re-hashes everything and checks the audit chain.
[Durability and backup](operations/durability.md) explains what survives a crash and what a restore does.

### How big can a repository be?

It has been measured with 100 000 files, where an integration takes a few hundred milliseconds ([ADR-0006](adr/0006-flat-trees-measured.md)
has the numbers and the triggers for changing the design). Larger repositories, partial or shallow clones, and Git LFS are
**not** supported or not tested. Try your own repository on a copy first.

### Can I run it highly available?

Yes, with PostgreSQL and S3 behind several `ag-server` replicas: [High availability](operations/high-availability.md) says
what is guaranteed (each guarantee is a test that kills and restarts real server processes), what fails how, and what is
not verified. The default deployments still run one replica, and PostgreSQL itself is a single point of failure unless
you run it highly available. The design is [ADR-0007](adr/0007-multi-replica-server.md).

### How do I connect my own agent or tool?

Through the CLI (stable exit codes and `--json` output: [shapes](reference/cli-json.md)), the HTTP/JSON and gRPC protocol
([AGWP v1](protocol/agwp-v1.md), [`openapi.json`](protocol/openapi.json)), the TypeScript and Python
[SDKs](guides/sdks.md) (not published to a registry yet), or the MCP server `ag-mcp` ([guide](guides/mcp.md), [ADR-0008](adr/0008-mcp-server.md)).

### Are these docs tested?

Largely, yes, and the pages say where. The CLI reference, help topics, `--json` shapes, completions and man pages are generated
from the binary and checked in CI; the tutorials' commands are executed against the real binaries and their output is
compared ([how](tutorials/README.md#how-these-tutorials-are-tested)); links, anchors, test names and `ag` commands are
checked by `scripts/check_docs.py` and the site build. What is *not* tested is called out where it comes up.
