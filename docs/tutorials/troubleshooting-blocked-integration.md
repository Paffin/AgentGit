# Troubleshooting a blocked integration

`ag integrate` never half-works: it either moves the stream, atomically, or it changes nothing and says why. This page
is the field guide to the "why". Every message below is a **real one**: the commands run against the real `ag` in the
test suite ([how](README.md#how-these-tutorials-are-tested)), the outputs are checked, and the table at the end lists
every refusal message that the code can print, which `scripts/check_docs.py` verifies against the source.

## The three answers, and what to do with each

| You see | Exit status | Meaning | What to do |
|---|---:|---|---|
| `Integrated (...)` | 0 | the stream moved | nothing |
| `Blocked (policy vN); the stream was not changed:` | **3** | something is **missing** that someone can provide: a validation, an approval, provenance, a lease token | provide it, run the **same** command again (it resumes the same operation) |
| `Denied by policy; ...`, `Conflict: ...`, `Invalid: ...` | **4** | **refused**: a rule forbids it, the edits overlap, or the request makes no sense. Waiting will not help | change the change (or the policy, if you are its administrator), then try again |
| `error: ...` | 1 | something else went wrong: bad input, an unknown id, I/O. Not a verdict on the change | read the message and the `hint:` line below it |

Scripts and agents branch on the exit status and on the stable `blockers[].code` of `--json`, never on the text; the
text is for people. When there are several reasons you get all of them at once, and **one denial among them makes the
whole answer a 4**.

Your toolbox for finding out more:

| Command | Tells you |
|---|---|
| `ag integrate P --json` | the outcome with a stable `code` per blocker (`missing_validation`, `insufficient_approvals`, `changes_requested`, `missing_provenance`, `lease`, `failed_validation`, `actor_not_allowed`, `author_not_allowed`, `too_many_changed_files`) |
| `ag preview P` | only whether the *merge* works (conflicts, unrelated history). It does **not** evaluate policy |
| `ag proposal show P` | the proposal's head, state and reviews |
| `ag policy show` | the policy in force and its version |
| `ag op list`, `ag op show OP` | the recorded operations; an integration that was blocked is still an operation you can resume |
| `ag status` | who you are, your proposals and leases, the state of the stream |
| `ag doctor` | whether the repository, keys and (with a remote) the server are healthy |

## 1. A repository with rules

The setup is the one from [Your first agent-safe repository](first-agent-safe-repo.md), without the lecture: CI must
pass on the exact tree, one approval from someone other than the author is needed, and changes by agents need signed
provenance.

<!-- doctest -->
```bash
export AG_IDENTITY=user:alice
mkdir shop && cd shop
ag init --namespace acme/shop
mkdir -p src docs
printf 'pub fn total(items: &[u32]) -> u32 {\n    items.iter().sum()\n}\n' > src/lib.rs
printf '# shop\n' > docs/README.md
ag snapshot -m "Initial import" > /dev/null
ag integrate "$(ag -q propose -t "Initial import")" > /dev/null
KEY=$(ag -q validate keygen ci)
cat > ../policy.json <<JSON
{
  "trusted_issuers": [{ "identity": "service:ci", "public_key": "$KEY" }],
  "required_validations": [{ "check": "tests", "issuers": ["service:ci"] }],
  "approvals": { "min": 1, "forbid_self_approval": true },
  "require_provenance_for_agents": true
}
JSON
ag policy set ../policy.json
```

<!-- expect -->
```text
Policy of 'main' is now version 1 (hash ...)
```

## 2. Blocked: no passing check on this exact tree

alice proposes a change.

<!-- doctest -->
```bash
printf 'pub fn total(items: &[u32]) -> u32 {\n    items.iter().copied().sum()\n}\n' > src/lib.rs
ag snapshot -m "Copy items before summing"
P1=$(ag -q propose -t "Copy items before summing")
ag integrate "$P1"                                 # expect-exit 3
```

<!-- expect -->
```text
Snapshot ... in workspace 'default'
Blocked (policy v1); the stream was not changed:
  - check 'tests' has no trusted passing attestation for this exact tree
  - approvals (default): 0 of 1 required
candidate tree: ...
Provide what is missing, then run `ag integrate prop_...` again (it resumes this operation).
  validate:  ag validate run prop_... --check <name> --key <issuer-key> -- <command>
  approve:   ag proposal review prop_... --approve
operation op_...
```

Two things are missing, and the output names the commands that provide them. The same facts, for a program:

<!-- doctest -->
```bash
ag --json integrate "$P1" > ../blocked.json        # expect-exit 3
grep '"code"' ../blocked.json
ag preview "$P1"
```

<!-- expect -->
```text
        "code": "missing_validation",
        "code": "insufficient_approvals",
strategy: FastForward   (algorithm ...)
candidate tree: ...
would change 1 file(s): src/lib.rs
```

`preview` is happy: the change merges cleanly. Whether it is *allowed* is the policy's business, which is what
`integrate` told us. Provide the check (the CI identity runs it and signs the result):

<!-- doctest -->
```bash
ag --as service:ci validate run "$P1" --check tests --key ci -- true
```

<!-- expect -->
```text
running `true` on tree ... (NOT sandboxed)
check 'tests' PASSED on tree ... (policy v1); attestation att_... signed by service:ci
```

### "I ran the check and it still says there is no attestation"

An attestation counts only if it is signed by a trusted issuer **and** names this exact candidate tree **and** was made
under the current policy version. So the same message returns when:

- the stream moved (or the proposal's head changed) after the check ran, so the candidate tree is a different tree;
- the **policy changed**: a new version invalidates earlier attestations on purpose, because the inputs of the check changed;
- the attestation is signed by a key that is not the one in `trusted_issuers`.

Here is the second one. alice changes the policy (a limit on the size of a change; storing the *same* document again
is not a new version, says so, and invalidates nothing):

<!-- doctest -->
```bash
ag policy set ../policy.json
cat > ../policy2.json <<JSON
{
  "trusted_issuers": [{ "identity": "service:ci", "public_key": "$KEY" }],
  "required_validations": [{ "check": "tests", "issuers": ["service:ci"] }],
  "approvals": { "min": 1, "forbid_self_approval": true },
  "require_provenance_for_agents": true,
  "max_changed_files": 100
}
JSON
ag policy set ../policy2.json
ag integrate "$P1"                                 # expect-exit 3
ag --as service:ci validate run "$P1" --check tests --key ci -- true
```

<!-- expect -->
```text
Policy of 'main' is unchanged: still version 1 (hash ...)
Policy of 'main' is now version 2 (hash ...)
Blocked (policy v2); the stream was not changed:
  - check 'tests' has no trusted passing attestation for this exact tree
  - approvals (default): 0 of 1 required
...
check 'tests' PASSED on tree ... (policy v2); attestation att_... signed by service:ci
```

Note `policy v2` in the message and in the new attestation: the version is part of what was signed.

## 3. Blocked: approvals

<!-- doctest -->
```bash
ag proposal review "$P1" --approve -m "mine"
ag integrate "$P1"                                 # expect-exit 3
```

<!-- expect -->
```text
Recorded 'approve' by user:alice on ...
Blocked (policy v2); the stream was not changed:
  - approvals (default): 0 of 1 required
```

The validation is satisfied now, but alice's own approval was recorded and **did not count**, because the policy says
`forbid_self_approval`. An approval counts if the reviewer matches `approvers` (any `user:` when that is empty), is not
the author, and, for a change by an agent outside its autonomy envelope, is a human. Someone else reviews:

<!-- doctest -->
```bash
ag --as user:bob proposal review "$P1" --approve
```

<!-- expect -->
```text
Recorded 'approve' by user:bob on ...
```

### "bob approved, and now it asks for the approval again"

Approvals are bound to the **head of the proposal they were written against**. If the author changes the proposal,
approvals *and* validations of the old head are gone, because they were about different content:

<!-- doctest -->
```bash
printf 'pub fn total(items: &[u32]) -> u32 {\n    items.iter().copied().sum::<u32>()\n}\n' > src/lib.rs
ag snapshot -m "Spell out the sum type"
ag proposal update "$P1"
ag integrate "$P1"                                 # expect-exit 3
```

<!-- expect -->
```text
Snapshot ... in workspace 'default'
Proposal prop_... now points at ... (earlier approvals no longer count)
Blocked (policy v2); the stream was not changed:
  - check 'tests' has no trusted passing attestation for this exact tree
  - approvals (default): 0 of 1 required
```

That is the feature: nobody can get a different change landed under yesterday's "looks good". Run the check and ask for
the review again. And a reviewer can also say no: a `request_changes` review by an eligible reviewer blocks until that
reviewer approves again.

<!-- doctest -->
```bash
ag --as service:ci validate run "$P1" --check tests --key ci -- true > /dev/null
ag --as user:bob proposal review "$P1" --request-changes -m "please add a test"
ag integrate "$P1"                                 # expect-exit 3
```

<!-- expect -->
```text
Recorded 'request_changes' by user:bob on ...
Blocked (policy v2); the stream was not changed:
  - approvals (default): 0 of 1 required
  - user:bob requested changes
```

<!-- doctest -->
```bash
ag --as user:bob proposal review "$P1" --approve -m "thanks"
ag integrate "$P1"
ag sync
```

<!-- expect -->
```text
Recorded 'approve' by user:bob on ...
Integrated (fast-forward): stream is now at ... (version 2, was ...), 1 file(s) changed.
...
```

Every `integrate` attempt on this proposal printed the **same operation id**. A blocked integration is a recorded
operation that the next attempt resumes, so retrying is always safe: it cannot land twice.

## 4. Agents: why "denied" and "blocked" at once

An agent proposes a documentation change.

<!-- doctest -->
```bash
ag agent register helper
HELPER=$(ag -q agent start helper)
ag --as "$HELPER" workspace create helper --dir ../helper
printf '# shop\n\nA demo shop.\n' > ../helper/docs/README.md
ag --as "$HELPER" snapshot -m "Describe the shop" --workspace helper
P2=$(ag --as "$HELPER" -q propose @helper -t "Describe the shop")
ag --as "$HELPER" integrate "$P2"                  # expect-exit 4
```

<!-- expect -->
```text
Denied by policy; the stream was not changed:
  - actor not allowed to integrate: agents may not integrate unless agent_autonomy.auto_integrate is on and the change is within its limits
  - check 'tests' has no trusted passing attestation for this exact tree
  - approvals (default): 0 of 1 required
  - agent-authored change has no signed provenance
```

Four reasons, one of them a denial, so the answer is a **4**: this policy does not let an agent integrate at all (no
`agent_autonomy`). The other three would only have *blocked*. A person integrating the same proposal sees just those:

<!-- doctest -->
```bash
ag integrate "$P2"                                 # expect-exit 3
```

<!-- expect -->
```text
Blocked (policy v2); the stream was not changed:
  - check 'tests' has no trusted passing attestation for this exact tree
  - approvals (default): 0 of 1 required
  - agent-authored change has no signed provenance
```

The provenance is the agent's signed statement of what produced the change; the agent attaches it (`ag provenance
attach`). Now suppose alice has reserved `docs/**` for herself with an exclusive lease, as an administrator doing a
big edit would:

<!-- doctest -->
```bash
ag lease acquire 'docs/**' --exclusive --ttl 30m
ag --as service:ci validate run "$P2" --check tests --key ci -- true > /dev/null
ag --as "$HELPER" provenance attach @helper --agent helper --model example-model
ag --as user:bob proposal review "$P2" --approve
ag integrate "$P2"                                 # expect-exit 3
```

<!-- expect -->
```text
Exclusive lease lease_... on 'docs/**' until ...
fencing token: 1   (pass `--fencing-token 1` to `ag integrate`)
...
Blocked (policy v2); the stream was not changed:
  - exclusive lease on 'docs/**' held by user:alice (held_by_other)
```

Only the lease is left, and its reason is in parentheses: `held_by_other` means another principal holds an exclusive
lease on a path the change touches (the change's author is not the holder), and `stale_or_missing_token` means the
author *is* the holder but the integration did not present the lease's current fencing token (as in the first
tutorial: pass `--fencing-token N`). Here the answer is to wait for, ask for, or release the lease:

<!-- doctest -->
```bash
LEASE=$(ag -q lease list | cut -d' ' -f1)
ag lease release "$LEASE"
ag integrate "$P2"
```

<!-- expect -->
```text
Released lease_...
Integrated (fast-forward): stream is now at ... (version 3, was ...), 1 file(s) changed.
```

## 5. Denied: a rule forbids it

Some policies cannot be satisfied by providing something. alice makes the rules stricter: only she may integrate, only
users may author, at most one file per integration.

<!-- doctest -->
```bash
cat > ../strict.json <<JSON
{
  "integrators": ["user:alice"],
  "allowed_authors": ["user:*"],
  "max_changed_files": 1
}
JSON
ag policy set ../strict.json
ag workspace create wide --dir ../wide
printf '# shop\n\nTwo files.\n' > ../wide/docs/README.md
printf 'pub fn count() -> u32 {\n    0\n}\n' > ../wide/src/count.rs
ag --as user:bob snapshot -m "Two files" --workspace wide
P3=$(ag --as user:bob -q propose @wide -t "Two files")
ag integrate "$P3"                                 # expect-exit 4
ag --as user:bob integrate "$P3"                   # expect-exit 4
ag --as "$HELPER" integrate "$P3"                  # expect-exit 4
```

<!-- expect -->
```text
Policy of 'main' is now version 3 (hash ...)
Created workspace 'wide' on 'main' in ../wide
Snapshot ... in workspace 'wide'
Denied by policy; the stream was not changed:
  - 2 files changed, policy allows at most 1
Denied by policy; the stream was not changed:
  - actor not allowed to integrate: user:bob is not in the policy's integrators
  - 2 files changed, policy allows at most 1
Denied by policy; the stream was not changed:
  - actor not allowed to integrate: agents may not integrate unless agent_autonomy.auto_integrate is on and the change is within its limits
  - 2 files changed, policy allows at most 1
```

None of these has a "provide it" answer: split the change into two proposals, ask an integrator, or have the policy's
administrator change the policy (`ag policy set`, which needs an admin of the *current* version). The fourth
denial, `author ... is not allowed to propose to this stream`, appears when a proposal's author does not match
`allowed_authors` (here an agent's proposal would show it, since only `user:*` may author).

The last denial is the one that cannot be waited out by providing evidence: a check that **failed**. A trusted FAIL
attestation for this exact tree denies the change even if a PASS also exists ("failure wins"):

<!-- doctest -->
```bash
ag policy set ../policy.json > /dev/null
ag proposal close "$P3"
printf 'pub fn total(items: &[u32]) -> u32 {\n    items.iter().copied().fold(0, |a, b| a + b)\n}\n' > src/lib.rs
ag snapshot -m "Use fold"
P4=$(ag -q propose -t "Use fold")
ag --as service:ci validate run "$P4" --check tests --key ci -- false      # expect-exit 4
ag integrate "$P4"                                 # expect-exit 4
```

<!-- expect -->
```text
Closed prop_...
Snapshot ... in workspace 'default'
running `false` on tree ... (NOT sandboxed)
check 'tests' FAILED on tree ... (policy v4); attestation att_... signed by service:ci
Denied by policy; the stream was not changed:
  - check 'tests' FAILED on this exact tree (attestation att_...)
  - approvals (default): 0 of 1 required
```

`validate run` itself exits 4 when the check fails and records the failure. The remedy is a different change: fix the
code, `ag snapshot`, `ag proposal update`, and the new head has a new tree and a clean slate.

## 6. Conflict: the edits overlap

Reset to an open policy to talk about merging. bob and alice both change the same line; alice's lands first.

<!-- doctest -->
```bash
echo '{}' > ../open.json
ag policy set ../open.json > /dev/null
ag proposal close "$P4"
ag sync
ag workspace create bob --dir ../bob
printf 'pub fn total(items: &[u32]) -> u32 {\n    items.iter().sum::<u32>() + 1\n}\n' > ../bob/src/lib.rs
ag --as user:bob snapshot -m "Plus one" --workspace bob
PB=$(ag --as user:bob -q propose @bob -t "Plus one")
printf 'pub fn total(items: &[u32]) -> u32 {\n    items.iter().sum::<u32>() + 2\n}\n' > src/lib.rs
ag snapshot -m "Plus two"
PA=$(ag -q propose -t "Plus two")
ag integrate "$PA" > /dev/null
ag integrate "$PB"                                 # expect-exit 4
```

<!-- expect -->
```text
Conflict: the proposal cannot be merged automatically; the stream was not changed.
  src/lib.rs  (content conflict (1 region(s)))
merge base: ...
Fix: `ag sync`, resolve the marked files, `ag snapshot`, `ag proposal update <id>`.
```

The stream was not touched. The fix is spelled out in the message: bring the stream's head into the workspace (the
clean parts merge, the overlapping ones get markers), resolve, snapshot, update the proposal.

<!-- doctest -->
```bash
ag --as user:bob sync --workspace bob              # expect-exit 4
cat ../bob/src/lib.rs
printf 'pub fn total(items: &[u32]) -> u32 {\n    items.iter().sum::<u32>() + 3\n}\n' > ../bob/src/lib.rs
ag --as user:bob snapshot -m "Plus three" --workspace bob
ag --as user:bob proposal update "$PB" @bob
ag integrate "$PB"
```

<!-- expect -->
```text
1 conflict(s). Resolve the files below, then run `ag snapshot -m "resolve"`:
  src/lib.rs  (content conflict (1 region(s)))
pub fn total(items: &[u32]) -> u32 {
<<<<<<< ours
    items.iter().sum::<u32>() + 1
=======
    items.iter().sum::<u32>() + 2
>>>>>>> theirs
}
Snapshot ... in workspace 'bob'
Proposal ... now points at ...
Integrated (fast-forward): stream is now at ... (version 5, was ...), 1 file(s) changed.
```

## 7. Invalid: the request makes no sense

<!-- doctest -->
```bash
ag stream create side
ag workspace create stray --dir ../stray --stream side
printf 'x\n' > ../stray/x.txt
ag snapshot -m "From elsewhere" --workspace stray
PS=$(ag -q propose @stray -t "From elsewhere" --stream main)
ag integrate "$PS"                                 # expect-exit 4
```

<!-- expect -->
```text
Snapshot ... in workspace 'stray'
Invalid: the proposal's history is unrelated to the stream's history
```

This proposal was written in a workspace on another stream, with no common ancestor with `main`, so there is no merge
to compute (`preview` reports the same as `problem:`). Rarer `Invalid:` reasons are `criss-cross history: …
equally good merge bases …` (the history has more than one best common ancestor and the merge refuses to guess: merge
the stream into the proposal by hand first) and `expired` (see the table). Unrelated history is fixed by rebuilding the
change on the stream's head.

## 8. Every refusal message

These are all the reasons that `ag integrate` (and `ag remote integrate`, which prints the same) can give, as they appear
in the code. `{name}` is a placeholder that the message fills in. `scripts/check_docs.py` fails if a message in this table
is no longer in the source, so the table cannot drift.

<!-- refusals -->
| Message | Exit | `--json` code or `outcome.status` | Provide / fix |
|---|---:|---|---|
| `check '{check}' has no trusted passing attestation for this exact tree` | 3 | `missing_validation` | `ag validate run` as a trusted issuer, on the current head, under the current policy version |
| `approvals ({scope}): {have} of {required} required` | 3 | `insufficient_approvals` | reviews by eligible people of the current head (`default` or a protected-path glob) |
| `{reviewer} requested changes` | 3 | `changes_requested` | that reviewer approves again |
| `agent-authored change has no signed provenance` | 3 | `missing_provenance` | `ag provenance attach` |
| `exclusive lease on '{scope}' held by {holder} ({reason})` | 3 | `lease` | `held_by_other`: the holder releases or the lease expires; `stale_or_missing_token`: `--fencing-token N` |
| `actor not allowed to integrate: {reason}` | 4 | `actor_not_allowed` | integrate as someone the policy lists, or change the policy |
| `author {author} is not allowed to propose to this stream` | 4 | `author_not_allowed` | propose as an allowed author |
| `check '{check}' FAILED on this exact tree (attestation {attestation})` | 4 | `failed_validation` | fix the code; a new head has a new tree |
| `{changed} files changed, policy allows at most {max}` | 4 | `too_many_changed_files` | split the change |
| `the proposal's history is unrelated to the stream's history` | 4 | `invalid` | rebuild the change on the stream's head |
| `criss-cross history: {n} equally good merge bases` | 4 | `invalid` | merge the stream into the proposal by hand, then update it |
| `expired` | 4 | `invalid` | a blocked operation that nobody resumed was expired by `ag recover`; run `ag integrate` again to start a new one |
| `Conflict: the proposal cannot be merged automatically` | 4 | `conflict` | `ag sync`, resolve, `ag snapshot`, `ag proposal update` |

(`{reason}` after `actor not allowed to integrate:` is one of two sentences: `{actor} is not in the policy's integrators`,
or `agents may not integrate unless agent_autonomy.auto_integrate is on and the change is within its limits`.)

## When it is none of these

- `error: permission_denied: this identity (...) may not perform 'integrate.run' on '...'` is the **server's role
  check**, not the policy: you need the `integrator` role ([team server tutorial](team-server.md)).
- `error: not found: proposal '...'` : a wrong or ambiguous id; any unique prefix of at least six characters works.
- A response was lost and you do not know whether the integration happened: `ag op list`, `ag op show OP`; repeating the
  same `integrate` replays the recorded result instead of acting twice.
- Something looks corrupt or half-done: `ag doctor`, `ag recover`, `ag verify`, and
  [Troubleshooting](../operations/troubleshooting.md) for the storage and platform errors.
