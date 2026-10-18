# Performance of the engine

What the engine's hot paths cost by repository size, how the numbers were taken, and what they do not say. The
server under load (HTTP/JSON, gRPC, SSE, rate limits) is a different measurement: see
[Performance under load](../operations/performance.md). The decision these numbers feed — when flat trees stop being
good enough — is [ADR-0006](../adr/0006-flat-trees-measured.md).

There are no targets here. Numbers are reported with their conditions; nothing in AgentGit promises them.

## How it is measured

`ag-bench engine` (`crates/ag-bench`) calls the real engine in-process — `ag-integrate`, `ag-merge`, `ag-tree` — with
no stub stores:

```sh
cargo build --release -p ag-bench
target/release/ag-bench engine                         # 1k, 10k, 100k files; in-memory stores
target/release/ag-bench engine --backend disk --sizes 1000,10000   # SQLite + one file per object, with fsync
target/release/ag-bench engine --json out.json --markdown out.md
```

* A repository of *n* files has paths `pkgNNN/modNN/fileN.rs` and two-line contents (the same synthetic layout as
  the `scale` tests); one more file, a 200-line text, is the target of the history rows.
* Operations that change state (the integrations) are sampled one after another, each on top of the previous
  result, so the repository grows by a handful of changesets per row. Pure functions (merge, diff, decode…) are
  sampled repeatedly on the same inputs.
* Each cell is the **median** of the samples with the (min–max) range; a cell without a range is a single sample
  (the one-off setup steps). The default is 7 samples, fewer if a row exceeds a 60 s budget (never below 3).
* `--backend memory` is the engine's CPU cost: in-memory metadata and objects, no fsync, no network.
  `--backend disk` is what `ag init` creates: SQLite in WAL mode with `synchronous=FULL` and one fsynced file per
  object.
* Every row asserts what it measured (an integration is `Strategy::FastForward` or `Strategy::Merge` as named, a
  rename is found, a merge is clean). `cargo test -p ag-bench` runs the whole suite at 1 000 files on every CI run
  (`crates/ag-bench/tests/smoke.rs`), so the rows cannot silently stop measuring what they say.

