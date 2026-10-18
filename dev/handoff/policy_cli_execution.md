# Handoff: policy / agent / protocol, CLI / client / git bridge, execution plane (work package `policy_cli_execution`)

Every CONFIRMED item of the three audit areas (`dev/audit/CONFIRMED.md`: `policy-agent-protocol`, `cli-client-git`,
`execution-plane`) is fixed with a test that fails without the fix, the CONTESTED ones are triaged below, and the
execution plane is now proved against **real Docker, real Podman (rootful and rootless) and a real delegated
cgroup v2** in CI instead of a fake runtime and a fake directory. Nothing here changes a format, a hash or the
protocol's wire shape incompatibly, so there is no ADR (the protocol additions are additive: `Conflict.message`;
the `Blocker`/`Conflict` code vocabularies were already what the code produced and are now fixed strings with tests).

## What landed on `main`

| Commit | Content |
|---|---|
| `ebd5631` | `ag-sandbox`: the OCI sandbox reads the runtime's record back (`oci_record.rs`), cancellation, orphan reaping, cgroup base; `tests/oci_real.rs`, `tests/cgroup_real.rs`; the workflow `oci-runtimes.yml` |
| `3207c3e` | `ag-worker`: start-up checks of the daemon and the image, stop on demand, `--oci-user`, the execution limits as flags, exclusive job directories, `CheckoutError::Unsafe`; `tests/oci_e2e.rs` |
| `1f4bcfe` | protocol and agents: stable conflict and blocker codes (`Conflict.message` is additive), idempotent `SetPolicy`, prediction by divergence, honest proto comments, regenerated OpenAPI/SDKs/Web schema |
| `40d14ad`, `2bbfeab` | what Podman records for a memory kill: a diagnostic step, then `RuntimeInfo::engine`, the note and the per-engine assertion |
| `3d95e01` | `ag-client` and `ag`: paging, `AG_TIMEOUT`, batches, NDJSON, caps, errors that are not swallowed; `ag-client/tests/client.rs`, `ag-cli/tests/remote_scale.rs` |
| `c2e89a5` | `ag-git`: hermetic git, NUL framing, `lossy_commits`, cat-file's own message, branch names by git; `ag-git/tests/environment.rs`, `ag-cli/tests/git_env.rs` |
| `f8cc4da` | `ag-sandbox`: classification, digest documentation, the isolation tests that asserted nothing; `sandbox.md`, `workers.md`, `security.md`, `adr-traceability.md` |
| `31924b2` | conflicts shown in words (CLI, Web UI), `ag policy set` of the document in force says "unchanged", the tutorial and policy docs |
| `6c83241` | regenerated CLI reference, JSON help, golden |
| `970764b`, `833294a` | the runtimes workflow runs on `main`; its actions are pinned by SHA (`check_workflows.py --strict` is a gate now) |
| `903499d` | the `git_parse` fuzz seeds and dictionary in the new log format |

## The execution-plane proof

The workflow `.github/workflows/oci-runtimes.yml` (**OCI runtimes and cgroup v2**, path-filtered to `ag-sandbox`, `ag-worker`,
`ag-client`, itself and `Cargo.lock`) has four jobs on `ubuntu-latest` (Linux 6.17 azure, cgroup v2, Docker 28.0.4, Podman 4.9.3):

| Job | What runs | Result |
|---|---|---|
| `docker` | `tests/oci_real.rs` (14 tests) and `ag-worker/tests/oci_e2e.rs` (5) against the runner's Docker, a digest-pinned BusyBox pulled by the job, `AG_REQUIRE_OCI=1` | green |
| `podman (rootful)` | the same, through a `sudo` wrapper (`/usr/local/bin/podman-rootful`) | green |
| `podman (rootless)` | the same, as the runner user | green |
| `namespace sandbox on a delegated cgroup v2` | the whole `ag-sandbox` suite with `AG_SANDBOX_CGROUP_BASE` on an empty directory the job delegated (`+memory +pids`), `AG_REQUIRE_CGROUP=1`, `AG_REQUIRE_SANDBOX=1`; includes `tests/cgroup_real.rs` | green |

