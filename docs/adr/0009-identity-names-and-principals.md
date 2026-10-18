# ADR-0009: An identity's name cannot start with the run separator

- Status: accepted
- Date: 2026-10-18
- Relates to: [object format](../architecture/object-format.md) (`Identity`), [agents guide](../guides/agents.md) (runs),
  `dev/audit/CONFIRMED.md` (core-formats)

## Context

An identity is `kind:name`. An agent's run is written `agent:name#run`, and the *principal* of an identity is the part
before the first `#`: approval, ownership and provenance rules compare principals, so that one agent cannot approve its
own work by starting a new run (`Identity::principal`).

`Identity::new` accepted `#` anywhere in the name, including as its first character. `agent:#run1` was a valid
identity whose principal was `agent:`: not an identity at all (an empty name), and the same one for every such run,
so `agent:#a` and `agent:#b` shared a principal. The principal was built without going through validation, so nothing
noticed. Registered agent names and names minted from an OIDC subject cannot start with `#`, so reaching this took an
administrator-minted subject or a locally chosen `--as` label; but the invariant "a principal is an identity" was false,
and it decides self-approval.

## Decision

`Identity::new` rejects a name that starts with `#` (`IdentityError::BadLength`, whose message now says so). Nothing else
changes: `#` stays allowed elsewhere in a name (`agent:bot#run-1`, `agent:a#b#c`, a trailing `#`), the principal is still
the part before the first `#`, and it is now always a valid identity. A property test pins "the principal of any accepted
identity is an accepted identity, and is its own principal".

This is a format-level change in the strict sense, because identities are decoded from changesets, proposals, reviews
and credentials (`Deserialize` validates). It is *narrowing*: it changes no hash, no canonical encoding and no merge
result of any value that was valid and well-formed before.

## Consequences

- ✅ The principal of every identity is an identity; principal comparison can no longer merge unrelated runs into
  an empty name.
- ⚠️ A stored identity of the form `kind:#…` no longer decodes. No `ag` command, registry, OIDC mapping or SDK produces
  one (searched across the repository, the SDKs and the web UI), and the compatibility fixtures (`tests/compat`) still
  decode. A deployment that did create one by hand would see decode errors for the objects naming it and must rewrite
  them; there is no migration, by design, because the value was never well-formed.
- ✅ No change to object hashes, wire formats or the OpenAPI/AGWP schemas (identities are strings there).