Conditions of the numbers below: commit `ff68edb` plus the change that introduced `ag-bench` and the blame fix
described under [A bottleneck found](#a-bottleneck-found-blame-over-an-uncached-chain); Intel Xeon @ 2.80 GHz, 4
logical CPUs, 16 GiB, Linux 6.18, rustc 1.97, release build (`lto = "thin"`, `codegen-units = 1`). It is a shared
cloud VM on which other jobs ran (load average 2–13 during the runs): treat differences below roughly 30 % as noise,
and re-run on your own hardware before drawing conclusions. The scheduled `Performance` workflow
([`perf.yml`](../../.github/workflows/perf.yml)) repeats the full run on a GitHub-hosted runner and uploads the JSON.

## Results, in memory

Milliseconds, median (min–max).

| operation | 1000 files | 10000 files | 100000 files |
|---|---:|---:|---:|
| ingest: put every blob | 1.91 | 11.2 | 164 |
| commit: build and store the import changeset | 0.685 | 5.75 | 85.4 |
| publish: the whole import (verify every blob) | 9.92 | 79.8 | 973 |
| integrate: initial import into an empty stream | 5.26 | 48.3 | 386 |
| publish: a one-file changeset on a published base | 1.55 (0.876–1.66) | 7.12 (7.00–10.6) | 85.8 (82.3–90.1) |
| propose: open a proposal for a published head | 0.072 (0.044–0.087) | 0.066 (0.063–0.107) | 0.103 (0.071–0.127) |
| integrate: fast-forward (1-file proposal) | 4.52 (2.92–5.43) | 27.4 (26.4–32.3) | 345 (321–454) |
| integrate: three-way merge (1 file vs 1 file) | 4.61 (4.20–5.50) | 61.8 (47.6–97.8) | 1086 (904–1235) |
| integrate: three-way merge, rename vs edit of the same file | 5.71 (3.95–8.02) | 63.3 (37.7–78.7) | 1586 (450–2232) |
| merge_trees: 1 file each side | 0.031 (0.030–0.044) | 0.419 (0.352–0.648) | 16.1 (13.2–18.2) |
| merge_trees: 1% of files renamed (exact) vs 1 edit | 0.058 (0.040–0.090) | 0.646 (0.607–0.998) | 22.1 (18.2–22.8) |
| merge_trees: 100 renamed and edited files (similarity) vs 1 edit | 0.070 (0.054–0.136) | 1.09 (0.899–1.20) | 19.3 (18.0–19.7) |
| detect_renames: 1% of files renamed (exact) | 0.015 (0.014–0.028) | 0.180 (0.166–0.428) | 5.57 (5.40–10.4) |
| detect_renames: 100 renamed and edited files (similarity) | 0.030 (0.029–0.055) | 0.478 (0.450–0.531) | 4.32 (4.15–4.51) |
| tree diff: 1 changed file | 0.009 (0.008–0.011) | 0.121 (0.092–0.215) | 3.53 (3.44–3.66) |
| tree diff: 1% of files changed | 0.009 (0.009–0.010) | 0.106 (0.097–0.189) | 3.44 (2.69–4.36) |
| manifest: Tree::decode (strict, as for a cached tree) | 0.235 (0.230–0.264) | 2.71 (2.55–3.21) | 55.4 (36.4–61.8) |
| manifest: Tree::encode | 0.009 (0.009–0.015) | 0.148 (0.107–0.285) | 2.89 (2.56–3.29) |
| manifest: Tree::id (encode + SHA-256) | 0.271 (0.266–0.315) | 3.24 (2.86–4.54) | 51.6 (51.0–52.3) |
| manifest: Tree::clone | 0.020 (0.019–0.023) | 0.170 (0.142–0.443) | 7.11 (5.65–9.63) |
| tree_of: head of the stream (cache hit) | 0.583 (0.556–0.638) | 7.37 (7.09–9.41) | 116 (77.2–130) |
| blame: one file, 20 changesets of history | 2.12 (2.06–2.45) | 22.1 (13.4–24.6) | 168 (138–195) |
| log --path: one file, 20 changesets | 0.470 (0.438–0.680) | 0.459 (0.427–0.545) | 0.472 (0.426–0.765) |

What the 100 000-file column says:

* **Everything is linear in the number of files, with small constants**, as ADR-0006 predicted: a tree diff is 3.5 ms,
  a clean tree merge 16 ms, rename detection 4–6 ms (it works from the change lists, not from the whole trees).
  Renames do not make a merge noticeably more expensive than a plain one: 1 000 exact renames add about 6 ms to the
  16 ms of `merge_trees`.
* **The cost of an integration is the manifest**, not the merge: a fast-forward is about 345 ms, of which hashing the
  manifest (`Tree::id`, 52 ms — SHA-256 over 8 MB on this CPU, about 160 MB/s), decoding a cached tree (55 ms) and
  verifying it (the same hash again when the cached manifest is read back) account for most. A three-way merge needs
  one more tree and costs about three times a fast-forward here; the ADR measured a smaller gap (370 ms vs 179 ms) on
  another machine, and the existing `scale` test on this one gave 322 ms and 504 ms the same hour, so the spread
  between runs is wide. Integrations on one stream serialize on the compare-and-swap: at 100 000 files that is
  roughly one to three per second per stream.
* **Publishing the whole import** (973 ms for 100 000 blobs) verifies every blob once; a one-file change costs 86 ms
  at 100 000 files because publication also derives and stores the new tree.
* `log --path` does not depend on the repository size (it uses a path index).

The scenario "rename vs edit of the same file" ends with the edit applied at the renamed path (asserted), at the cost
of the rename detection on top of the three-way merge; its wide range at 100 000 files (450–2232 ms) is the noise of
the shared VM, not a property of the algorithm.

### On disk

`--backend disk`, 1 000 and 10 000 files (100 000 files was not run: the ingest alone fsyncs 100 000 object files):

| operation | 1000 files | 10000 files |
|---|---:|---:|
| ingest: put every blob | 1043 | 9037 |
| publish: the whole import (verify every blob) | 19.1 | 166 |
| publish: a one-file changeset on a published base | 3.31 (3.12–4.36) | 14.0 (11.1–26.9) |
| integrate: fast-forward (1-file proposal) | 6.57 (5.68–6.90) | 50.5 (34.8–96.0) |
| integrate: three-way merge (1 file vs 1 file) | 10.0 (9.30–11.5) | 63.4 (53.5–76.2) |
| integrate: three-way merge, rename vs edit of the same file | 12.4 (9.74–14.2) | 65.4 (47.5–81.9) |
| blame: one file, 20 changesets of history | 2.50 (2.34–2.63) | 15.5 (15.0–24.4) |

The disk adds fixed costs per write transaction and per object (about 1 ms per blob: every object is written to a
temporary file, fsynced and renamed; `synchronous=FULL` for the metadata), not per-file costs inside an integration:
at 10 000 files a fast-forward is 50 ms on disk against 27 ms in memory. The full table is in the `perf-full`
artifact of the scheduled workflow.

## A bottleneck found: blame over an uncached chain

The first run of this suite showed `blame` costing far more than the ADR's table suggested, because the ADR's blame
row had a history of three changesets and this one has twenty. For every changeset it visited, blame built the whole
tree of the changeset and of its parent (`Repository::tree_of`). A tree is cached only for keyframes (every 32nd
depth) and live heads (ADR-0003), so each of those calls replays the operation lists from the nearest cached ancestor
over a copy of the whole tree — and the two calls of the next changeset replay the same chain again. The cost was
quadratic in the length of the uncached chain times the repository size, to look up one path.

Fix (`crates/ag-integrate/src/blame.rs`, `Repository::entry_at`): blame needs one directory entry per changeset, so it
now looks the path up directly — the first changeset on the first-parent chain whose operations mention the path
decides, a cached tree decides when none does — and no tree is built. Results are unchanged (every blame test passes;
`blame_over_a_long_uncached_chain_with_a_delete_and_a_recreate` covers deletion and re-creation across more than two
keyframe intervals).

Blame of one 200-line file changed by 20 unintegrated changesets, memory backend, median of 5 (before) and 7 (after):

| files | before | after |
|---:|---:|---:|
| 1 000 | 40.0 ms | 2.12 ms |
| 10 000 | 423 ms | 22.1 ms |
| 100 000 | 10 606 ms | 168 ms |

What guards it: the benchmark row (`blame: one file, 20 changesets of history`) and the correctness tests. There is no
test that fails on the old code, because the old code was correct and slow; a timing assertion would be flaky on
shared CI runners.

## Not measured, therefore not claimed

* PostgreSQL metadata and S3 objects (network latency dominates there; `scripts/pg-test.sh` / `scripts/s3-test.sh`
  run the functional tests, not a benchmark).
* Deep directory trees, very large single files, a million files, concurrent integrations *inside* the engine (the
  server-level concurrency is in [Performance under load](../operations/performance.md)).
* Memory: tree sizes are estimated in ADR-0006 (150–200 bytes per file); this suite does not measure resident memory
  of the engine.
* Other CPUs. SHA-256 throughput in particular depends on hardware support (SHA extensions) and decides the share of
  hashing in the numbers above.

## Where the next gain would come from

An in-process cache of decoded trees would remove the decode and verification of a cached head
(about 105 ms of the 345 ms fast-forward at 100 000 files) — ADR-0006 already names it. It trades memory (15–20 MB per
cached tree at 100 000 files, per open repository) for latency, so it needs a budget in the configuration and a
decision about it; it is not done here.
