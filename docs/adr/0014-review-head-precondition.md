# ADR-0014: Bind a submitted review to the head the caller inspected

- Status: accepted; verification recorded with the implementation
- Date: 2026-10-27

## Context

A stored review already names a proposal head. Previously, the server chose that head when the request arrived. If the author updated the proposal after a reviewer inspected the diff but before submitting, an approval could be recorded against code the reviewer had not seen. A browser warning alone cannot close this race.

## Decision

Add `expected_head` as field 6 of `ReviewProposalRequest`. An empty value retains the existing current-head behavior for older clients. A nonempty value is a changeset ID; it must match the proposal head inside the same metadata write transaction that inserts the review and its audit event. A mismatch returns `failed_precondition` (HTTP 412) without creating a review or audit event. Existing role checks still apply.

The Web UI sends the head associated with the review draft. A visible head change preserves the draft and requires the reviewer to inspect and explicitly acknowledge the new code. A race after that acknowledgement is still rejected by the server; an error preserves the text. This does not make review submission idempotent.

## Alternatives and consequences

Checking the head only in the UI, or reading it before the write transaction, leaves a concurrent-update race. Requiring the new field immediately would break existing clients; the optional field allows an incremental upgrade. Older clients that omit it retain their previous semantics and do not gain this protection. The server advertises `proposals.review_head`. The Web UI checks that feature before submitting and preserves the draft with an upgrade explanation if it is absent; an older server cannot silently ignore the precondition.

No stored format, hash, merge algorithm or metadata schema changes. Existing reviews remain unchanged. HTTP and gRPC use the same request schema and engine operation. There is no irreversible data migration.
