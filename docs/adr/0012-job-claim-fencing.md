# ADR-0012: A fencing token for job claims

- Status: accepted
- Date: 2026-10-20
- Relates to: ADR-0001 (leases with fencing for exclusivity), ADR-0007 (several replicas),
  [AGWP v1](../protocol/agwp-v1.md), [workers](../guides/workers.md),
  [compatibility](../architecture/compatibility.md)

## Context

An execution-plane worker claims a validation job with a lease and keeps it with heartbeats. A claim whose lease
expires is given to the next claimer, and the check that identifies "the holder" is the worker's name
(`claimed_by` = `<principal>|<worker label>`). That separates two *different* workers. It does not separate two
incarnations of the *same* name: a worker process that was paused (a stopped container, a long garbage-collection
pause, a partition) while its lease ran out, whose job was claimed again by a restart of itself under the same label,
wakes up and still passes the check. It can heartbeat, fail or complete the job it no longer owns, and a completion
carries an attestation. The lease of exclusive file leases has had a fencing token from the start
(`Integrate.fencing_tokens`); the job queue had none. The audit listed it under "not fixed" (queue fencing tokens).

## Decision

**The token of a claim is the job's `attempts` at the time of the claim.** `job_claim` already increments `attempts` in
the statement that takes the job, so every claim of a job has a larger number than any earlier one, whoever claimed it.
It is returned in the `attempts` field of the `Job` that `ClaimJob` answers with; no new column, no schema change.

**Heartbeat, complete and fail carry it, optionally.** `HeartbeatJobRequest.fencing_token = 5`,
`CompleteJobRequest.fencing_token = 7`, `FailJobRequest.fencing_token = 6`, all `uint64`. When it is non-zero the
request is applied only if it equals the job's current `attempts` and the job is still claimed by the named worker;
otherwise it is refused like any request from a worker that lost its claim (`failed_precondition` for completion,
`not_found` for heartbeat and fail, which is what a lost claim already produced). The check is inside the same
conditional `UPDATE` that applies the change (and, for completion, also before the attestation is stored, so a stale
claimant cannot even leave an attestation behind). A value that cannot be a claim number (above `u32::MAX`) is
`invalid_argument`, not read as "none".

**Compatibility: a new optional field, absent = old behaviour.** 0 means "not given" and leaves the check as it was
(the worker's name). Protobuf and JSON clients that do not know the field keep working unchanged; the field is
additive in the OpenAPI document and the generated SDKs. No data migration, no new schema version, no change to
stored data, hashes or the audit format. `ag-worker` sends the token on every call; other workers should.

## Consequences

* ✅ A paused worker cannot act on a claim that was handed out again, even under the same label.
* ✅ No stored state changes; replicas and builds of different ages interoperate (an old server ignores the field, an
  old worker omits it).
* ⚠️ Fencing is opt-in per request. An old worker, or one that never sends the token, is as exposed as before. A
  server option now requires it after workers have been upgraded: `[jobs] require_fencing_token = true`.
  Set it on every replica; the default remains false for compatibility. Missing/zero tokens return
  `failed_precondition` before any job or attestation mutation.
* ⚠️ The token is the claim count, so it also moves when a job is claimed again after a retry (`fail` with a delay):
  a worker must use the number of *its* claim, which is what `ag-worker` does by taking it from the claimed job.
* Tests: `a_claim_number_fences_a_stale_claimant_of_the_same_name` (storage; SQLite and PostgreSQL),
  `a_stale_claim_of_the_same_worker_is_fenced_off` (HTTP, on a real server: the same worker name claims twice; the old
  number is refused on heartbeat, fail and complete and changes nothing; the current number and no number are accepted).
