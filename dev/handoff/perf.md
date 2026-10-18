# Handoff: perf (benchmarks, load test, one bottleneck fixed)

Pages: [architecture/performance.md](../../docs/architecture/performance.md) (engine, by repository size),
[operations/performance.md](../../docs/operations/performance.md) (the server under load). Tool: `crates/ag-bench`.
Workflow: `.github/workflows/perf.yml`.

## State

* **`ag-bench engine`**: publish (whole import, one-file change), propose, integrate (initial, fast-forward,
  three-way merge, merge with a rename against an edit), `merge_trees` (plain, 1 % exact renames, 100 renames with
  edits), `detect_renames`, tree diff, manifest decode/encode/id/clone, `tree_of` (cache hit), blame, `log --path`,
  at 1k/10k/100k files, memory and disk backends. Every row asserts what it measures. Numbers, machine and method are
  in the architecture page; ADR-0006 got a pointer.
* **`ag-bench load`**: starts a real `ag-server` per scenario (fresh data dir, default config except one setting) and
  drives it with real HTTP/JSON (ureq) and gRPC (tonic) clients: `baseline` (reads), `streams` (concurrent integrations
  on different streams), `contention` (same stream, server merges), `conflict` (same file), `sse` (100 subscribers,
  half SSE half gRPC, delivery lag and exactly-once check), `ratelimit` (token bucket vs configuration, retry hints,
  second credential unaffected), `shedding` (`max_concurrent_requests`). p50/p90/p99/max, server CPU and peak RSS from
  `/proc`, failures by code. Numbers and conditions are in the operations page.
* **Bottleneck fixed**: blame built the whole tree of every visited changeset and of its parent; over a chain of
  unintegrated changesets (trees cached only at keyframes) that is quadratic. `Repository::entry_at` looks the one path
  up instead. Blame of a 200-line file over 20 changesets: 40 ms -> 2.1 ms (1k files), 423 ms -> 22 ms (10k),
  10 606 ms -> 168 ms (100k). Test: `blame_over_a_long_uncached_chain_with_a_delete_and_a_recreate` (correctness over
  more than two keyframe intervals with a deletion and a re-creation). It does NOT fail on the old code (the old code was
  correct, only slow); a timing assertion would be flaky on shared runners, so the regression guard is the benchmark row.
* `cargo test -p ag-bench` (~95 s in debug on the shared VM): the engine benchmark at 1 000 files (all rows present,
  sane summaries) and the load generator against a real `ag-server` (builds it with cargo first): streams never conflict,
  contention is merged, same-file changes report `conflict`, rate limit admits no more than burst + rate x time, the
  other credential is unaffected, shedding leaves the server ready, every one of the subscribers gets every
  `stream.integrate` event once.
* CI: `perf.yml`. `smoke` (push/PR on the relevant paths) is `continue-on-error`, uploads `perf-smoke`; `full` (weekly
  and by hand) uploads `perf-full`. Neither compares against a threshold.

## Evidence

CI: see the run links in the final report of the session (filled in after the push; this file cannot know its own
commit's runs). Local: `cargo test -p ag-bench` x4 green; `cargo test -p ag-integrate`, `-p ag-server --test
history_api`, `-p ag-cli --test cli blame` green; clippy `-D warnings` on `ag-bench`/`ag-integrate`; fmt; `check_docs`,
`check_no_stubs`, `check_workflows` clean.

## Findings from the measurements (not fixed; for ROADMAP notes)

1. **Write throughput is bound by fsync, not CPU.** 16 clients on 16 different streams: 79.5 integrations/s with the
   server at 88 % of ONE core; the same scenario with the servers' data on tmpfs: 231/s (2.9x). SQLite
   `synchronous=FULL` plus writers to one repository's database taking turns. Group commit / batching writes of one
   integration into fewer transactions would be the lever; it touches the durability story, so not done. PostgreSQL
   metadata was not measured.
2. **Same-stream contention: ~11-14 % of integrations answer `aborted`** ("stream changed 8 times while integrating;
   retry") at 16 writers; harmless (retry) but the client must retry; the server gives up after 8 CAS attempts.
3. **`Retry-After` is `1` second for a `rate_limited` rejection whose `retry_after_ms` is 1-20 ms.** Correct by the
   header's format (whole seconds) but a client that honours the header waits 50x longer than needed.
4. **An in-process LRU of decoded trees** (ADR-0006 mentions it): ~105 ms of the ~345 ms fast-forward at 100k files is
   decoding + verifying (re-encoding and hashing) the cached head. SHA-256 over the 8 MB manifest is 52 ms here
   (~160 MB/s). Needs a memory budget setting; not done.
5. Disk ingest costs ~1 ms per blob (fsync per object file); 100k files on disk was not run (90+ s for the ingest alone).

## NOT verified

* Any deployment: one shared 4-vCPU VM (load average 2-13 from other jobs during the runs), loopback, generator on the
  same cores. Differences below ~30 % are noise. The first-hour ADR-0006 numbers and ours differ by up to 2x for the same
  operation on the same code (`scale` test on this VM gave 322/504 ms where the ADR says 179/370).
* PostgreSQL metadata, S3 objects, several replicas, TLS, the Web UI/MCP/workers under load; runs longer than 20 s.
* The `perf.yml` workflow on a GitHub-hosted runner (see the final report for what CI showed).

## Next steps

* Run `perf.yml` by hand on the GitHub runner and put its numbers beside the VM's.
* Measure `streams` with PostgreSQL metadata (`[repos]` storage) to see whether the fsync limit moves.
* Decide on the decoded-tree LRU and on batching writes (items 1 and 4).
