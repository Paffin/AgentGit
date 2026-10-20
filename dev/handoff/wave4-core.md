# Handoff: wave 4, core and server follow-ups

Four items of ROADMAP "Finish server hardening" / "Performance", in order, each landed on `main` as its own slice.
Numbers and conditions are in [architecture/performance.md](../../docs/architecture/performance.md); decisions in
[ADR-0011](../../docs/adr/0011-proposal-paging-indexes-and-background-leases.md) and
[ADR-0012](../../docs/adr/0012-job-claim-fencing.md).

## What landed

| # | Item | Commit | Tests that fail without it |
|---|---|---|---|
| 1 | `ag agent reap --remote [NAME] [--timeout D] [--dry-run]` (calls `AgentService.ReapRuns`; `agent.operate`; audited under the caller; the local form gained `--dry-run`). The periodic reaper takes a `run-reaper` lease in the control database before each pass, so one replica of several reaps | `a64f608` | `an_operator_reaps_the_stale_runs_of_a_server_from_the_command_line` (real in-process server, real `ag` binary: a writer is refused, below-minimum refused, dry run, reap, audit entry `run.abandon` by the operator, idempotent), `only_the_replica_holding_the_reaper_lease_reaps` (server), `a_lease_has_one_holder_until_it_expires_or_is_released`, `replicas_racing_for_an_expired_lease_get_exactly_one_winner` (control database, SQLite and PostgreSQL, 4 threads x 5 rounds) |
| 2 | Metadata schema 3: three indexes for the proposal paging query; idempotency records without a time are dated once by the first sweep; new compatibility fixture | `2c26f2d` | `every_proposal_page_is_read_in_index_order_without_sorting` (plan of the exact statement, SQLite and PostgreSQL), `companions_are_written_only_for_keys_that_lack_one`, `records_written_before_they_carried_a_time_are_dated_once_and_then_expire`, `every_fixture_is_at_the_current_metadata_schema_with_the_paging_indexes` |
| 3a | An integration costs 3 durable commits (was 7 fast-forward / 8 merge): record written `ready`, tokens only if changed, one derive commit | `f87756d` | `an_integration_is_a_few_durable_commits` (`Meta::committed_writes()`), `crash_at_every_step_recovers_to_exactly_one_integration` unchanged and green, `an_operation_left_prepared_by_an_earlier_build_is_still_recovered` |
| 3b | Decoded-tree cache (`repos.tree_cache_bytes`, default 256 MiB) and bounded same-stream retry (`repos.integrate_attempts` 24, `repos.integrate_backoff_ms` 32) | `c82dd26` (+ a follow-up that moved the size estimate out of `ag-tree`, see below) | cache: `cached_trees_are_the_trees_by_definition_at_every_point_of_a_history`, `a_budget_too_small_for_the_history_evicts_and_stays_correct`, `garbage_collection_empties_the_repositorys_part_of_the_cache`, unit tests of `DecodedTrees`, `trees_are_served_from_memory_and_are_the_same_trees_without_the_cache`, `a_small_budget_evicts_and_the_answers_stay_right`; retry: `an_integration_that_loses_the_race_many_times_still_lands` (12 forced losses at a gate; the old limit of 8 fails it), `the_retry_is_bounded_and_the_caller_is_told_to_retry`, `the_wait_between_attempts_is_spread_per_operation_and_bounded` |
| 3c | `test(mcp)`: `closing_stdin_ends_the_process_cleanly` waited for nothing and raced the stderr drain thread (it failed once in the PostgreSQL + S3 job) | `47271e4` | (the flake; not reproducible locally) |
| 4 | Job-claim fencing token (ADR-0012): optional `fencing_token` on heartbeat/complete/fail, no schema change; `ag-worker` sends it; OpenAPI golden, Python and TypeScript SDKs and the Web UI schema regenerated | `67b0e1e` | `a_claim_number_fences_a_stale_claimant_of_the_same_name` (SQLite and PostgreSQL), `a_stale_claim_of_the_same_worker_is_fenced_off` (HTTP, real server) |

## Numbers (same shared 4-vCPU VM; differences under ~30 % are noise, see the page)

* Commits per integration 7 / 8 -> 3 / 3. `ag-bench load streams` (16 clients, HTTP, 15 s, 3 alternating rounds):
  87.6 -> 100.0 integrations/s (+14 %). Engine on disk: inside the noise except the three-way merge at 1 000 files
  (about -25 %).