Asserted **from inside the container** (`oci_real.rs`): no route to the network (with a positive control that reaches the
host's listener under `--network host`), read-only root, `NoNewPrivs`, no capabilities, `memory.max`/`pids.max` of the
container's cgroup, a fork bomb stopped at the pids limit, memory hog killed by the kernel, CPU quota and CPU-time limit,
descriptor/core/file-size limits, size-capped `/tmp`, read-only mounts, none of the worker's keys or environment visible,
exit statuses and stdin, timeout/flood/cancel cleanup with no container left, orphan reaping, a missing image refused.

Run history (each failure was a bug in the code or the docs, and was fixed there):

| Run | What it showed |
|---|---|
| [39109282344](https://github.com/Paffin/AgentGit/actions/runs/39109282344) | all four jobs red: a container left in `Created` when the client was killed mid-creation (the kill hook now waits for the container to exist and runs before the client is killed); a cgroup OOM kill took the namespace init with it (`memory.oom.group`), now classified as `MemoryLimit`; Podman spells capabilities and ulimits differently from Docker in `inspect` (the record check now normalises them) |
| [39109810465](https://github.com/Paffin/AgentGit/actions/runs/39109810465) | docker and cgroup v2 green; both Podman jobs red on the record check |
| [39110505559](https://github.com/Paffin/AgentGit/actions/runs/39110505559) | Podman 13 of 14: it does not set `State.OOMKilled` for a kill the kernel logs as a memory-cgroup OOM kill. The sandbox does not guess (137 is also any other `SIGKILL`): it reports `Exited(137)`, says why in the report notes, and the test asserts exactly that per engine (`RuntimeInfo::engine`) |
| [39111592492](https://github.com/Paffin/AgentGit/actions/runs/39111592492) | **all four jobs green** (the worker end to end on Podman included) |

What changed in the code because of it: `OciSandbox` reads the runtime's own record (`inspect`, `info`) back and fails a run whose
protections are not what was asked (`oci_record.rs`); `verified` means "read back", never "the kernel enforced it this run";
`run_cancellable` (lease lost, stop requested) kills and removes the container; `reap_orphans`; start-up checks of the daemon
and the image; `--oci-user` defaults that work for rootful/rootless; `OciConfig` timeouts; the cgroup base is explicit
(`AG_SANDBOX_CGROUP_BASE`, `--cgroup-base`) and the no-internal-processes rule is documented and tested.

`docs/architecture/sandbox.md` now has a claim-by-claim table of what is verified where (Linux CI, macOS CI, real Docker/Podman,
delegated cgroup v2) and a list of what is **not** verified; `docs/guides/workers.md` says the same for operators.

## Findings

### CONFIRMED (all fixed)

| Item (`file:line`, severity) | Fix | Test that fails without it |
|---|---|---|
| `ag-agent/src/predict.rs:39` medium: prediction collected every ancestor of every head | `ag_dag::divergence`: a bidirectional walk whose cost is the size of the divergence, not the history under it; `predict` uses it | `ag-dag`: `divergence_of_two_lines_that_forked_from_a_long_history_costs_only_the_fork` (a graph that counts the nodes it is asked for), `divergence_of_related_and_identical_changesets`, a proptest against the naive set difference; `ag-agent`: `prediction_walks_the_divergence_not_the_history_under_it` |
| `predict.rs:71` medium: 1000 proposals cut before the stream filter, no signal | `Tx::proposals_open_for_stream`: every open proposal of the stream, filtered in SQL | `ag-agent`: `proposals_of_other_streams_never_crowd_out_the_ones_that_matter` |
| `agwp.proto:14` medium: `SetPolicy` claimed idempotent, appended a version each time | the same document again is a no-op (no new version, attestations stay valid) | `ag-integrate/tests/policy.rs`: `setting_the_same_policy_again_is_a_no_op_that_keeps_the_attestations_valid` |
| `agwp.proto:722` medium: `Conflict.kind` was display text | `ConflictKind::code()` (fixed strings) is the wire `kind`; the display text moved to the additive `Conflict.message` (and is what `ag sync` prints) | `ag-merge`: `every_conflict_kind_has_its_own_stable_code`; `ag-integrate/tests/engine.rs`: `every_kind_of_conflict_has_a_stable_code_on_the_wire`, `stored_outcomes_without_a_message_still_read`; `ag-server/tests/agents_flow.rs` asserts the wire fields |
| `agwp.proto:927` medium: run status `stale` | already fixed on `main` by `server_hardening` (`abandoned`); dropped | - |
| `agwp.proto:1129` medium: event topics nobody emits | the comment now says what is emitted (the audit actions) | comment only |
| `agwp.proto:728` low: `Blocker.code` vocabulary | `Unmet::code()` is a fixed string, equal to the serde tag, used by `Blocker::from_unmet` | `ag-policy`: `every_unmet_requirement_has_a_fixed_wire_code_equal_to_its_serde_tag` |
| `ag-cli/src/cmds/remote.rs:142` medium: fixed 120 s deadline; `clone` built its own options | `AG_TIMEOUT` (positive whole seconds, else refused), one `options_for`, a push sends objects in batches of at most 32 MiB (`Options::upload_batch_bytes`), downloads end at the deadline when the server goes quiet, a client's own deadline is `deadline_exceeded` | `ag-cli/tests/remote_scale.rs`: `a_deadline_that_is_not_a_positive_number_of_seconds_is_refused`; `ag-client/tests/client.rs`: `uploads_are_split_into_calls_of_a_bounded_size`, `a_call_that_the_server_never_answers_ends_at_the_deadline`, `a_stream_that_goes_quiet_ends_at_the_deadline_instead_of_hanging`; `ag-protocol`: `errors_survive_a_grpc_status_round_trip` |
| `remote.rs:754` medium: first page only | `proposals` and `events` read every page, `log -n` asks for as many pages as it needs, `blame --max-commits` | `remote_scale.rs`: `a_listing_of_more_proposals_than_one_page_holds_is_complete`, `replaying_the_events_reads_every_page` |
| `ag-git/src/run.rs:8` medium: ambient `GIT_*` environment and user config | `git_command()` removes `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY`, `GIT_COMMON_DIR`, `GIT_NAMESPACE`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`, `GIT_PREFIX` for every git process; `log` gets `--no-show-signature --encoding=UTF-8` | `ag-cli/tests/git_env.rs`: `a_hook_environment_does_not_redirect_the_bridge_to_another_repository`; `ag-git/tests/environment.rs`: `the_users_git_configuration_does_not_change_what_is_imported` |
| `flow.rs:189` low: silent caps of 200/50/200 | `-n/--limit` on `proposal list`, `agent runs`, `lease list --all` (0 = all) and a stderr hint when a listing stopped at its cap | `remote_scale.rs`: `listings_stop_at_a_cap_that_is_said_out_loud` |
| `remote.rs:725` low: "Removed remote" with the credential still on disk | the credential is deleted first; if that fails the remote is kept and the error names the file | `remote_scale.rs`: `removing_a_remote_whose_credential_cannot_be_deleted_says_so_and_keeps_the_remote` |
| `remote.rs:958` low: `events --follow --json` was concatenated pretty JSON | one compact object per line (NDJSON); `--json` help topic and reference say so | `remote_scale.rs`: `a_live_feed_is_one_json_object_per_line` |
| `repo.rs:256` low: any error became "no history yet" | only `NotFound` does | `remote_scale.rs`: `a_metadata_database_that_cannot_be_read_is_not_an_empty_history` |
| `ag-git/src/export.rs:53` low: hand-made ref-name rules, after `git init` | `git check-ref-format` before anything is created; the real git error is reported when fast-import fails | `ag-git/tests/environment.rs`: `a_branch_name_is_judged_by_git_and_before_anything_is_created` |
| `ag-git/src/lib.rs:17` low: "everything is reported" | the guide says what is reported; `ImportReport.lossy_commits` is new (CLI prints it, `--json` has it) | `environment.rs`: `text_that_is_not_utf8_is_repaired_and_the_report_says_so` |
| `ag-sandbox/src/oci.rs:148` medium: `--pull never` contract not stated or checked | the contract is in the docs; `ag-worker run --sandbox oci` asks the daemon and looks for `--oci-image` at start-up; a policy image that is not in the store fails the job before the download; `doctor --oci-image` | `tests/oci_real.rs`: `a_missing_image_is_a_runtime_error_found_before_any_work_and_the_sandbox_never_pulls`; `ag-worker/tests/oci_e2e.rs`: `an_image_that_is_not_in_the_local_store_fails_the_job_before_the_tree_is_downloaded`; `tests/oci.rs`: `the_image_and_the_daemon_can_be_checked_before_any_job` |
| `ag-worker/src/main.rs:104` medium: container user could not write the tree for a non-root/rootless worker | `--oci-user` is numeric with a default that works (65534 for a root worker that hands the tree over, own ids for rootful, `0:0` for rootless); a root worker needs a numeric user | `ag-worker` unit tests `the_oci_user_is_numeric_and_the_flag_has_no_hidden_default`, `a_root_worker_needs_a_numeric_container_user_to_hand_over_the_tree`; `oci_e2e.rs`: `a_check_runs_in_a_real_container_on_the_exact_tree_and_can_write_it` on docker, rootful and rootless Podman |
| `ag-sandbox/src/detect.rs:35` low: "available" because a file is on `PATH` | `probe_runtime` asks the daemon (`info`), `RuntimeInfo` says rootless/user namespace/seccomp/engine; `detect` and `doctor` use it | `tests/oci.rs`: `the_image_and_the_daemon_can_be_checked_before_any_job`; real runtimes: `the_report_is_what_the_runtime_recorded_and_stable` |
| `ag-worker/src/worker.rs:271` low: no cancellation | `Sandbox::run_cancellable`: a lost lease or a stop request kills the check (the container is stopped and removed); a second signal does it for all running checks | `tests/oci_real.rs`: `cancelling_a_run_kills_and_removes_the_container`; `tests/oci.rs`: `a_cancelled_run_kills_and_removes_the_container_and_returns_no_outcome`; `oci_e2e.rs`: `stopping_the_worker_now_kills_the_container_and_hands_the_job_back` |

### CONTESTED (triaged)

Index `[n]` is the position in `dev/audit/audit-final.json`. "Fixed" means a code or test change; "stays" says why, citing code or docs.

| Item | Decision |
|---|---|
| [19] `predict.rs:51` overlap paths cut to 20 without a signal | Fixed: the constant is `MAX_OVERLAP_PATHS`, `Overlap.paths` in the proto says "at most 20, a sample", test `an_overlap_lists_at_most_the_documented_number_of_paths`. No `truncated` field: that would be a protocol addition for an advisory signal (the overlap's existence is the signal, real conflicts come from merge preview) |
| [20] `provenance.rs:45` `prompt_digest` unvalidated, `hex`/`sha2` unused | Fixed: `attach`/`attach_signed` refuse anything but 64 lowercase hex (`provenance_is_signed_verified_and_tamper_evident`); the two unused dependencies are gone |
| [21] `eval.rs:36` `Facts.stream` ignored | Fixed: the field is removed |
| [22] `agwp.proto:1417` undocumented defaults and caps | Fixed in the proto comments (claim lease and wait, heartbeat, diff context, subscribe heartbeat); no behaviour change |
| [100] `ag-client` `with_run` swallows a bad run id | Fixed: `with_run` returns `Result`; `a_run_id_that_cannot_be_sent_is_an_error_and_never_dropped` |
| [101] `get_file` returns a truncated stream as a whole file | Fixed: the last-chunk marker and the announced size are checked; `a_file_that_stops_early_or_is_not_the_announced_size_is_an_error` (tonic reports a clean truncation as an error, but a server that simply stops sending is not) |
| [102] `ag-client` has no tests | Fixed: `crates/ag-client/tests/client.rs` (11 tests: real server for chunks, batches, 10 500 ids in `missing`, bad run ids; a scripted server only for what a correct server never does: fail on demand, lie about a stream, go quiet) |
| [104] `git log` framing with in-band separators, lossy decoding | Fixed: NUL framing (`-z`), `lossy_commits` in the report, fuzz target `git_parse` updated; `a_message_may_contain_any_character_except_nul`, `the_log_parser_rejects_what_is_not_whole_records` |
| [105] `cat-file` stderr discarded | Fixed: stderr is captured and is the error when `cat-file` dies (also when it dies mid-blob); `when_cat_file_dies_the_error_is_the_reason_it_gave` |
| [108] OCI backend only against a fake script | Fixed: the whole proof above |
| [109] OCI isolation flags are constants | Fixed: read back from `inspect`/`info` (`oci_record.rs`); `host_root` from the configured user; `verified` means "read back" |
| [112] cgroup v2 only against a fake directory | Fixed: `tests/cgroup_real.rs` and the whole suite on a delegated cgroup in CI |
| [113] `open_files` and `file_size_bytes` fixed | Fixed: `--open-files`, `--file-size-mb` (profile fields, in the digest) |
| [114] output cap, `/tmp` size, stored tail, default CPU fixed | Fixed: `--output-mb`, `--tmp-mb`, `--result-tail-kb`, `--default-cpu-secs`; `every_limit_a_worker_operator_may_want_to_change_is_a_flag`, `a_limit_of_zero_is_refused_not_passed_to_the_sandbox` |
| [115] default system mounts carry no identity | Fixed: `--system-mounts-identity`, a start-up warning and `doctor` list the mounts without one; `the_system_mounts_can_be_named_and_the_digest_then_binds_what_they_are`, `mounts_that_name_no_content_are_listed` |
| [117] pinned-digest check runs after the check | **Stays.** The digest describes the run (isolation actually in force), so it cannot be known before it; the server re-checks at completion; documented in `workers.md` ("what the worker does" and "limits worth knowing") |
| [119] retry delay, backoff, client timeout fixed; silent clamps | Fixed: `--retry-after-secs`, `--backoff-max-secs`, `--client-timeout-secs`, `--shutdown-grace-secs`; a lease or wait outside its range is corrected with a warning |
| [120] `classify` reclassifies any non-zero exit as `MemoryLimit` | Fixed: only a death by a signal is, thresholds are named constants (`NEAR_MEMORY_NUM/DEN`, `CPU_KILL_SLACK_MS`), four unit tests in `supervise.rs` |
| [121] digest does not cover the filter's strength or a profile version | **Stays**, with the docs corrected: hashing a profile version or the filter changes every digest and every pinned `required_environment_digest` (a format/hash change: it needs an ADR, `docs/adr/`). What the digest covers and does not cover is now stated exactly in `digest.rs` and `sandbox.md` (including that `memory`/`tasks` enforcement kinds are not hashed, which the old doc claimed) |
| [122] exit 125 decided by substring matching of stderr | Fixed: the container's `State` decides (a container that only reached `created` never ran); the substrings are the fallback when `inspect` is unreachable; `a_command_that_never_started_is_an_exec_error_and_one_that_exits_125_is_a_failed_check` |
| [123] `kill`/`rm` results dropped, OOM read defaults to false | Fixed: removal failure is a note and `reap_orphans` finds the container; an unreadable state with a 137/signal is an error, not a guess; timeouts are `OciConfig` fields |
| [124] predictable job dir name, existing directory trusted | Fixed: `job_dir` fails on RNG error, directories are created exclusively, a scratch directory somebody else could have prepared is refused; `a_scratch_directory_somebody_else_could_have_prepared_is_refused` |
| [125] vacuous assertions in `isolation.rs` | Fixed: unique `sleep` markers polled from the host's process table; the remount test checks the exit codes of the commands |
| [126] every checkout error is retryable | Fixed: `CheckoutError::Unsafe` is a permanent failure; `what_no_worker_could_check_is_told_apart_from_what_another_attempt_might_fix` |

### Re-read at commit 5d35ee2 and dropped

* `proto/agwp/v1/agwp.proto:927` (run status `stale` vs `abandoned`): already fixed on `main` by `server_hardening` (the comment
  says `abandoned`); nothing left to do.

## Evidence

Local, on the tree that was pushed (rebased on `9c84add`):

* `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo +1.89.0 check --workspace --all-targets`
  (the MSRV job).
* Tests, crate by crate (a full `cargo test --workspace` does not fit this machine's disk: ~165 MB per test executable that links
  `ag-server`): `ag-dag` 11, `ag-merge` 41, `ag-policy` 28, `ag-storage` 123, `ag-agent` 19, `ag-integrate` 98, `ag-protocol` 6,
  `ag-git` 16, `ag-sandbox` 142 (the namespace and OCI suites **skip** on this host, see below), `ag-worker` 52, `ag-client` 11,
  `ag-cli` 93, `ag-server` 177, `ag-mcp` 70; all passed. The OpenAPI golden and `api_contract` pass without regeneration.
  Docker 29.8.2 ran `oci_real.rs` (14) and `oci_e2e.rs` locally before CI did.
* Web UI: `vue-tsc` (app, test, node), `prettier --check .`, `eslint .`, `vitest run` (347 passed), `check:api`.
  Go orchestrator: `go vet ./...`, `go test ./internal/...`.
* `scripts/check_docs.py --require-ag`, `gen_cli_docs.py --check`, `check_no_stubs.py`, `check_workflows.py --strict`,
  `sdk/tools/sync_doc_samples.py --check`, `gen_readme_assets.py --check`.
* **Mutation checks** (a fix was undone and the named test failed): the `ag-cli` tests of `remote.rs` against the old
  `remote.rs` (five failed), `rev_or_default` against the old function, the `GIT_*` scrub with an empty list, the
  `Timeout expired` mapping in `ag-protocol`.
* Things the local runs found that the unit tests of the same commits had not: the troubleshooting tutorial relied on "storing
  the same policy again is a new version" (the idempotent `SetPolicy` made it false: the tutorial now changes the policy, the
  CLI says "unchanged"); `ag integrate` printed the new conflict code instead of the words; the Web UI's test types and
  its e2e expected the old `kind` text (the UI shows `message` now); `vue-tsc` failed on specs built before the schema gained
  `message`.

CI on `main` for `903499d` (the code that this page describes; the commit that adds this page changes only the page and two
fuzz targets):

* **OCI runtimes and cgroup v2**: [run 39115811180](https://github.com/Paffin/AgentGit/actions/runs/39115811180) green, all four jobs
  (docker, podman rootful, podman rootless, namespace sandbox on a delegated cgroup v2).
* **Packaging** green ([39115811164](https://github.com/Paffin/AgentGit/actions/runs/39115811164)).
* **Fuzz** was **red**: `fuzz/` is a workspace of its own that no local gate builds, and two targets did not compile against the
  changed APIs (`job_spec`: `Backend` gained fields, `policy_eval`: `Facts.stream` is gone). Fixed in the commit that adds this
  page; `cargo check --manifest-path fuzz/Cargo.toml --features heavy --bins` is clean. Lesson: build the fuzz crate when a
  public type of the fuzzed crates changes.
* The other workflows (CI, Docs, Windows, Web UI, Containers, Quality, Security, CodeQL) were still queued or running when this
  page was written; a push to `main` restarts the CI of `main`, so their results are reported by the package's final report
  and not frozen here.

## What is NOT verified

* Other versions and configurations of the runtimes (only the runner's Docker 28.0.4 and Podman 4.9.3); Docker Desktop, WSL,
  SELinux-enforcing hosts, `userns-remap`, CRI-O/containerd front ends, gVisor, Kata; any image other than BusyBox.
* microVMs as the boundary around the worker: recommended in the docs, never exercised.
* Windows (no sandbox; `ag-worker` refuses to run), macOS beyond the limits-only fallback, aarch64 beyond `cargo check`, cgroup v1.
* The cgroup base delegated by a systemd unit (`Delegate=yes`): CI creates the same layout by hand.
* The root-only namespace tests run as root only on the original development host; CI runs as the unprivileged runner user.
* On this development host the namespace sandbox cannot start at all (`bind mount on /work: Invalid argument`), so the
  namespace, cgroup and Podman evidence is only what Actions shows; Docker 29.8.2 was run locally.
* Podman does not record OOM kills (see above): a memory kill under Podman is `Exited(137)`.
* The environment digest still binds booleans, limits, mounts and the image, not the strength of the filter or the kernel
  (contested item "digest profile version", below).

## Next steps

* Add a `CHANGELOG`/`README` line (the final report of this package carries the text; the brief forbids touching them here).
* If a policy must pin the sandbox's strength, an ADR for a profile version in the environment digest (see the contested table).
* Run the runtimes workflow on a schedule so a new runner image (new Docker/Podman) is noticed.
* Pull `Podman`'s OOM state from `podman events` or the cgroup if a future version of it records it, then drop the note.
* A development note for whoever runs the suites next on a machine that several sessions share: with a shared
  `CARGO_TARGET_DIR`, a workspace crate that another session built from its own sources *later* than the last edit
  here looks fresh to cargo (its dependency information is relative to the package) and is linked as it is: the symptom
  is an `unresolved import` of something that is in the source. `touch`-ing the sources, or a target directory of one's own
  (`CARGO_PROFILE_DEV_DEBUG=0` keeps it small), is the cure; so is running `ag-server`'s tests one crate at a time, because
  each test executable that links it is ~165 MB.
* The `git_parse` fuzz target was changed (log framing) and could not be built locally (libFuzzer needs a nightly toolchain
  and the disk); the `Fuzz` workflow builds and runs it on `main`.
