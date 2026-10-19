# ADR-0006: Flat trees — measured costs, optimizations, and when to go hierarchical

- Status: accepted (hierarchical Merkle trees deferred, with triggers)
- Date: 2026-10-17
- Relates to: ADR-0003 (sparse derived-tree cache), ROADMAP "Scale"

## Context

A tree is a flat, sorted manifest of every file: `TreeId = H(manifest)`, held in memory as one ordered map.
ADR-0003 noted that this is O(files) per tree and listed *hierarchical Merkle directories* (O(changed
directories)) as a "separate, larger change". That change alters the tree-id function — and with it the
conformance vectors and everything bound to a tree id (attestations, preparations, intents) — so it should be
justified by numbers, not by taste. Nobody had measured.

## Measurements

`cargo test --release -p agentgit-integrate --test scale -- --ignored --nocapture` (and the sibling `scale` tests in
`ag-tree` and `ag-merge`) build a synthetic repository (paths `pkgNNN/modNN/fileN.rs`) and time the engine
in-process: one thread, in-memory object store, SQLite metadata in memory. They are CPU costs of the engine,
not of a deployment (no network, no disk latency). Release build on a 4-vCPU Intel Xeon @ 2.10 GHz cloud VM (the tests themselves are single-threaded).

Integration of a one-file proposal, by repository size, **after** the optimizations below:

| files | initial import | fast-forward | three-way merge (1 file vs 1 file) | `tree_of` (cached) | blame of one file |
|---:|---:|---:|---:|---:|---:|
| 1 000 | 2.5 ms | 1.7 ms | 3.2 ms | 0.3 ms | 1.8 ms |
| 10 000 | 22 ms | 13 ms | 26 ms | 3.1 ms | 18 ms |
| 100 000 | 351 ms | 179 ms | 370 ms | 49 ms | 298 ms |

Where the time went at 100 000 files, **before → after**:

| step | before | after |
|---|---:|---:|
| decode a cached manifest (`Tree::decode`) | 115 ms | 31 ms |
| `merge_trees`, one file each side | 364 ms | 31 ms |
| fast-forward integration | 415 ms | 179 ms |
| three-way-merge integration | 1 249 ms | 370 ms |
| blame of one file | 569 ms | 298 ms |

Remaining costs scale linearly with the number of files: hashing the manifest (`Tree::id`, 7 ms at 100 000),
cloning a tree (10 ms), decoding it from the cache (≈ 30 ms), and holding it in memory (roughly 150–200 bytes
per file, so ~15–20 MB per tree at 100 000 files and a few trees at once during an integration).

## Decision

1. **Fix the quadratic-looking constants first** (done, no format change):
   - `Tree::decode` / `from_entries` validated the file/directory rule with an allocating lookup and a range
     scan per path. It is now one pass with a hash set of paths and no per-path allocation; decoding builds
     the map from the already-sorted entries in linear time.
   - `merge_trees` walked the union of all three trees with three lookups per path. It now computes the
     differences with two linear lock-step walks (`Tree::changes`) and examines only the paths their side
     changed (plus renamed ones), producing the result by editing `ours`. The previous algorithm is kept as
     `merge_full` — the fallback when the edit would break the file/directory rule (such cases must be
     *reported* as conflicts), and the reference in a property test (`merge_incremental == merge_full` on random
     trees with edits, renames and file/directory collisions; two deliberate mutations of the fast path are
     caught by it). Rename detection also works from the change lists instead of rescanning the trees.
2. **Defer hierarchical Merkle trees.** At 10 000 files every operation is a few tens of milliseconds; at
   100 000 an integration costs a few hundred. That is acceptable for the repositories AgentGit targets today
   and does not justify a change of the tree-id function.
3. **Go hierarchical when one of these is true** (and then write the format ADR, a v2 conformance vector set,
   and make the tree format a per-repository setting so v1 repositories keep working):
   - repositories above roughly 250 000 files are a requirement, or
   - integration latency at the target repository size, with the expected concurrency, misses its budget —
     remember integrations on one stream serialize on the compare-and-swap, so ≈ 0.4 s each is ≈ 2–3 per second
     per stream at 100 000 files, or
   - memory per open repository (a few tens of MB per tree per concurrent integration) becomes the limit.

## Consequences

- No change to any hash, format or protocol; all existing tests and conformance vectors are untouched.
- A 100 000-file monorepo is workable today, with integrations in the few-hundred-millisecond range; beyond a
  few hundred thousand files it is not, and the roadmap item stays open with measurable triggers.
- Not measured, and therefore not claimed: behaviour with many concurrent integrations, PostgreSQL or S3
  latency, very deep directory trees, very large individual files, or multi-million-file repositories. The
  `scale` tests are the starting point for those measurements.
- A cheap further win exists if needed: an in-process LRU of decoded trees (a tree of a changeset is
  immutable), which would remove most of the ≈ 3 × 45 ms decode per integration at 100 000 files.

## Update 2026-10-18: reproducible measurements

The suite in `crates/ag-bench` (`ag-bench engine`) repeats these measurements for publish, integrate, three-way merge
with rename detection, tree diff, blame and manifest decode at 1k/10k/100k files, and
[Performance of the engine](../architecture/performance.md) records the numbers with their conditions. It found one
bottleneck that this table did not show (blame over a long chain of changesets without cached trees, 10.6 s at 100 000
files, now 168 ms); the decision above is unchanged.
