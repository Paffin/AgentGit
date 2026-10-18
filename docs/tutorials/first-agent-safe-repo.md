# Your first agent-safe repository

You will build a small repository in which an AI agent can propose changes, but **cannot land code without a
passing CI check on the exact tree, a human's approval, signed provenance and the lease on the files it touches**,
while a documentation-only change inside the agent's *autonomy envelope* lands with no human at all. It takes about
fifteen minutes, most of it reading: every command below is run by the test suite against the real `ag` binary
([how the tutorials are tested](README.md#how-these-tutorials-are-tested)), and the output blocks are checked
against what the commands print.

You need `ag` ([install](../getting-started.md#install)) and a shell. Nothing else: no server, no network, no
model. The "agent" is just an identity that runs commands; in real use it is your coding agent, calling the same
commands (or the [MCP server](../protocol/README.md), or an [SDK](../guides/sdks.md)).

Three identities take part: **alice** (a human, the maintainer), **bob** (a human reviewer) and **refactor-bot**
(the agent). `service:ci` is the identity of your CI system.

## 1. A repository with a first commit

<!-- doctest -->
```bash
export AG_IDENTITY=user:alice
mkdir hello && cd hello
ag init --namespace me/hello
mkdir -p src docs
printf 'pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n' > src/lib.rs
printf '# hello\n\nA small library.\n' > docs/README.md
ag snapshot -m "Initial import"
FIRST=$(ag -q propose -t "Initial import")
ag integrate "$FIRST"
```

<!-- expect -->
```text
Initialized AgentGit repository in .../hello/.ag
  namespace: me/hello
  stream:    main
...
Snapshot ... in workspace 'default'
Integrated (fast-forward): stream is now at ... (version 1), 2 file(s) changed.
```

What happened: `snapshot` recorded your files privately in your **workspace**; `propose` published them and opened a
**proposal** to change the stream `main`; `integrate` moved `main` to include it. With no policy stored, a human may
integrate anything, so this first change went straight in. (`-q` makes `propose` print only the proposal id, which
the shell captures in `$FIRST`. `AG_IDENTITY` says who you are; see [Configuration](../reference/configuration.md).)

## 2. Write the rules down

A **policy** is a JSON document per stream that says what must be true before a change lands. You need a trusted
CI first: `ag validate keygen` creates a signing key for it and prints the public key, which goes into the policy.

<!-- doctest -->
```bash
KEY=$(ag -q validate keygen ci)
cat > ../policy.json <<JSON
{
  "trusted_issuers": [{ "identity": "service:ci", "public_key": "$KEY" }],
  "required_validations": [{ "check": "tests", "issuers": ["service:ci"] }],
  "approvals": { "min": 0, "min_for_agents": 1, "forbid_self_approval": true },
  "agent_autonomy": { "auto_integrate": true, "allowed_paths": ["docs/**"], "max_files": 5 },
  "require_provenance_for_agents": true
}
JSON
ag policy set ../policy.json
```

<!-- expect -->
```text
Policy of 'main' is now version 1 (hash ...)
```

(The policy file lives next to the repository, `../policy.json`, not in it, so that it is not part of your snapshots.)
Read it as five rules:

| Field | Rule |
|---|---|
| `trusted_issuers` | An attestation counts only if it is signed by `service:ci` **and** with this exact key. An agent signing its own result proves nothing. |
| `required_validations` | The check `tests` must have passed on **the exact tree that would result from integrating**, not on some earlier branch state. |
| `approvals` | A change written by an agent needs one human approval (`min_for_agents`), and nobody approves their own change. |
| `agent_autonomy` | An agent may integrate **by itself** only a change that touches at most 5 files, all under `docs/`. |
| `require_provenance_for_agents` | A change by an agent must carry a signed record of who/what/why produced it. |

The policy is versioned and append-only (`ag policy history`), and unknown fields are rejected, so a typo cannot
quietly weaken it. The [policy cookbook](policy-cookbook.md) has more recipes; the
[reference](../guides/policy.md) has every field.

## 3. Give the agent an identity, a lease and a workspace

<!-- doctest -->
```bash
ag agent register refactor-bot
BOT=$(ag -q agent start refactor-bot)
echo "$BOT"
TOKEN=$(ag --as "$BOT" -q lease acquire 'src/**' --exclusive --ttl 30m)
ag --as "$BOT" workspace create refactor --dir ../refactor
```

<!-- expect -->
```text
Registered agent:refactor-bot (key ...). Its private key is .../hello/.ag/keys/refactor-bot.json.
agent:refactor-bot#run_...
Created workspace 'refactor' on 'main' in ../refactor
```

- `agent register` creates the agent's identity and signing key. `agent start` begins a **run** and prints the
  identity to act as: `agent:refactor-bot#run_…`. Everything the agent does is recorded under it, and `agent finish`
  ends the run and releases its leases.
- The **lease** says "I am working on `src/**`". While an *exclusive* lease is active, a change that touches those
  paths can only land if its author holds the lease **and** the integration presents the lease's **fencing token**,
  a number from a strictly increasing counter: an agent that stalled, whose lease expired and was given to
  someone else, cannot land its stale change later. `-q` printed the token, which is in `$TOKEN`.
- The **workspace** is a private directory that starts at the head of `main`; the agent edits there, and its
  changes reach no one until they are proposed.

`--as IDENTITY` runs a single command as someone else; in real use the agent's process has `AG_IDENTITY` set to its
identity instead. The agent now makes a change to the *code*:

<!-- doctest -->
```bash
printf 'pub fn add(a: i32, b: i32) -> i32 {\n    a.wrapping_add(b)\n}\n' > ../refactor/src/lib.rs
ag --as "$BOT" snapshot -m "Use wrapping_add" --workspace refactor
PROP=$(ag --as "$BOT" -q propose @refactor -t "Use wrapping_add")
```

<!-- expect -->
```text
Snapshot ... in workspace 'refactor'
```

## 4. The agent cannot land it

The agent tries to integrate its own proposal. The exit status is **4**: refused, and waiting will not help.

<!-- doctest -->
```bash
ag --as "$BOT" integrate "$PROP"     # expect-exit 4
```

<!-- expect -->
```text
Denied by policy; the stream was not changed:
  - actor not allowed to integrate: agents may not integrate unless agent_autonomy.auto_integrate is on and the change is within its limits
  - check 'tests' has no trusted passing attestation for this exact tree
  - approvals (default): 0 of 1 required
  - agent-authored change has no signed provenance
  - exclusive lease on 'src/**' held by agent:refactor-bot#run_... (stale_or_missing_token)
```

The agent is not allowed to integrate this change at all: `src/lib.rs` is outside its envelope. Now alice tries. The
exit status is **3**: *blocked*, because everything listed is something a person or a system **can provide**.

<!-- doctest -->
```bash
ag integrate "$PROP"                 # expect-exit 3
```

<!-- expect -->
```text
Blocked (policy v1); the stream was not changed:
  - check 'tests' has no trusted passing attestation for this exact tree
  - approvals (default): 0 of 1 required
  - agent-authored change has no signed provenance
  - exclusive lease on 'src/**' held by agent:refactor-bot#run_... (stale_or_missing_token)
candidate tree: ...
Provide what is missing, then run `ag integrate prop_...` again (it resumes this operation).
```

Scripts and agents do not parse that text. With `--json` the same refusal carries stable codes
(`missing_validation`, `insufficient_approvals`, `missing_provenance`, `lease`, …) and the exit status says which
kind it is (0 landed, 3 blocked, 4 refused); see [exit codes](../reference/help-topics.md#exit-codes) and the
[blocked-integration tutorial](troubleshooting-blocked-integration.md).

## 5. Provide what is missing

Each line of the list has its own answer, and they can be given in any order.

<!-- doctest -->
```bash
ag --as service:ci validate run "$PROP" --check tests --key ci -- sh -c 'test -f src/lib.rs'
ag --as "$BOT" provenance attach @refactor --agent refactor-bot --model example-model --rationale "wrapping is the intended behaviour"
ag --as user:bob proposal review "$PROP" --approve -m "looks right"
ag integrate "$PROP"                 # expect-exit 3
```

<!-- expect -->
```text
check 'tests' PASSED on tree ... (policy v1); attestation att_... signed by service:ci
Signed provenance record #1 for ... by agent:refactor-bot
Recorded 'approve' by user:bob on ...
Blocked (policy v1); the stream was not changed:
  - exclusive lease on 'src/**' held by agent:refactor-bot#run_... (stale_or_missing_token)
```

- `validate run` checked out **the exact tree integration would produce**, ran the command on it and signed the
  result as `service:ci`. (`ag validate run` runs the command on your machine *without a sandbox*; for code you
  do not trust, use the [execution plane](../guides/workers.md).) Had the stream moved afterwards, the attestation
  would no longer match and CI would have to run again.
- `provenance attach` is the agent's signed statement of what produced the change.
- bob's approval is bound to the proposal's current head: pushing another edit would reset it.
- One thing is left: the lease. The change touches `src/**`, which the agent holds exclusively, so the integration has
  to present the lease's current token.

<!-- doctest -->
```bash
ag integrate "$PROP" --fencing-token "$TOKEN"
ag log main -n 3
```

<!-- expect -->
```text
Integrated (fast-forward): stream is now at ... (version 2, was ...), 1 file(s) changed.
...
... agent:refactor-bot#run_...
    Use wrapping_add
```

The refused attempts changed nothing, and the one that succeeded moved `main` atomically with a compare-and-swap.

## 6. Inside the envelope, no human is needed

A documentation change by an agent stays within `docs/**` and five files, so the *policy* lets the agent integrate it
itself. CI and provenance are still required: the envelope removes the human, not the evidence.

<!-- doctest -->
```bash
ag agent register docs-bot
DOCS=$(ag -q agent start docs-bot)
ag --as "$DOCS" workspace create docs --dir ../docs-ws
printf '# hello\n\nA small library.\n\n## Usage\n\nCall `add`.\n' > ../docs-ws/docs/README.md
ag --as "$DOCS" snapshot -m "Document usage" --workspace docs
DOCPROP=$(ag --as "$DOCS" -q propose @docs -t "Document usage")
ag --as "$DOCS" integrate "$DOCPROP"     # expect-exit 3
ag --as service:ci validate run "$DOCPROP" --check tests --key ci -- sh -c 'test -f docs/README.md'
ag --as "$DOCS" provenance attach @docs --agent docs-bot --model example-model --rationale "the usage section was missing"
ag --as "$DOCS" integrate "$DOCPROP"
```

<!-- expect -->
```text
Blocked (policy v1); the stream was not changed:
  - check 'tests' has no trusted passing attestation for this exact tree
  - agent-authored change has no signed provenance
...
Integrated (fast-forward): stream is now at ... (version 3, was ...), 1 file(s) changed.
```

## 7. Everything is on the record

<!-- doctest -->
```bash
ag --as "$BOT" agent finish "${BOT#*#}"
ag log main
ag verify
ag audit
```

<!-- expect -->
```text
Run run_... completed; leases released
...
... agent:docs-bot#run_...
    Document usage
...
Repository is healthy.
Audit chain intact: ... entries, head ...
```

`ag log` is the history, `ag trace RUN` shows what one agent run did, `ag verify` re-hashes the stored objects and
cross-checks the metadata, and `ag audit` verifies the hash chain of the audit log: every decision above, refused or
not, is in it, and the head hash is what you would store somewhere an attacker cannot rewrite.

## What next

- Put the same repository behind a server so that several people and agents share it:
  [Run a team server](team-server.md).
- Tune the rules: [Policy cookbook](policy-cookbook.md).
- When `integrate` says *blocked* and you do not know why:
  [Troubleshooting a blocked integration](troubleshooting-blocked-integration.md).
- Coming from Git: [Migrating a Git repository](migrating-from-git.md), and [AgentGit compared](../comparison.md).
- The [agents guide](../guides/agents.md) covers tasks, leases and the full loop of an agent run.
