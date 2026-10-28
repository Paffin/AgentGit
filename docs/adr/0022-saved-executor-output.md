# ADR 0022: opt-in bounded executor output

Status: implemented in development; not in beta.3.

## Decision

An executor may opt into `publish_output`. The default keeps stdout on the host,
as before. At session end the orchestrator saves at most the last 32 KiB of each
stream through `agent.output.save` before recording the final run outcome. It
removes the session token, known environment values and private signing-key hex
encodings. This is best-effort redaction of known values, not proof that output
contains no sensitive data. Operators must explicitly opt in.

The server accepts bounded, monotonically sequenced snapshots from `agent.operate`
callers while the run is active. Exact retries are idempotent. Readers with
`agent.read` can inspect saved output after completion. Text is stored separately
in metadata and does not count as validation, provenance, review or successful
execution. Audit records contain counts and sequences, never captured text.

Deleting output requires `agent.operate`. The text is removed while the run and
audit remain. A tombstone rejects future uploads for that run, including retries
that would otherwise resurrect the deleted text. This is logical deletion, not
erasure of existing backups or database journals.

The run page offers separate stdout/stderr tabs, byte counts, text download and a
confirmed deletion. Rendering is plain text. The setup wizard offers an unchecked
capture option and explains that repository readers can see saved output.

## Limits

Only the latest bounded snapshot is retained; initial delivery is at session end,
not live streaming. Counters are producer-reported and may describe a longer
stream than the saved text. Redaction does not detect arbitrary encodings or
unknown secrets. Older servers cannot save output; enabling the option does not
change an executor's authorization, task outcome or integration policy.
