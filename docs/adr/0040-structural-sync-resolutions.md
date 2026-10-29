# ADR 0040: resolve the complete structural merge candidate

Status: accepted.

## Context

A file/directory clash cannot be represented by a valid tree. The merge engine previously
returned the workspace tree as a best-effort fallback. Virtual sync then applied explicit
resolutions to that fallback, which could discard unrelated clean additions, edits and
deletions from the incoming branch. Directory sync could also save the fallback as a pending
merge and later commit an incomplete snapshot.

## Decision

Retain the pre-validation entry map only for structural conflicts. Explicit virtual-sync
resolutions apply to that complete candidate before final tree validation. Keeping a file
requires explicit deletion of conflicting descendants; choosing a directory removes the
colliding file. The server never silently chooses the discarded subtree. Existing object,
path and tree limits still apply. Invalid or incomplete choices do not move the workspace.
The `workspaces.structural_resolution` capability identifies this guarantee to clients.

The ordinary merge algorithm, its version and its public best-effort tree remain unchanged.
The additional map exists only for a structural conflict; clean merges do not acquire it.
No persistent schema or object format changes.

Local directory sync refuses a structural clash before changing files, base, head or pending
state. A caller can explicitly change and snapshot its layout before retrying, or use virtual
sync with a complete resolution. Snapshots of legacy structural pending merges are also
refused, including with `--force`; the caller must preserve its edited files and reset/retry
instead of silently committing an incomplete result. Ordinary text-conflict workflows remain.

## Validation

Repository tests cover both structural orientations and choices, preserve independent clean
add/edit/delete operations, check merge parents and workspace/stream heads, and verify that
invalid resolutions do not mutate the workspace. Merge tests retain exact blob/type/mode and
handle multiple clashes. Local tests check unchanged files/state and a successful explicit
layout retry; legacy pending tests cover ordinary and forced snapshots.
