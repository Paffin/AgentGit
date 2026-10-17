# Policy reference

A policy is a JSON document per stream. It is **versioned and append-only**: `ag policy set` stores a new
version; old versions stay readable (`ag policy history`). It is evaluated as a pure function of facts
read in the transaction that commits the integration ([how](../architecture/publication-and-integration.md)),
so there is no window between "checked" and "committed".

```bash
ag policy example        # prints an example document to start from
ag policy set policy.json
ag policy show           # the policy in force and its version (0 = built-in default)
```

Documents are parsed strictly: **unknown fields are rejected** (a typo must not silently weaken a policy),
and policies that could never be satisfied are rejected on load (for example a required validation with no
`trusted_issuers`).

## Default (no policy stored)

Anyone who is not an agent may integrate; no validation, no approvals, no limits. Agents cannot integrate on
their own (see `agent_autonomy`), and agent-authored changes need one human approval
(`approvals.min_for_agents = 1`).

## Fields

```jsonc
{
  "integrators":  ["user:*", "service:merge-queue"],   // who may run `ag integrate`
  "admins":       ["user:root"],                       // who may change the policy / close others' proposals
  "allowed_authors": ["user:*", "agent:*"],            // who may author proposals to this stream
  "trusted_issuers": [ { "identity": "service:ci", "public_key": "<64 hex>" } ],
  "required_validations": [
    { "check": "tests", "issuers": ["service:ci"], "environment_digest": "sha256:…", "max_age_secs": 86400 }
  ],
  "approvals": {
    "min": 1,                       // approvals for every integration
    "min_for_agents": 1,            // …when an agent authored it and it is outside the autonomy envelope
    "approvers": ["user:*"],        // who counts; empty = any user:… identity
    "forbid_self_approval": true
  },
  "protected_paths": [ { "glob": ".github/**", "min_approvals": 2, "approvers": ["user:admin*"] } ],
  "agent_autonomy": { "auto_integrate": true, "allowed_paths": ["docs/**"], "max_files": 10 },
  "require_provenance_for_agents": true,
  "max_changed_files": 500
}
```

Identity patterns are globs over the identity string (`user:*`, `agent:docs-*`, `service:ci`); a pattern
matches the full identity or its principal (so `agent:bot` matches `agent:bot#run_1`). Path globs use
`*` within a path component and `**` across components.

### Who may do what

| Field | Empty means | Notes |
|---|---|---|
| `integrators` | any non-agent | agents are only let in through `agent_autonomy` (or by being listed explicitly) |
| `admins` | any `user:` identity | changing policy requires being an admin of the **current** policy version |
| `allowed_authors` | anyone | otherwise the proposal is `Denied` (`author_not_allowed`) |

### Validation

For each `required_validations` entry there must be an attestation that is **all of**:

- a valid Ed25519 signature over its content;
- from an identity **and** public key listed in `trusted_issuers` (and in the entry's `issuers`, if given);
- about **this exact tree** (the candidate's tree ID — see [Concepts](../concepts.md));
- under **this policy version** (changing the policy invalidates earlier attestations on purpose: the
  inputs of the check changed);
- with the entry's `environment_digest` if given, and not older than `max_age_secs` if given;
  attestations dated more than five minutes in the future are ignored.

A trusted, correctly bound **FAIL** attestation makes the change `Denied` even if a PASS also exists:
failure wins. A missing one is only `Blocked` (`missing_validation`).

### Approvals

Reviews attach to the **proposal head** they were written against; if the head moves, earlier reviews no
longer count. The latest decision per reviewer *principal* wins. An approval counts if the reviewer matches
`approvers` (or is a `user:` when empty), is not the author's principal (when `forbid_self_approval`), and
— for agent-authored changes outside the envelope — is a human. An eligible `request_changes` blocks
(`changes_requested`) until that reviewer approves again.

`protected_paths` add requirements per matching rule when the integration changes a matching path: at
least `min_approvals` approvals from that rule's `approvers` (or the global eligibility if empty).

### Agent autonomy

A change is **inside the envelope** if `max_files > 0`, it changes at most `max_files` files, every changed
path matches `allowed_paths`, and it touches no protected path.

- Agent-authored and **inside**: needs only `approvals.min` (often 0).
- Agent-authored and **outside**: needs `max(min, min_for_agents)` approvals from humans.
- An agent may *run* the integration only if `auto_integrate` is on **and** the change is inside the
  envelope; otherwise `actor_not_allowed`.
- `require_provenance_for_agents` blocks agent-authored changes without a valid signed provenance record.

`auto_integrate` requires a non-empty `allowed_paths`. With `max_files` left at 0 nothing is ever inside the
envelope.

### Limits

`max_changed_files` (0 = unlimited): integrations changing more files are `Denied`.

## Outcome codes

`ag integrate --json` returns `outcome.status` ∈ `integrated`, `blocked`, `conflict`, `denied`, `invalid`,
and for blocked/denied a list of `blockers[]` with a stable `code`:

| code | meaning | exit |
|---|---|---|
| `missing_validation` | no trusted PASS for this exact tree | 3 |
| `insufficient_approvals` | not enough eligible approvals of the current head (`detail.scope`: `default` or a glob) | 3 |
| `changes_requested` | an eligible reviewer asked for changes | 3 |
| `missing_provenance` | agent-authored change without signed provenance | 3 |
| `lease` | an exclusive lease on the scope is held by someone else, or the token is stale/missing | 3 |
| `failed_validation` | a trusted FAIL exists for this exact tree | 4 |
| `actor_not_allowed`, `author_not_allowed` | role/pattern mismatch | 4 |
| `too_many_changed_files` | over `max_changed_files` | 4 |

(The exit code is 4 when any blocker is a denial, otherwise 3.)

## Recipes

**Solo project, just keep history safe:** no policy.

**Team with CI:** trusted CI issuer + `required_validations: [tests]` + `approvals.min: 1`.

**Agents on docs, humans on code:** the `agent_autonomy` example above plus `require_provenance_for_agents`
and a CI validation.

**Sensitive paths:** `protected_paths` for `.github/**`, `SECURITY.md`, deployment config, each with
`min_approvals: 2` and a short `approvers` list.
