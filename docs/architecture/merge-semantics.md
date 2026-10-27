# Merge semantics

Merge is a **pure, deterministic function** of `(base, ours, theirs)` and an algorithm version
(`ag-merge/3+ag-diff/1`, recorded in every merge changeset message; merges recorded as `ag-merge/2+ag-diff/1` are
reproduced by that version, see [Algorithm versions](#algorithm-versions)). The same inputs always give the same
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
| a file added beneath a directory the other side emptied ambiguously | `DirectoryRenameAmbiguous` conflict (reported at the added file) |
| a file added beneath a renamed directory whose target path is taken | `DirectoryRenameCollision` conflict (reported at the added file) |

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
- Only regular files take part. Directory renames are built on top of these file renames
  ([below](#directory-renames)); under `ag-merge/2` they showed up as one rename per file, and a *new* file added
  into a directory the other side moved stayed where it was added.
- When one side moved a file and the other **modified** it, the two are merged as one file at the **new
  path** (`base[old]`, `ours[new]`, `theirs[old]` — or the mirror image), with the ordinary line-level rules.
  A content conflict there is reported at the new path, and the conflict carries the three original paths so
  `ag sync` can write the markers into the right file.
- Both sides moving the file to the *same* path merge their contents against the base; to *different* paths
  is a `RenameRename` conflict; a move against a delete is a `RenameDelete` conflict. If the other side also
  created something at the destination, the ordinary rules (and `BothAdded`) apply.

## Directory renames

Since `ag-merge/3` ([ADR-0013](../adr/0013-directory-renames.md)). A directory is not renamed because a path looks
similar; it is renamed because its files were. For one side relative to the base, a base directory `D` is renamed
to `E` when all of these hold:

1. `D` is **vacated**: every base entry beneath it is gone on that side, and that side added nothing beneath it.
2. **More than half** of the base entries beneath `D` were detected as file renames (exact or similar, above) to
   the *same relative path* beneath `E`.
3. Neither directory lies inside the other (`a` to `a/b` is a nesting; it is not followed).

Then, for the changes of the *other* side:

| The other side | Result |
|---|---|
| **added** a file beneath `D` | the file moves to the same relative path beneath `E` (a `directory_rename` note says so) |
| **modified** a file beneath `D` that this side deleted while adding the same relative path beneath `E` | merged as that file's rename at `E`, even when the move rewrote the file beyond the similarity threshold |
| deleted or left alone a file beneath `D` | as before (the ordinary and file-rename rules) |
| added a file whose target beneath `E` is taken by either side, or two additions head for one target | `directory_rename_collision` conflict at the added file |

A vacated directory from which files went to **several places**, or to one place with half or fewer of the files, is
**ambiguous**: a file the other side added beneath it is a `directory_rename_ambiguous` conflict that names the
candidate directories. Nothing is guessed. A vacated directory that only lost its files (no file renamed anywhere)
is a deletion, and an addition beneath it stays where it was added. For nested directories the **deepest vacated
ancestor** of the added path decides, so `lib/` moved to `src/` with `lib/x/` moved to `other/x/` sends `lib/x/new`
to `other/x/new` and `lib/new` to `src/new`. Added files that are themselves the target of a file rename keep the
file-rename rules.

If both sides move the same directory to different places, the files conflict one by one as `RenameRename`, as in
version 2.

**Explainability.** Every used directory rename is a `MergeNote` of kind `directory_rename` (renaming side, old and
new directory, files moved of files total, the relocated paths — at most 100 listed — and the count of edits that
followed). `ag preview` prints it as `note:` lines, the API field `notes` of `MergePreview` carries it, and the merge
changeset's message records it:
`Directory-Rename: lib -> src (ours renamed it; 3 of 3 files; 1 added and 0 edited followed)`. Notes never make a merge
unclean.

**Bounds.** The analysis is linear in the changed paths plus one pass over the base tree, and runs only when a file
was renamed. More than 50 000 distinct (old directory, new directory) pairs on a side disable that side's directory
renames (file renames still apply), a property of the inputs. The pairing of exact renames is linear in the number
of files, including thousands of identical ones.

## Algorithm versions

| Tag | Adds | Reproduced by |
|---|---|---|
| `ag-merge/2+ag-diff/1` | file renames ([ADR-0005](../adr/0005-rename-aware-merge.md)) | `Algorithm::V2` |
| `ag-merge/3+ag-diff/1` | directory renames ([ADR-0013](../adr/0013-directory-renames.md)) | `Algorithm::V3` (current) |

`ag_merge::Algorithm::parse(tag)` maps the `Merge-Algorithm:` line of a merge changeset back to the version that
produced it, and `merge_trees_with` runs it; the compatibility suite checks the merges recorded by old
repositories that way. Without directory renames in the inputs the two versions return the same tree and
conflicts (property-tested). A new version is needed for any change of what a merge returns; display-only
analysis (copies, below) needs none.

## Cost

Measured with `cargo test --release -p agentgit-merge --test scale directory_rename_costs -- --ignored --nocapture`
on the build container (4 CPUs, in-memory blobs; one directory holding all the files is renamed on one side, the
other side edits one file and adds 20; wall time of one merge):

| Files | `ag-merge/2` | `ag-merge/3` | Same, all file contents identical (`ag-merge/2` / `ag-merge/3`) |
|---|---|---|---|
| 10 000 | 55 ms | 66 ms | 32 ms / 45 ms |
| 100 000 | 1.13 s | 1.30 s | 0.84 s / 1.15 s |

Most of that is detecting 100 000 file renames and rewriting 200 000 paths, which version 2 does too; the directory
analysis adds about 15 %. Without any renamed file nothing is added.

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

## Copies

A copy is a file created as a duplicate of another file that stays where it is. **No merge uses copies** (a copy
is just an add there). They are display and provenance only: `ag show --copies` marks them `C old -> new (NN% similar)`
and `ag blame --copies` blames the lines a copy took over on whoever wrote them in the source. A file created in a
changeset is a copy of a file of its first parent when the content is **identical**, or else when at least **50 %** of
the bytes are in common lines (`2 × common / (old + new)` over the multiset of lines, the measure of renames) with one
of at most **200** candidates (files the same changeset modified first, then the new file's directory, then path
order; binary files and files over 256 KiB match exactly or not at all). The best match wins, ties go to the same
file name and then the lowest path. Files that are the target of a rename are not copies.

## Not implemented

- **Copies in merges.** A file copied on one side and the original edited on the other is not related by the
  merge; the copy carries the content it was made with.
- **Directory moves into themselves** (`a` to `a/b`), moves that split a directory with no majority (a conflict
  when the other side adds beneath it), and chains (a directory renamed twice in one side's history is its net
  effect).
- **Semantic merge.** Language-aware merging and API-signature analysis are advisory ideas for the agent
  layer, not part of the correctness core (ADR-0001 §6).

## Conflict resolution flow

`ag preview <proposal>` reports conflicts without side effects. In a workspace, `ag sync` applies the clean
part of the merge and writes `<<<<<<< ours` / `=======` / `>>>>>>> theirs` markers into conflicting text
files. `ag snapshot` refuses while markers remain (unless `--force`) and records the resolution as a merge
changeset with both parents, which then integrates as a fast-forward.
