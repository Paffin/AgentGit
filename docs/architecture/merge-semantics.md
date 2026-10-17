# Merge semantics

Merge is a **pure, deterministic function** of `(base, ours, theirs)` and an algorithm version
(`ag-merge/2+ag-diff/1`, recorded in every merge changeset message). The same inputs always give the same
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
| both sides moved the same file to different paths | `RenameRename` conflict (reported at the old path) |
| one side moved a file, the other deleted it | `RenameDelete` conflict (reported at the new path) |

The executable bit merges independently of content (if both changed it, ours wins; with two possible values
one side always equals the base, so this never conflicts).

## Renames

Before the per-path rules, each side is compared with the base for **renames** (algorithm version
`ag-merge/2`; [ADR-0005](../adr/0005-rename-aware-merge.md)): a path that exists only in the base is paired
with a path that exists only in the side, if their contents are the same file.

- *Exact* renames (identical bytes) always win over similar ones. A *similar* rename needs ≥ 60 % of the
  bytes in common lines (`2 × common / (old + new)`, over the multiset of lines); binary files and files
  over 256 KiB match exactly or not at all. Candidate pairs are chosen greedily by (similarity ↓, same file
  name first, old path ↑, new path ↑), every path at most once, within a fixed work budget (more than 200
  deleted or added files, or too many line comparisons, means exact matches only). The result depends only
  on the trees and blob contents — never on iteration order, clocks or randomness.
- Only regular files take part; directory renames show up as one rename per file, and a *new* file added
  into a directory the other side moved stays where it was added.
- When one side moved a file and the other **modified** it, the two are merged as one file at the **new
  path** (`base[old]`, `ours[new]`, `theirs[old]` — or the mirror image), with the ordinary line-level rules.
  A content conflict there is reported at the new path, and the conflict carries the three original paths so
  `ag sync` can write the markers into the right file.
- Both sides moving the file to the *same* path merge their contents against the base; to *different* paths
  is a `RenameRename` conflict; a move against a delete is a `RenameDelete` conflict. If the other side also
  created something at the destination, the ordinary rules (and `BothAdded`) apply.

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

- **Directory renames and copies.** Files added under a moved directory are not relocated; copies are not
  tracked (a copy is just an add).
- **Semantic merge.** Language-aware merging and API-signature analysis are advisory ideas for the agent
  layer, not part of the correctness core (ADR-0001 §6).

## Conflict resolution flow

`ag preview <proposal>` reports conflicts without side effects. In a workspace, `ag sync` applies the clean
part of the merge and writes `<<<<<<< ours` / `=======` / `>>>>>>> theirs` markers into conflicting text
files. `ag snapshot` refuses while markers remain (unless `--force`) and records the resolution as a merge
changeset with both parents, which then integrates as a fast-forward.
