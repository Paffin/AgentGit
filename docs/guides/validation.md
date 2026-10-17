# Validation and attestations

"Tests passed" is only meaningful if you can say **on what**. AgentGit binds a validation to the exact
*tree* (resulting repository state) of the candidate, the policy version and the environment, and signs it.

## The model

An **attestation** says: *issuer I states that check C had result R on tree T, under policy version P, in
environment E, at time t.* It is signed with I's Ed25519 key over a domain-separated message
([format](../architecture/object-format.md)). The policy lists which `(identity, public key)` pairs it
trusts. Consequences:

- an agent that signs "pass" with its own key proves nothing — its key is not a trusted issuer;
- a trusted issuer's signature cannot be moved to another tree, policy version or environment;
- when the stream moves, the candidate (merge) tree changes, the old attestation no longer matches, and the
  change is blocked until the new candidate is validated — "validated yesterday" is never reused for
  "today's tree" ([`attestation_cannot_be_reused_after_the_stream_moves`](../architecture/publication-and-integration.md)).
  If the new candidate happens to have an identical tree, the attestation still applies.

## Using the built-in local runner

```bash
ag validate keygen ci
# → prints {"identity":"service:ci","public_key":"…"} to put in the policy's trusted_issuers

ag policy set policy.json            # trusted_issuers + required_validations: [{"check":"tests"}]
ag integrate <proposal>              # blocked: prints the candidate tree
ag validate run <proposal> --check tests --key ci -- cargo test --workspace
ag integrate <proposal>              # lands
```

`validate run`:

1. computes the proposal's exact candidate (`ag preview` semantics) and refuses if it conflicts;
2. checks that tree out into a fresh temporary directory;
3. runs your command there with a **cleared environment** (only `PATH`, `HOME`=the temp dir, `AG_TREE_ID`,
   `AG_PROPOSAL`);
4. signs and stores a PASS (exit status 0) or FAIL attestation for that tree, the current policy version
   and an environment digest (SHA-256 of the command and platform, or `--env`).

Exit code 0 if the check passed, 4 if it failed (the FAIL attestation is recorded and **denies** that tree).

> ⚠️ **The runner is not a sandbox.** The command has your user's file and network access. It is for code
> you trust and for local workflows. Running untrusted code needs an isolated execution plane, which is on
> the [roadmap](../../ROADMAP.md). Keep issuer keys out of any environment that runs untrusted code.

## Your own CI today

An external system that can run the Rust API can sign attestations itself:

```rust
let att = Attestation::sign(&issuer_key, id, tree_id, "tests".into(), CheckResult::Pass,
                            policy_version, env_digest, issuer_identity, now)?;
repo.submit_attestation(&att)?;        // verifies the signature, stores it; the policy decides if it counts
```

`Repository::submit_attestation` accepts any validly signed attestation (bounded at 64 per tree); only
those from `trusted_issuers` ever satisfy a policy. Remote execution planes that submit attestations over
a network arrive with the server ([roadmap](../../ROADMAP.md)).

## Inspecting

```bash
ag validate list <tree-id>       # attestations recorded for a tree
ag trace <tree-id>               # who submitted what, when (audit trail)
```
