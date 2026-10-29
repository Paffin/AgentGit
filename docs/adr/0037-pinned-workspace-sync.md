# ADR 0037: revision-bound workspace conflict resolution

Status: accepted.

## Decision

A client resolving a virtual-workspace conflict must know both immutable revisions it
is choosing between. `SyncWorkspaceResponse.stream_head` names the exact target branch
revision used by that attempt, including an empty string for an empty branch. It is not a
later read of the branch. The response head likewise names the result of this attempt.

A resolving client sends `expected_stream_head` on the next sync. The engine checks that
head before preparing a merge and inside the metadata transaction that moves the
workspace. A branch move rejects the operation with the existing failed-precondition
response. The existing workspace head/base check remains in force. A rejected operation
does not publish a different workspace or proposal head; prepared immutable objects may
be reclaimed by normal GC.

The API pins each individual attempt to the branch snapshot it read, including empty
branches. Clients that need continuity between preview and save negotiate
`workspaces.sync_precondition`; old servers may ignore unknown request fields and do
not supply the target revision. Clients must not silently fall back when applying an
inspected resolution.

For binary, oversized or type-changing files, the Web UI selects an existing version by
its blob identifier, type and executable bit. It does not decode and re-encode those
bytes. Definite missing paths may resolve to deletion; failed or unauthorized reads
remain errors. Text edits retain their original encoding markers, line ending style and
mode. Structural file/directory and rename conflicts remain explicit CLI work.

The final proposal update still compares its inspected head. Session cancellation keeps
cleanup tied to the initiating credential. None of this changes policy, approval,
canonical history or stored metadata formats; no database migration is required.
