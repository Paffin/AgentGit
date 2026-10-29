# ADR 0029: compare the inspected proposal head before replacing it

Status: accepted.

## Context

Two browser editors or agents can prepare different descendants of the same proposal head.
An unconditional update lets the later request silently replace work already accepted from
another editor. Conflict resolution and review suggestions must not introduce this race.

## Decision

Add the optional `expected_head` field to `UpdateProposalRequest` (protobuf field 5). A caller
that inspected a revision supplies its full changeset ID. A mismatch returns HTTP 412 /
`failed_precondition`, preserving the proposal head and its update audit history. Publication
of immutable objects remains a separate operation; a contender may publish an unreferenced
changeset before losing the metadata comparison. Normal retention and garbage collection apply.

The engine compares the expected head and writes the replacement plus audit entry in one
metadata write transaction. Even legacy calls without the field compare the head read at the
start of that operation, preventing two overlapping publications from blindly overwriting one
another. Legacy calls still cannot detect work changed before their request began. Ownership,
open-state checks and head-bound approvals retain their existing semantics. No schema or
canonical object format changes.

Advertise `proposals.update_head`. Clients must negotiate it before relying on this precondition:
older servers may ignore an unknown JSON field. The browser conflict resolver requires this
feature and retains the head inspected when the dialog opened, even if live updates arrive.
The local CLI exposes `ag proposal update --expected-head`; generated SDKs expose the same
optional request field. Requests without it remain compatible. This operation still does not
promise idempotency-key deduplication; a retry should inspect current state after an ambiguous
response rather than overwrite a newer revision.

## Scope

This is the concurrency prerequisite for applying review suggestions. Structured suggestions,
atomic application and requested-reviewer workflows remain separate deliverables.
