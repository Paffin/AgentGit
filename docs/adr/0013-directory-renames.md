# ADR-0013: Directory renames in the tree merge (`ag-merge/3`), and copy detection for display

- Status: accepted
- Date: 2026-10-20
- Relates to: [ADR-0005](0005-rename-aware-merge.md) (file renames, `ag-merge/2`), ADR-0001 §6 (deterministic merge),
  `docs/architecture/merge-semantics.md`

## Context

`ag-merge/2` follows a *file* rename: one side moves a file, the other edits it, the edit lands at the new path.
Moving a whole directory is the same refactoring at a larger scale, and it is the common one for agents (split a
package, rename a module). Under version 2 it leaves two kinds of damage when the other side was working in the old
directory at the same time:

- files the other side **added** beneath the old directory stay there, so the old directory is recreated next to the
  new one, and the result compiles against neither layout;
- files the other side **edited** whose moved copy was rewritten too much to be matched by content similarity end
  as `DeleteModify` conflicts although the move is plain from the directory.

Merge semantics are part of the specification: the algorithm version is recorded in every merge changeset, and a
recorded merge must stay verifiable.

## Decision

**1. Algorithm version.** `ag_merge::ALGORITHM_VERSION` becomes `ag-merge/3+ag-diff/1` and is what new merge
changesets (integration and workspace sync) record. `ag-merge/2+ag-diff/1` stays runnable: `ag_merge::Algorithm`
(`V2`, `V3`), `Algorithm::parse(tag)` and `merge_trees_with(algorithm, …)` reproduce a merge from the version it
recorded, and `merge_trees` is `merge_trees_with(Algorithm::CURRENT, …)`. The compatibility suite verifies the
recorded merges of the old repositories with the version they name. Nothing recomputes an existing merge on read;
version 2 results are stored changesets.

**2. A directory rename is derived from file renames, never from names alone.** For one side relative to the base, a
base directory `D` is renamed to `E` when

- `D` is *vacated* (every base entry beneath it is gone on that side and nothing was added beneath it),
- *more than half* of the base entries beneath `D` were detected as renamed (exact or similar content, as in
  ADR-0005) to the same relative path beneath `E`, and
- neither directory lies inside the other (`a → a/b` is a nesting, not a rename; it is not followed).

A vacated directory from which files left for several places, or for one place with half or fewer of the files, is
*ambiguous*; one that merely lost its files has no rename. Directories are judged at every depth; for a path the
other side added, the *deepest* vacated ancestor decides.

**3. What follows.**

- A file the other side **added** beneath a renamed `D` is placed at the same relative path beneath `E`. It follows
  only if that target is free: if either side already has something there, or two files head for the same
  target, it is a `directory_rename_collision` conflict, reported at the added file's path.
- If the vacated directory is *ambiguous*, an added file beneath it is a `directory_rename_ambiguous` conflict
  (candidates listed, at most 8). It is never a guess; the best-effort tree keeps the file where it was added.
- A file the other side **modified** that this side deleted from `D` while adding the same relative path beneath `E`
  is treated as that file's rename even if its content is no longer similar, so the edit merges at the new path (a
  file the other side *deleted* is not paired this way: the version-2 result stands).
- Paths that already take part in a file rename, and any nesting beyond the above, behave as in version 2.

**4. A typed, explainable note.** `TreeMerge` carries `notes: Vec<MergeNote>`; the one kind is `directory_rename`
(renaming side, old and new directory, how many files moved, the files that followed — capped at 100 listed — and
the edits that followed). A used directory rename is written to the merge changeset's message as a
`Directory-Rename: lib -> src (ours renamed it; 3 of 3 files; 1 added and 0 edited followed)` line, shown by `ag
preview` (`note:` lines and the moves) and `ag show`. Notes never make a merge unclean.

**5. Bounded work.** The analysis is linear in the changed paths plus one pass over the base tree, and only runs
when a file was renamed. At most 50 000 distinct (old, new) directory pairs per side (`MAX_DIRECTORY_PAIRS`);
beyond that the side's directory renames are not detected and version-2 behaviour applies, a property of the inputs
alone. No output depends on hash-map iteration order. The exact-rename pairing, which was quadratic in the number of
identical files, is now linear (it picks exactly the same pairs; the quadratic definition stays in the tests as the
reference).

**6. Copies are not part of the merge.** `ag blame --copies` and `ag show --copies` report a file created as a
*copy* of another file of the first parent: identical content, or else at least 50 % of the bytes in common
(`MIN_COPY_SIMILARITY`, same measure as renames) with one of at most 200 candidate files (those the same changeset
modified first, then the new file's directory, then path order). Lines a copy took over are blamed on whoever wrote
them in the source. This is derived display only: no merge, id, hash, wire format or stored object uses it, so it
needs no algorithm version. It is off by default (the server and MCP blame are unchanged).

## Consequences

- A directory move on one side and concurrent work in the old directory on the other integrates without a human, and
  says so in the preview and the history. Where the evidence is not strong enough the answer is a conflict.
- The same three trees can now give a *different* result than under `ag-merge/2` (clean where version 2 left files
  behind or conflicted; a conflict where version 2 merged silently into the old directory, in the ambiguous and
  collision cases). Merges already recorded are unaffected, and a proposal that merged yesterday may now carry a
  conflict that needs a look. Preparations and attestations are bound to tree ids, not to the algorithm, so a clean
  merge's tree is validated like any other.
- Heuristic limits: a false directory rename needs a vacated directory with a majority of content-matched files
  moving together, so it is rarer than a false file rename; it can still relocate a file the author meant to leave in
  a directory that was in fact moved. Validation and review, which are independent of mergeability, still apply.
- Not covered: nested moves where the new directory is inside the old one, a rename of the repository root, moves
  that split a directory with no majority (a conflict, by design), copies in merges.
- New conflict codes `directory_rename_ambiguous` and `directory_rename_collision` appear in the open `kind` string
  of the wire conflict (clients must already tolerate unknown kinds, `docs/protocol/agwp-v1.md`); no field, id or
  hash changed.
- Cost, measured (`cargo test --release -p agentgit-merge --test scale directory_rename_costs -- --ignored`, one
  renamed directory of 100 000 files, the other side editing one file and adding 20): see
  `docs/architecture/merge-semantics.md#cost`.
