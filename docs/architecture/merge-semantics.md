# Merge semantics

Merge is a **pure, deterministic function** of `(base, ours, theirs)` and an algorithm version
(`ag-merge/1+ag-diff/1`, recorded in every merge changeset message). The same inputs always give the same
result, so an integration decision can be reproduced and audited. Nothing here uses a model.

> "Mergeable" means only that an unambiguous result tree exists. It says nothing about whether the result
> works. Validation and approval are separate, independent steps ([Concepts](../concepts.md)).

## Which three trees

For integration: `ours` = the current stream head, `theirs` = the proposal, `base` = their merge base
(`ag-dag::merge_bases`). Criss-cross histories with more than one best base, and unrelated histories, are
**refused** with an explanation rather than merged by guesswork. The result is committed as a changeset
whose parents are `[head, proposal]` and whose operations are relative to the head.

## Tree level (per path)

```text
ours == theirs            → take it           (both did the same thing, including both deleting)
theirs == base            → take ours
ours == base              → take theirs
otherwise (both changed it, differently):
```

| Situation | Result |
|---|---|
| both regular text files | line-level three-way merge (below); may conflict |
| both regular, any side binary (NUL in first 8 KiB) | `Binary` conflict |
| a file over 16 MiB on any side | `TooLarge` conflict |
| one side deleted, other modified | `DeleteModify` conflict |
| both added the path with different content | `BothAdded` conflict |
| symlink involved, or type changed | `TypeChange` conflict |
| merged result would put a file where a directory is needed | `FileDirectory` conflict |

The executable bit merges independently of content (if both changed it, ours wins; with two possible values
one side always equals the base, so this never conflicts).

## Line level

Both sides are diffed against the base with the in-repo Myers implementation (`ag-diff`, deterministic;
common prefix/suffix trimmed first; beyond an edit distance of 2048 a region is reported as one big hunk,
which is always *correct* but not minimal). Hunks from the two sides that **overlap or touch** (share a
boundary, including two insertions at the same position) form a *cluster*:

- a cluster with hunks from only one side applies that side;
- a cluster is clean only if **both sides produced identical text** for it;
- otherwise it is a conflict region.

Touching hunks conflict on purpose: the engine cannot know that adjacent edits are semantically
independent, and a needless conflict costs a human a minute whereas a silent bad merge costs a bug. One
unchanged line between two edits is enough to merge them.

Properties checked by tests (`ag-merge`): disjoint edits merge; identical changes merge; overlapping and
adjacent edits conflict; one-sided changes always win; conflict *status* is symmetric in `ours`/`theirs`;
the result is deterministic.

## Not implemented

- **Rename detection.** A rename is a delete plus an add, so a rename on one side and an edit on the other
  is a `DeleteModify` conflict.
- **Semantic merge.** Language-aware merging and API-signature analysis are advisory ideas for the agent
  layer, not part of the correctness core (ADR-0001 §6).

## Conflict resolution flow

`ag preview <proposal>` reports conflicts without side effects. In a workspace, `ag sync` applies the clean
part of the merge and writes `<<<<<<< ours` / `=======` / `>>>>>>> theirs` markers into conflicting text
files. `ag snapshot` refuses while markers remain (unless `--force`) and records the resolution as a merge
changeset with both parents, which then integrates as a fast-forward.
