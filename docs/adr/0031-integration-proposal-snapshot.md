# ADR 0031: bind integration commit to the proposal snapshot

Status: accepted.

## Context

Integration computes its candidate outside the metadata write transaction. The original
implementation rechecked the stream inside that transaction, but checked the proposal head
and open state earlier. An author could update or close the proposal between candidate
computation and commit. The old candidate could then land and mark the changed proposal as
integrated. Head-bound approvals alone do not close that interval.

## Decision

Immediately before committing, compare both mutable inputs inside the same metadata write
transaction: the complete stream state and the proposal's head/open state. A mismatch returns
to the existing bounded integration loop. A changed or closed proposal aborts the original
operation on its next inspection; callers must inspect the current proposal and start a new
operation. It does not silently integrate the replacement head.

Apply the same comparison when recording `already_integrated`. Although that path moves no
stream head, it must not mark a newer or closed proposal as integrated. Its stored outcome
must describe the stream state checked in the transaction.

The checks, policy facts, head CAS, proposal state, intent transition and audit remain atomic.
No schema, object format or wire contract changes. Existing aborted-operation and idempotency
semantics are unchanged; this extends their protection to the final commit interval.
