# ADR-0003: Sparse derived-tree cache (keyframes)

- Status: accepted
- Date: 2026-10-17
- Relates to: ADR-0001 §4 ("tree index — derived"), §1 ("strong metadata, eventual derived indexes")

## Context

Trees (full repository states) are derived from the changeset DAG:
`tree(c) = apply(tree(first_parent(c)), ops(c))`. The first implementation cached the full tree manifest for
**every** published changeset. A synthetic benchmark (3 000 files, 1 500 commits) showed ≈ 170 KB per cached
tree: the metadata database reached 37 MB after caching only 128 of 1 500 trees, and a full cache would have
been ≈ 250 MB — quadratic in practice, for data that is by definition disposable.

## Decision

Cache trees **sparsely**:

- every changeset whose **first-parent depth** is a multiple of `KEYFRAME_INTERVAL` (32) — the depth is
  stored when the changeset is published, so the schedule is a property of the chain, not of how merges
  inflate generation numbers;
- the tree of every **live head** (stream heads, open proposal heads, workspace heads and bases), written
  eagerly after integration so the common next step is a cache hit;
- prune everything else after each dispatch of the derive worker (`prune_derived`).

Any other tree is rebuilt by walking the first-parent chain to the nearest cached ancestor and replaying
fewer than 32 operation lists (`resolve_tree` already stops at the first cache hit). Cached manifests are
verified on read; a bad entry is treated as a miss.

## Consequences

- Storage drops to ≈ one manifest per 32 changesets plus the live heads: the same benchmark's database went
  from 37 MB to 4 MB.
- Worst-case cost of reading an arbitrary historical tree is bounded by 31 delta applications.
- `ag reindex` rebuilds the same sparse set, so the cache remains provably disposable.
- Flat manifests are still O(files) per tree; hierarchical Merkle directories are a separate, larger change
  listed in the roadmap.
