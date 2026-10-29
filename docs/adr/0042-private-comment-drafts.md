# ADR 0042: private account-backed proposal comment drafts

Status: accepted.

## Decision

General proposal comments have one private draft per proposal and full acting identity,
separate from review-decision drafts. Schema 11 adds `comment_drafts` without rewriting
review drafts. Its body is bounded to 16 KiB of UTF-8, matching a posted comment; the
inspected head supplies context but does not pin retained history. Agent runs remain
distinct owners. No API accepts an owner selector.

Get, save and clear require proposal-comment permission and stream access. They are
available after proposal closure, like posting a general comment. Saves and clears
compare the inspected version in the metadata transaction. Identical retries are
no-ops; cleared drafts retain versioned tombstones so stale devices cannot resurrect
notes. Draft text never enters shared audit records, events or public proposal data.
Private HTTP responses use `Cache-Control: no-store`; administrators and backups can
still access stored notes.

CLI, SDK and MCP expose explicit get/save/clear operations under the
`proposals.comment_drafts` capability. Web preserves tab-local drafts and explicitly
saves to the account. An empty form may restore saved notes; differing text requires a
choice. Saving never posts a comment. Posting still checks the inspected proposal head.
Cleanup uses the captured account and version, and cleanup failure cannot turn a
successful post into a failed post or cause duplicate submission.

Back up before opening an older repository with this development build. Downgrades
require restoring that backup. Thread replies and file-edit drafts remain outside this
slice.
