# Policy cookbook

Recipes for the question "what must be true before a change lands on this stream?". Each recipe is a complete policy
document you can store with `ag policy set FILE`; combine them by merging their fields (a policy is one JSON object).
The reference for every field is the [Policy guide](../guides/policy.md); `ag help policy` is the short version.

**These recipes are tested.** Every JSON block on this page is parsed by the real policy parser in the test suite
(`cargo test -p agentgit-policy --test cookbook` fails if a recipe stops being valid, or if a block lacks a marker), and the
recipes marked *run* are stored in a real repository by the doc-snippet harness and exercised with real proposals
([how](README.md#how-these-tutorials-are-tested)). What a recipe *means* is what the guide says it means; where this page
makes a claim that goes beyond a run, it says so.

Two things to know before you start:

- **The sample key.** Every `public_key` below is the same sample, `5091ec77…`. It is a valid key, but nobody holds its
  private half, so the sample CI identity can never sign anything. Replace it with the output of
  `ag validate keygen NAME` (or `ag-worker keygen`) for your CI.
- **A policy that differs from the one in force is a new version**, and changing the policy invalidates the
  attestations made under the old one, on purpose. Storing the same document again changes nothing (no new version,
  no audit entry, no attestation lost), so a deploy that sets the policy every time is safe. `ag policy history` lists
  the versions.
  Only an admin of the *current* version may replace it, and unknown fields are rejected so that a typo cannot weaken a
  policy: that is the first of the [mistakes](#mistakes-the-parser-catches) below.

## 1. Solo project: no policy

Do nothing. With no policy stored, any person may integrate, there are no required checks or approvals, and agents may
not integrate on their own (a change by an agent needs one human approval). `ag policy show` says so:

<!-- doctest -->
```bash
export AG_IDENTITY=user:alice
mkdir solo && cd solo
ag init --namespace me/solo
ag policy show | head -1
```

<!-- expect -->
```text
# policy v0 of stream 'main' (built-in default; nothing stored yet)
```

## 2. A small team with CI and one review

*Run.* CI must pass on the exact tree; one person other than the author must approve. This is the policy to start with.

<!-- policy: team-ci -->
```json
{
  "trusted_issuers": [
    { "identity": "service:ci", "public_key": "5091ec77f7483bc470cac7de2c8155f56817e040a7ec5b97c41e30539cc7e803" }
  ],
  "required_validations": [{ "check": "tests", "issuers": ["service:ci"] }],
  "approvals": { "min": 1, "forbid_self_approval": true }
}
```

`trusted_issuers` pins identity **and** key: a check counts only when it is signed by that identity with that key.
`issuers` on the validation narrows which trusted issuers may satisfy *this* check. Applied to a repository, with the
sample key swapped for a real one:

<!-- doctest -->
```bash
cd .. && mkdir team && cd team
ag init --namespace acme/team
printf 'pub fn one() -> u32 {\n    1\n}\n' > lib.rs
ag snapshot -m "Initial import" > /dev/null
ag integrate "$(ag -q propose -t "Initial import")" > /dev/null
SAMPLE=5091ec77f7483bc470cac7de2c8155f56817e040a7ec5b97c41e30539cc7e803
KEY=$(ag -q validate keygen ci)
sed "s/$SAMPLE/$KEY/" "$AGDT_POLICIES/team-ci.json" > ../team-ci.json
ag policy set ../team-ci.json
printf 'pub fn one() -> u32 {\n    1 + 0\n}\n' > lib.rs
ag snapshot -m "Change"
P=$(ag -q propose -t "Change")
ag integrate "$P"                                      # expect-exit 3
ag --as service:ci validate run "$P" --check tests --key ci -- true > /dev/null
ag --as user:bob proposal review "$P" --approve > /dev/null
ag integrate "$P"
```

<!-- expect -->
```text
Policy of 'main' is now version 1 (hash ...)
Snapshot ... in workspace 'default'
Blocked (policy v1); the stream was not changed:
  - check 'tests' has no trusted passing attestation for this exact tree
  - approvals (default): 0 of 1 required
...
Integrated (fast-forward): stream is now at ... (version 2, was ...), 1 file(s) changed.
```

## 3. Four eyes on sensitive paths

*Run.* Everything needs one approval; a change that touches CI configuration or the security policy needs two, from a
short list of people.

<!-- policy: sensitive-paths -->
```json
{
  "trusted_issuers": [
    { "identity": "service:ci", "public_key": "5091ec77f7483bc470cac7de2c8155f56817e040a7ec5b97c41e30539cc7e803" }
  ],
  "required_validations": [{ "check": "tests", "issuers": ["service:ci"] }],
  "approvals": { "min": 1 },
  "protected_paths": [
    { "glob": ".github/**", "min_approvals": 2, "approvers": ["user:alice", "user:bob", "user:carol"] },
    { "glob": "SECURITY.md", "min_approvals": 2, "approvers": ["user:alice", "user:bob", "user:carol"] }
  ]
}
```

The rules **add up**: the global `min` still applies, and each protected rule that matches a changed path adds its own
count, from its own `approvers` (or the global eligibility if that list is empty). Path globs use `*` within a path
component and `**` across components. `min_approvals` must be at least 1 (a protected path that needs no approval is
rejected as a mistake).

<!-- doctest -->
```bash
cd .. && mkdir site && cd site
ag init --namespace acme/site
mkdir -p src .github/workflows
printf 'pub fn one() -> u32 {\n    1\n}\n' > src/lib.rs
printf 'name: ci\n' > .github/workflows/ci.yml
ag snapshot -m "Initial import" > /dev/null
ag integrate "$(ag -q propose -t "Initial import")" > /dev/null
KEY=$(ag -q validate keygen ci)
sed "s/$SAMPLE/$KEY/" "$AGDT_POLICIES/sensitive-paths.json" > ../sensitive.json
ag policy set ../sensitive.json
printf 'pub fn one() -> u32 {\n    1 + 0\n}\n' > src/lib.rs
printf 'name: ci\non: push\n' > .github/workflows/ci.yml
ag snapshot -m "Touch code and CI config"
P=$(ag -q propose -t "Touch code and CI config")
ag --as service:ci validate run "$P" --check tests --key ci -- true > /dev/null
ag --as user:bob proposal review "$P" --approve > /dev/null
ag integrate "$P"                                      # expect-exit 3
ag --as user:carol proposal review "$P" --approve > /dev/null
ag integrate "$P"
```

<!-- expect -->
```text
Blocked (policy v1); the stream was not changed:
  - approvals (.github/**): 1 of 2 required
...
Integrated (fast-forward): stream is now at ... (version 2, was ...), 2 file(s) changed.
```

## 4. Agents may update docs on their own; humans review code

The recipe from [the first tutorial](first-agent-safe-repo.md): an agent that changes at most five files, all under
`docs/`, may integrate by itself once CI and provenance are in; anything else needs a human.

<!-- policy: agent-docs -->
```json
{
  "trusted_issuers": [
    { "identity": "service:ci", "public_key": "5091ec77f7483bc470cac7de2c8155f56817e040a7ec5b97c41e30539cc7e803" }
  ],
  "required_validations": [{ "check": "tests", "issuers": ["service:ci"] }],
  "approvals": { "min": 0, "min_for_agents": 1, "forbid_self_approval": true },
  "agent_autonomy": { "auto_integrate": true, "allowed_paths": ["docs/**"], "max_files": 5 },
  "require_provenance_for_agents": true
}
```

"Inside the envelope" means `max_files > 0`, at most `max_files` files changed, every changed path matches
`allowed_paths`, and no protected path is touched. Outside it, an agent-authored change needs `max(min, min_for_agents)`
approvals, **from humans**. `auto_integrate` without `allowed_paths` is rejected (see the mistakes below);
`max_files: 0` is accepted but means that nothing is ever inside the envelope.

## 5. Only the merge queue lands changes

Humans propose and review; a service identity runs `ag queue run`, and nobody else may integrate. A person's attempt
is *denied* (exit 4), not blocked.

<!-- policy: merge-queue -->
```json
{
  "integrators": ["service:merge-queue"],
  "admins": ["user:alice"],
  "trusted_issuers": [
    { "identity": "service:ci", "public_key": "5091ec77f7483bc470cac7de2c8155f56817e040a7ec5b97c41e30539cc7e803" }
  ],
  "required_validations": [{ "check": "tests", "issuers": ["service:ci"] }],
  "approvals": { "min": 1, "forbid_self_approval": true }
}
```

`integrators` and `admins` take identity patterns (`user:*`, `agent:docs-*`, `service:merge-queue`); an empty
`integrators` means "any non-agent", and an empty `admins` means "any `user:`", so **set `admins` whenever you restrict
`integrators`**, or any user can replace your policy. (Why this is a policy question and not a server role: the server's
role decides who may *ask*; the policy decides what must be true. In a server deployment you need both: the queue's
identity needs the `integrator` role there.)

## 6. Fresh evidence only

A passing check older than an hour is not good enough, and the check must have run in a known environment (the digest
your CI reports; `ag validate run --env` sets it, and a worker reports the one it computed).

<!-- policy: fresh-checks -->
```json
{
  "trusted_issuers": [
    { "identity": "service:ci", "public_key": "5091ec77f7483bc470cac7de2c8155f56817e040a7ec5b97c41e30539cc7e803" }
  ],
  "required_validations": [
    {
      "check": "tests",
      "issuers": ["service:ci"],
      "max_age_secs": 3600,
      "environment_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
    }
  ]
}
```

Attestations dated more than five minutes in the future are ignored too. The sample digest is a placeholder: use the
value your CI produces. *Not run:* the clock-dependent part (an hour passing) is not exercised by the tests.

## 7. Checks that run on a trusted worker

Instead of trusting whatever a CI says, let the policy say *how* to run the check, and let an
[execution-plane worker](../guides/workers.md) run it in a sandbox and sign the result. The command lives in the
policy, never in the repository under test: the code being checked does not get to choose the command that checks it.

<!-- policy: worker-checks -->
```json
{
  "trusted_issuers": [
    { "identity": "service:ci-1", "public_key": "5091ec77f7483bc470cac7de2c8155f56817e040a7ec5b97c41e30539cc7e803" }
  ],
  "required_validations": [
    {
      "check": "tests",
      "issuers": ["service:ci-1"],
      "run": {
        "argv": ["cargo", "test", "--offline"],
        "timeout_secs": 1200,
        "memory_mb": 4096,
        "max_pids": 512,
        "network": false,
        "env": { "RUST_BACKTRACE": "1" }
      }
    }
  ],
  "approvals": { "min": 1 }
}
```

`argv` runs without a shell; `network` is off unless the policy turns it on *and* the worker allows it. The worker's
public key must be in `trusted_issuers` under the same identity, otherwise it runs checks that count for nothing
(`ag-worker keygen` prints the snippet). *Not run here:* this needs a server and a worker; the end-to-end test of that
path is in `crates/ag-worker`, and the guide above says what is verified where.

## 8. A release stream

Stricter than `main`: two independent checks, two approvals, a short list of integrators, a cap on the size of one
integration, and CI evidence no older than a day.

<!-- policy: release -->
```json
{
  "integrators": ["user:alice", "service:merge-queue"],
  "admins": ["user:alice"],
  "allowed_authors": ["user:*"],
  "trusted_issuers": [
    { "identity": "service:ci", "public_key": "5091ec77f7483bc470cac7de2c8155f56817e040a7ec5b97c41e30539cc7e803" },
    { "identity": "service:security-scan", "public_key": "5091ec77f7483bc470cac7de2c8155f56817e040a7ec5b97c41e30539cc7e803" }
  ],
  "required_validations": [
    { "check": "tests", "issuers": ["service:ci"], "max_age_secs": 86400 },
    { "check": "audit", "issuers": ["service:security-scan"], "max_age_secs": 86400 }
  ],
  "approvals": { "min": 2, "approvers": ["user:alice", "user:bob", "user:carol"], "forbid_self_approval": true },
  "protected_paths": [{ "glob": "deploy/**", "min_approvals": 3 }],
  "max_changed_files": 50
}
```

`allowed_authors` limits who may *open* a proposal to this stream: an agent's proposal is denied with `author ... is not
allowed to propose to this stream`. (The two issuers share the sample key here only because the sample is the one valid
key on this page; real issuers have different keys.) Per-stream policies mean a release stream and `main` can have
different rules in the same repository.

## 9. Cap the blast radius

An agent (or a script) that rewrites half the tree is a smell whatever it says in its commit message. This limits one
integration to 25 changed files; past that it is *denied* (exit 4), and the answer is to split the change.

<!-- policy: small-changes -->
```json
{ "max_changed_files": 25 }
```

`max_changed_files: 0` means unlimited.

## Mistakes the parser catches

A policy that could never be satisfied, or that does not say what you think, is rejected when you set it, not
discovered when someone is blocked. Each block below is fed to the real parser by the test, which checks that it fails
and that the error says what is written in the marker.

A typo in a field name (unknown fields are rejected):

<!-- policy-invalid: typo-in-field: unknown field -->
```json
{ "requird_validations": [{ "check": "tests" }] }
```

A required check that nobody is trusted to sign:

<!-- policy-invalid: no-issuers: could never be satisfied -->
```json
{ "required_validations": [{ "check": "tests" }] }
```

A check that names an issuer that is not trusted:

<!-- policy-invalid: unknown-issuer: not in trusted_issuers -->
```json
{
  "trusted_issuers": [
    { "identity": "service:ci", "public_key": "5091ec77f7483bc470cac7de2c8155f56817e040a7ec5b97c41e30539cc7e803" }
  ],
  "required_validations": [{ "check": "tests", "issuers": ["service:other"] }]
}
```

Autonomy for agents without saying where:

<!-- policy-invalid: auto-integrate-nowhere: requires non-empty allowed_paths -->
```json
{ "agent_autonomy": { "auto_integrate": true } }
```

A protected path that needs no approval:

<!-- policy-invalid: protected-without-approvals: min_approvals >= 1 -->
```json
{ "protected_paths": [{ "glob": "deploy/**", "min_approvals": 0 }] }
```

A glob that does not compile:

<!-- policy-invalid: broken-glob: invalid policy -->
```json
{ "protected_paths": [{ "glob": "[", "min_approvals": 1 }] }
```

A trusted worker's command that is not a command:

<!-- policy-invalid: empty-argv: run.argv must name a command -->
```json
{
  "trusted_issuers": [
    { "identity": "service:ci-1", "public_key": "5091ec77f7483bc470cac7de2c8155f56817e040a7ec5b97c41e30539cc7e803" }
  ],
  "required_validations": [{ "check": "tests", "run": { "argv": [] } }]
}
```

Not caught: a policy that is valid but wrong for you, such as `"approvers": ["user:*"]` where you meant a team. The
parser checks syntax and satisfiability, not intent; `ag policy show`, a dry run on a scratch stream (`ag stream create`)
and the [troubleshooting tutorial](troubleshooting-blocked-integration.md) are how you check intent.

## Changing policy safely

<!-- doctest -->
```bash
ag policy show | head -1
ag policy history
ag policy set ../team-ci.json
ag --json policy history | grep -c '"version"'
```

<!-- expect -->
```text
# policy v1 of stream 'main'
...
Policy of 'main' is now version 2 (hash ...)
2
```

- Test a change on a scratch stream first: `ag stream create staging`, `ag policy set FILE --stream staging`.
- Remember the side effect: attestations made under version 1 do not count under version 2, so open proposals need their
  checks re-run ([why](troubleshooting-blocked-integration.md#i-ran-the-check-and-it-still-says-there-is-no-attestation)).
- The policy is part of the audit trail: `ag audit` and `ag trace` show who set which version when.
