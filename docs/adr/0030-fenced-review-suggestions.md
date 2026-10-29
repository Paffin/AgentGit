# ADR 0030: explicit, revision-bound review suggestions

Status: accepted.

## Decision

Represent a suggested replacement as one final fenced `suggestion` block in a line thread's
existing body. Text before the block explains the proposal. The delimiter contains at least
three backticks and is longer than every run of backticks in the explanation and replacement.
An empty replacement requests deletion of the anchored logical line. Other clients retain
readable text; no database migration, protobuf field or canonical VCS object changes.

The UI supports one anchor line on the new side, replaced by zero, one or multiple
lines. A range uses the explicit fence header `suggestion:-0+N`: it replaces the anchor
and the following `N` original lines (at most 100 lines total). A plain `suggestion`
header still means exactly one line. The range must exist in the immutable anchored
revision; the author previews the original file before confirming. Older clients
retain range bodies as readable text and do not apply them as single-line suggestions. It does not interpret general comments, replies, arbitrary Markdown or multiple
suggestions as executable instructions. It never executes the suggested code.

Application is an explicit author action after preview. The client requires negotiated
`proposals.update_head`, author identity, write access and a current open proposal. It reads
the immutable anchored source, checks the exact line, and supports regular UTF-8 files up to
1 MiB. It preserves executable mode, BOM and untouched content, retaining the line-ending
style. Binary files, symlinks, old-side anchors and mismatching lines are refused.

A private workspace prepares one changeset. Updating the proposal uses the anchored head as
`expected_head`; a concurrent update is refused by the core transaction (ADR 0029). Workspace
cleanup retains the original session even after cancellation. A failed cleanup may leave a
private workspace visible to its owner; it does not move a branch. Ambiguous network outcomes
require inspecting the proposal, not blindly applying again.

The operation does not integrate a branch, forge an approval, preserve a validation for changed
content or automatically resolve the discussion. Normal new-head review and exact-tree check
rules still apply. The old suggestion remains attached to its old revision and readable.

Authors can also select suggestions from loaded discussions for one explicit batch preview.
All anchors must belong to the current proposed head; overlapping original ranges are refused.
Within each file, replacements run from the last original line upward so inserted or deleted
lines cannot shift remaining anchors. Up to 100 suggestions and 4 MiB of resulting content
produce one workspace changeset and one head-checked proposal update. Files are read once
and every exact source line is checked before publishing. Discussions remain open.

## Remaining scope

Server-native suggestion metadata and an
explicit native CLI apply command remain future work. Existing CLI/SDK thread body operations
can read and write the convention without schema changes.
