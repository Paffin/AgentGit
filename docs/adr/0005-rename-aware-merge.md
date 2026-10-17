# ADR-0005: Rename-aware tree merge

- Status: accepted
- Date: 2026-10-17
- Relates to: ADR-0001 §6 (deterministic merge), `docs/architecture/merge-semantics.md`

## Context

With version 1 of the merge a rename was a delete plus an add. When one agent moved a file and another edited
it — the normal shape of parallel refactoring, which is what agents do all day — the result was a
`DeleteModify` conflict that a human had to resolve by hand, although an unambiguous answer existed.

Merge semantics are part of the specification (CONTRIBUTING requires an ADR to change them): integration
decisions must be reproducible, and the merge algorithm version is recorded in every merge changeset.

## Decision

Tree merge detects renames between the base and each side and merges a moved-and-edited file as one unit at
its new path. Detection is a pure function of the trees and blob contents, defined in
`docs/architecture/merge-semantics.md#renames`:

- exact content first, then line-multiset similarity ≥ 60 %, greedy and tie-broken by path, each path used
  once; regular text files only for similarity;
- bounded work (≤ 200 candidates per side, a fixed comparison budget), falling back to exact-only — a
  property of the *inputs*, so determinism is preserved;
- FNV-1a line hashes (fixed, platform-independent) rather than a randomized hasher;
- new conflict kinds `RenameRename` and `RenameDelete`; a conflict at a renamed path carries the original
  paths of the three versions (`ConflictOrigin`) so workspace markers land in the right file.

`ag_merge::ALGORITHM_VERSION` becomes `ag-merge/2+ag-diff/1`. It is written into the message of every merge
changeset, so history records which semantics produced each merge.

## Consequences

- A rename on one side and an edit on the other integrates without a human, and a conflict inside such a file
  is reported (and marked) at the file's new path.
- The same three trees can now produce a *different* result than under `ag-merge/1` (a clean merge where
  there used to be a conflict, never the reverse for inputs without renames). Existing merge changesets are
  unaffected — they are stored results, not recomputed — but a proposal that conflicted yesterday may be
  mergeable today. Preparations and attestations are bound to tree ids, not to the algorithm, so a clean
  merge's tree is validated like any other.
- Similarity is a heuristic. A false positive pairs unrelated files whose contents are ≥ 60 % identical (a
  deleted template and a new file derived from it); the merge then merges them as one file, which can
  produce a surprising but still *conflict-checked* result — and validation and review, which are independent
  of mergeability, still apply. A false negative degrades to the old behaviour (delete/modify conflict).
- Not covered: directory renames (files added into a moved directory), copies, and rename detection in the
  displayed diff.
- Cost: detection runs per merge and reads blobs of deleted and added files only when both sets are
  non-empty; the budget caps worst-case work.
