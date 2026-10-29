# ADR 0041: bounded structural conflict previews

Status: accepted.

## Context

An explicit file/directory choice must account for every affected candidate path. The union
of the two source trees is insufficient: directory-rename propagation can create a candidate
path that appears in neither source. Guessing in the browser can retain unwanted files or
silently imply deletion of a path the user never inspected.

## Decision

A fresh virtual-sync file/directory conflict may carry `resolution_scope` with candidate
`paths` and `complete`. The paths are at or beneath the conflict root in deterministic order.
A complete scope includes that root and every candidate descendant. Limits are 100 paths per
scope and 1,000 across one response; any omitted path makes that scope incomplete. Incomplete
scopes can be empty. Ordinary conflicts, legacy outcomes and proposal preparation do not
invent scope metadata. Clients negotiate `workspaces.structural_preview`.

The server scans retained candidate paths once and matches actual component ancestors,
without copying unrelated repository entries or reproducing rename detection in clients.
This metadata is a preview, not authorization or a server-selected resolution. Existing
workspace and branch revision checks still bind the eventual operation.

The browser groups nested conflicts under minimal file/directory roots, reads each source
subtree at its pinned revision, and offers an explicit choice of the complete source side.
It preserves original blobs, types and modes. The resulting batch retains chosen paths and
explicitly deletes other affected paths, including synthetic candidate destinations. A
missing, malformed, truncated or over-budget preview cannot enable this action. Other
structural conflict kinds remain separate work.

No metadata migration, object-format change or merge-algorithm version change is needed.

## Validation

Core tests cover synthetic relocated paths, sibling prefix boundaries, exact limits and
aggregate exhaustion. HTTP/gRPC tests bind scopes to the inspected stream and preserve the
absence of scope on ordinary/legacy data. Client tests preserve incomplete previews rather
than synthesizing completeness. Browser scenarios cover both file/directory orientations,
revision races, metadata preservation and independent clean changes.
