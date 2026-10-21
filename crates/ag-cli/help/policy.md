Policy: what must be true before a change lands

A policy is a JSON document per stream. It is versioned and append-only: `ag policy set FILE` stores a new
version, `ag policy history` lists them, `ag policy show` prints the one in force. Unknown fields and
policies that can never be satisfied are rejected when you set them.

    ag policy example > policy.json     a document to start from
    ag policy set policy.json           needs an admin of the current version
    ag policy show

You do not have to write the first one: four presets are built in (solo, team, agents, strict).

    ag policy preset list               who each is for
    ag policy preset apply agents       store one (or `ag init --preset team`); `--ci-key ci` also requires a signed check
    ag policy explain                   the policy in force in plain language, read from the document itself

The fields

    integrators / admins / allowed_authors   who may integrate, change policy, author proposals
    trusted_issuers                          identities AND public keys whose attestations count
    required_validations                     checks that must have passed on the exact candidate tree
    approvals                                min, min_for_agents, approvers, forbid_self_approval
    protected_paths                          globs that need extra approvals (min_approvals, approvers)
    agent_autonomy                           auto_integrate, allowed_paths, max_files: the envelope
    require_provenance_for_agents            agent changes need a valid signed provenance record
    max_changed_files                        deny integrations that touch more files

With no policy stored: any non-agent may integrate; agents cannot integrate on their own and need one human
approval.

What the verdicts mean

    Blocked (exit 3)   something you can provide is missing: validation, approval, provenance, lease token
    Denied  (exit 4)   the policy forbids it: actor not allowed, a trusted FAIL on this tree, too many files

Validation is bound to the tree: an attestation counts only if it is signed by a trusted issuer, names this
exact candidate tree, and was made under this policy version. Changing the policy, or the stream moving,
means validating again. Approvals are bound to the proposal head: pushing a new head resets them.

`--json` output of `ag integrate` carries stable blocker codes: missing_validation, insufficient_approvals,
changes_requested, missing_provenance, lease, failed_validation, actor_not_allowed, author_not_allowed,
too_many_changed_files.

Command help: `ag policy --help`. More: docs/guides/policy.md and docs/tutorials/policy-cookbook.md
