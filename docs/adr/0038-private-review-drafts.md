# ADR 0038: private account-backed review drafts

Status: accepted.

## Decision

Add one private review-decision draft per proposal and full acting identity. It contains
the inspected head, intended decision and at most 64 KiB of UTF-8 notes. Saving a draft
never submits a review, approves code, updates the proposal, or broadcasts draft contents.
Agent runs remain distinct owners; a human account can resume its own draft on another
device. The API never accepts an owner selector and applies review capability and proposal
stream authorization before accessing a draft.

Save and clear compare the version read by that device inside their metadata transaction.
Identical desired content is a no-op retry. Clearing erases the content but retains a
versioned tombstone so an older device cannot resurrect it. A later draft cannot be cleared
by cleanup after a review submitted on another device. Existing drafts can be read or
cleared after proposal closure; new saves require an open proposal.

The browser retains its local tab draft. On opening review, an account draft may restore an
empty local form. A differing local draft requires an explicit choice; it is not silently
overwritten. Saving to the account is explicit. Submission remains a separate head-checked
action, and failure to clear a saved draft does not turn successful submission into failure.
Old servers retain the local-only behavior, negotiated through `proposals.review_drafts`.

Schema 10 adds `review_drafts`. Back up before opening schema-9 repositories; downgrade
restores the backup. Draft contents are excluded from public proposal responses, shared
audit records and repository events; private HTTP responses use `Cache-Control: no-store`.
Server administrators and backups can still contain these unsubmitted notes.

A draft's head is context, not a history-retention pin. A stale draft must be reviewed
against the current proposal head before submission. File-edit and general-comment drafts
are separate future extensions, not implicit members of this first review-draft API.