* Decoded-tree cache, 100 000 files (memory backend): fast-forward 332 / 330 -> 183 / 178 ms; three-way merge 1 054 /
  880 -> 214 / 191 ms; publish of one file 86 -> 32-38 ms. Peak RSS of that run: 168 MiB without, 714 MiB with a
  512 MiB budget. The size estimate was checked against ten real clones of a 100 000-file tree (ratio 0.94).
* Same-stream `aborted` at 16 writers: 14-16 % -> 3-4 % (two alternating runs); the tail grew (p99 0.5-0.6 s -> 0.8-1.5 s,
  max 6 s) because the losers now retry inside the call.

## Behaviour changes worth knowing

* **Metadata schema 3** is not readable by builds that only know schema 2 (they refuse the repository). Start the new
  build first in a rolling upgrade (ADR-0007). The control schema is unchanged (3): the reaper lease is a `kv` row.
* New defaults: `repos.tree_cache_bytes = 268435456`, `repos.integrate_attempts = 24`, `repos.integrate_backoff_ms = 32`.
  `0` / `1` switch the cache off / the retry down to one attempt. New metrics `agwp_tree_cache_*`.
* `ag agent reap` without `--timeout` prints `Abandoned N run(s)` as before; `--json` of the local form is still a
  list of run ids. `ag agent reap --remote` prints the server's answer (runs, released leases, revoked credentials).
* `Tx::job_heartbeat/complete/fail` take a new `claim: Option<u32>` argument (storage API; the only callers are the
  server and tests). `Repository::with_tree_cache`, `with_contention_policy`, `Meta::committed_writes` are new.
* The size estimate of the cache lives in `ag-storage` (`approx_tree_bytes`). A first version added a method to
  `ag-tree`; the Quality workflow's per-crate coverage then reported `ag-tree` at 51.5 % (floor 90 %: 437 counted
  lines instead of 222, with all the new ones uncovered, although nothing there was untested) on `c82dd26`. Putting
  `ag-tree` back to the exact source it had made the question moot; the follow-up commit says so. If that coverage
  artefact comes back after a change to `ag-tree`, look at how `cargo llvm-cov` merges the instrumented copies.

## Not done / limits

* **The reaper lease is time-based** (replica clocks within a fraction of `2 x reap_interval_secs`); two replicas
  briefly reaping together is harmless (a reap is idempotent). Not tested with real replicas on PostgreSQL + S3
  (`multi_replica.rs` is the place); the contention is tested on the control database with threads, the gating on one
  server.
* Schema 3: three more index entries per proposal write (not measured as a load); the plan test forbids sequential
  scans on PostgreSQL to learn which index the statement could use, it does not show the planner's choice on a large
  table. A rolling upgrade that keeps an old replica writing idempotency records leaves undated ones behind the
  one-time backfill.
* Batching: PostgreSQL commits and S3 were not measured; the disk numbers are inside the VM's noise because its fsync
  is cheap. Not batched: the object write of a merge changeset (must be durable before the metadata that names it).
* The cache clones a tree on every hit (7 ms at 100 000 files); `Arc<Tree>` would remove it. Disk backend and PostgreSQL
  not measured with the cache. Garbage collection empties a repository's entries, but `StoreHistory` keeps its own
  small cache of changesets (pre-existing), so `tree_of` of a just-removed changeset can still answer from it.
* The retry makes the tail longer; a per-stream in-process queue would make same-stream writers take turns instead.
* Fencing is opt-in per request (an old worker is as exposed as before); no server option yet to require it.
* The Go orchestrator and the MCP server are not affected (they use run heartbeats, not job claims).
* CI: `a64f608`, `2c26f2d` and `c82dd26` had their CI/Windows runs cancelled by later pushes (the repository cancels
  superseded runs); `f87756d` had a red PostgreSQL + S3 job from the `ag-mcp` flake above (fixed in `47271e4`);
  `c82dd26` had a red Quality job (the coverage floor above). See the final report for the runs of the head commit.

## Evidence

Local: `cargo fmt --all --check`, `cargo clippy -p <touched crates> --all-targets -- -D warnings`, `cargo check
--workspace --all-targets`, `cargo test -p agentgit-storage -p agentgit-integrate -p agentgit-server -p agentgit-cli
-p agentgit-worker`, `scripts/pg-test.sh cargo test -p agentgit-storage -p agentgit-integrate` and the server tests
named above, `check_docs.py`, `gen_cli_docs.py`, `sdk/python/scripts/generate.py --check`, the TypeScript generator and
`openapi-typescript` (7.13.0) run for the SDK and the Web UI schema. The workspace-wide test run was not repeated at the
end (disk).
