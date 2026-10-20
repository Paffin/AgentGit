# Security model

AgentGit assumes that some of the code, some of the people and — above all — some of the *agents* touching
a repository are wrong, confused or hostile. This page states what is defended against, where the defence
lives, which test exercises it, and what is **not** defended.

To report a vulnerability see [SECURITY.md](../../SECURITY.md).

## Trust zones

Four inputs are treated as untrusted: **user code**, **repository content**, **agents**, and **results of
tools run on that content**. The deterministic core (hashing, formats, merge, CAS) does not depend on any
of them being honest.

| Zone | What it can do | What it cannot do |
|---|---|---|
| Agent | propose changes, claim work (leases), attach signed provenance | integrate on its own (default), approve its own work, forge a CI result, write outside its workspace |
| Repository content | be merged, checked out, validated | escape the checkout, plant `.git`/`.ag`, follow a symlink out |
| Execution plane (CI) | issue signed attestations **if its key is listed in the policy** | be impersonated by anyone without its key |
| Local user with disk access | everything | — see "Known limits" |

## Controls

| Threat | Control | Where | Test |
|---|---|---|---|
| Object substitution / bit rot | content IDs recomputed on **every read**; corruption is an error, never data | `ObjectStore::get` | `fs_detects_corruption_and_put_heals_it`, `a_damaged_blob_is_an_error_not_a_silent_checkout` |
| Non-canonical encodings that decode to the same value | strict decoders: minimal varints, no trailing bytes, sorted unique ops | `ag_object::canon`, `Changeset::decode` | `rejects_non_minimal_and_overflowing_varints`, `accepted_varints_reencode_identically`, cross-implementation vectors |
| Hash confusion between object types | domain-separated IDs (kind and version in the preimage) | `ag_object::object_id` | `domain_separation_by_kind_and_version` |
| Oversized / pathological input | limits on blob size, ops, parents, message, DAG depth, tree size; length prefixes bounded before allocation | `ag_types::Limits`, `Reader::bounded_varint` | `bounded_reads_do_not_allocate_huge_buffers`, `rejects_duplicate_parents_and_limits` |
| A damaged or hand-edited control row widening access | a credential whose `scope_json`, or a repository whose `quota_json`, cannot be read is an error (`internal`), never the default — and the default scope is "unrestricted", the default quota "unlimited" | `Control::credential_from`, `repo_from` | `an_unreadable_scope_or_quota_is_an_error_never_a_wider_grant`, `a_credential_whose_scope_cannot_be_read_never_authenticates_as_unrestricted` |
| A delegated credential wider than its minter | scope containment is decided only where it can be proved (`*`, equal patterns, prefix patterns); anything else is refused | `authz::pattern_within`, `Scope::contains` | `containment_never_claims_more_than_is_true`, `a_scoped_credential_cannot_mint_a_wider_one_with_a_glob_it_only_looks_like_it_contains` |
| One run of an agent acting for another, or a session credential that outlives its session | a credential can be bound to **one run** (`scope.run_id`, ADR-0010): it acts only as that run, is checked against the run on every request, and dies with it; an open event stream ends too. Name-bound credentials remain (see the known limits) | `App::dispatch` (`ensure_bound_run_alive`), `effective_actor`, `owns` | `a_run_bound_credential_acts_only_as_its_run`, `a_run_finishes_only_its_own_task_and_signs_only_its_own_provenance`, `a_run_bound_credential_manages_only_its_own_run_credentials`, `a_credential_can_only_be_bound_to_a_running_run_of_its_own_agent`, `a_run_bound_credential_dies_with_its_run`, `a_credential_stops_working_when_its_run_is_gone_even_if_it_was_never_revoked`, `an_event_stream_opened_with_a_run_bound_credential_ends_with_the_run` |
| A service that starts agents holding every administrative power | the `delegator` role mints credentials for `agent:*` subjects only, with the capabilities it holds itself on every repository of the scope and never the administrative ones (`NEVER_DELEGATED`); a test fails when a new administrator-only capability is not on that list | `Role::Delegator`, `Authorizer::delegable`, `App::mint_credential` | `a_delegator_mints_credentials_for_agents_and_only_for_agents`, `a_delegator_cannot_escalate_through_an_agent_that_is_more_powerful_than_it`, `a_delegator_lists_and_revokes_the_credentials_of_agents_and_no_others`, `every_capability_only_administrators_hold_is_never_delegated`, `a_delegator_can_pass_on_what_it_holds_where_it_holds_it_and_no_more` |
| Dead runs holding tasks and leases forever; operator actions passing as the agent's | `agent.operate` finishes a task whatever its owner and reaps silent runs (never below `agents.min_stale_after_secs`); every action is audited under the operator's identity with `operator: true` and the owner it overrode | `ag_agent::registry::reap_runs`, `finish_task_as_operator` | `an_operator_finishes_a_task_that_belongs_to_somebody_else_and_it_is_on_record`, `acting_on_another_agents_run_needs_agent_operate_and_a_writer_does_not_have_it`, `an_operator_reaps_stale_runs_their_leases_and_their_credentials`, `a_task_is_not_started_twice_while_its_first_run_lives_and_can_be_once_it_is_reaped`, `the_server_reaps_stale_runs_by_itself_when_it_is_told_to` |
| Role bindings that silently never apply | patterns are validated when granted; every binding reaches the authorizer (no row limit) | `Control::grant_role`, `all_bindings` | `a_role_can_only_be_granted_on_patterns_that_compile`, `every_role_binding_reaches_the_authorizer_however_many_there_are` |
| A revoked credential or role still honoured by another replica | credentials are looked up in the control database on every request; role bindings are cached for at most `control.bindings_ttl_ms` and dropped on a cluster notice; an open event stream re-checks its credential every 5 s | `Authenticator::still_active`, `cluster::Bus` | `a_revoked_credential_and_a_revoked_role_stop_working_on_every_replica` (`multi_replica.rs`) |
| Filling a repository past its quota | bytes are charged where they enter the object store, atomically across requests and replicas | `quota::MeteredStore` | `workspace_edits_are_counted_and_limited_like_uploads`, `concurrent_workspace_edits_cannot_pass_the_quota_between_them` |
| Memory pinned by unauthenticated uploads | an object body is read only after authentication, rate limiting and authorization, and no further than the limit for that repository | `PUT …/objects/{id}` | `an_unauthenticated_upload_is_refused_before_its_body_is_read` |
| Path traversal | `RepoPath` forbids `..`, `.`, absolute paths, empty components, NUL/control chars, `\`; **re-validated on decode** | `ag_types::RepoPath` | `rejects_dangerous_paths`, `decode_rejects_traversal_paths`, property test |
| Planting `.git` / `.ag` (hook injection) | those components are rejected in any case combination | `RepoPath`, Git importer | `hostile_git_content_cannot_escape_or_plant_dot_git` |
| Writing through symlinks on checkout | every parent must be a real directory; symlink targets must stay inside the root (no absolute, no `..` climbing above it); temp-file + rename | `workdir::{safe_target,check_link_target}`; also re-checked at publication | `checkout_refuses_to_write_through_symlinked_directories`, `symlink_targets_must_stay_inside_the_root` |
| Terminal escape injection via commit messages, names, diffs | control characters are rendered as `\u{…}` in human output | `sanitize_for_terminal` | `hostile_text_in_history_cannot_inject_terminal_escapes` |
| Lost or duplicated update under races | stream CAS on `(head, version)` inside the committing transaction | `Tx::cas_stream`, `commit_integration` | concurrency tests (12 and 24 parallel connections) |
| Replay / double submission | idempotency keys bound to a request hash | `intent_create` | `reused_idempotency_key_for_a_different_request_is_refused` |
| Forged CI results | attestations are signed, bound to tree + policy version + environment, and only count if issuer *and key* are in `trusted_issuers`; a trusted FAIL beats a PASS | `ag-policy` | `self_signed_agent_attestations_are_worthless`, `attestation_is_bound_to_tree_policy_version_and_signature` |
| Stale validation | the candidate tree ID changes when the stream moves, so old attestations stop matching | engine | `attestation_cannot_be_reused_after_the_stream_moves` |
| Self-approval, approval laundering | approvals bind to the reviewed head; compared by *principal*, so new runs do not help; agents don't count by default | `ag-policy` | `one_person_cannot_approve_twice_via_runs`, `approvals_are_bound_to_the_reviewed_head_and_exclude_the_author` |
| Agents exceeding their mandate | integration by agents is off unless `agent_autonomy` allows it and the change fits (paths, file count, no protected paths); human approvals required otherwise | `ag-policy` | `agent_autonomy_envelope_and_provenance`, `agents_propose_humans_integrate` |
| A stale worker acting on a job claim that was handed out again | the claim number (`attempts`) as an optional `fencing_token` on heartbeat/complete/fail, checked in the same statement that applies the change (ADR-0012) | `ag-storage` jobs, `ag-server` jobs API | `a_stale_claim_of_the_same_worker_is_fenced_off` |
| Stale lock holders | exclusive leases + strictly increasing fencing tokens checked server-side | `ag-agent::leases` | `fencing_tokens_are_strictly_increasing_and_reject_stale_holders` |
| Forged provenance | records are signed with the registered agent key; the key must match the registry; a record cannot be re-filed under another changeset | `ag-agent::provenance` | `provenance_is_signed_verified_and_tamper_evident` |
| Policy typos silently weakening rules | unknown fields rejected; unsatisfiable policies rejected on load | `Policy::parse` | `unknown_fields_are_rejected`, `unsatisfiable_policies_are_rejected` |
| Tampering with history/audit | hash-chained audit entries written in the same transaction as the change; `ag audit`/`ag verify` | `Tx::audit_*`, `fsck` | `audit_chain_detects_tampering`, `fsck_reports_a_healthy_repository_and_finds_each_kind_of_damage` |
| Data loss by GC | fail closed; roots incl. reflog retention; grace period; atomic age-checked delete | `gc.rs` | GC tests in [Publication & integration](publication-and-integration.md) |
| Key theft from disk | key files `0600` on Unix, refused on load if group/world accessible; on Windows an owner-only ACL is set when the file is created but cannot be read back, so a loosened ACL is not detected ([Windows](../operations/windows.md)); never overwritten | `ag_crypto::KeyPair`, `ag_crypto::private_file` | `key_file_roundtrip_and_permissions`, `unix_files_are_0600_and_directories_0700_and_loose_files_are_refused`, `windows_files_and_directories_get_an_owner_only_acl` |

## Fuzzing

Every parser and decoder that takes bytes from outside the process has a [libFuzzer](https://llvm.org/docs/LibFuzzer.html)
target in [`fuzz/`](https://github.com/Paffin/AgentGit/tree/main/fuzz) (a cargo-fuzz workspace of its own; a pinned
nightly toolchain; the address sanitizer, debug assertions and overflow checks on). A target asserts more than "does
not crash": what decodes re-encodes to the same bytes, applying the hunks of a diff to `a` gives exactly `b`, a required
check without a trusted, correctly signed `pass` for this exact tree is never `Allow`. The invariants are written at
the top of each file.

| Target | Takes | Crate |
|---|---|---|
| `object_envelope`, `canon_reader`, `changeset_decode`, `tree_decode` | the stored-object envelope, the canonical primitives, `ChangesetV1`, tree manifests | `ag-object`, `ag-changeset`, `ag-tree` |
| `repo_path` | path strings: nothing accepted can escape a checkout, name `.git`/`.ag` or hide a control character | `ag-types` |
| `diff_myers`, `merge_text`, `merge_trees`, `tree_ops`, `rename_detect` | arbitrary text and generated trees: determinism, symmetry, the trivial cases of a merge, the structure of renames | `ag-diff`, `ag-merge`, `ag-tree` |
| `policy_json`, `policy_eval`, `glob_match`, `ignore_rules` | policy documents, generated policies and facts, glob patterns, `.agignore` files | `ag-policy`, `ag-integrate` |
| `signed_json` | attestation and provenance JSON, key and signature hex, and a real signature check | `ag-policy`, `ag-agent`, `ag-crypto` |
| `agwp_json`, `agwp_proto` | every message of the wire protocol (187), as the HTTP/JSON gateway and as a gRPC peer receive it | `ag-protocol` |
| `git_parse` | what `git` prints (`diff-tree -z`, `log` records) and the commit-to-changeset conversion | `ag-git` |
| `sandbox_spec`, `job_spec` | a sandbox specification and a validation task as a worker receives it, down to the container runtime's arguments | `ag-sandbox`, `ag-worker` |

**Where it runs.** [`fuzz.yml`](https://github.com/Paffin/AgentGit/blob/main/.github/workflows/fuzz.yml) builds all
twenty targets once and, on every change to a fuzzed crate or to `fuzz/`, runs each for **60 seconds** from the
checked-in seeds and the corpus the nightly run cached (a smoke test: it catches a regression that a seed or a
recently found input reaches, not a bug that needs hours). **Every night** it runs each target for **15 minutes** in
four parallel jobs, minimises the corpus and caches it for the next night; a crash, a hang over 20 s or 2 GB of memory
fails the job and uploads the reproducer. By hand: `fuzz/run.sh 300 merge_trees`; reproduce a downloaded crash with
`cargo +nightly fuzz run -s address --features heavy <target> <file>`. The harness is held to the code by the same
build: on 2026-10-18 a change to the sandbox's `Backend` type left `job_spec` and `policy_eval` not compiling, the build
job of the next run on `main` failed, and the next commit fixed the harness.

**What it has found.** Each fix has a regression test that fails on the old code.

| Bug | Found by | Regression test |
|---|---|---|
| `ag_diff::unified_diff` overflowed for a huge context (`2 * context`): a panic in debug builds, a wrong hunk header in release (`ag diff -U <huge>`) | `diff_myers` | `unified_diff_survives_an_absurd_context` |
| `ag_policy::evaluate`: `now - issued_at` overflowed before the signature was checked, so a forged attestation naming a trusted issuer could trigger it | `policy_eval` | `extreme_timestamps_are_blocked_not_a_panic` |
| a `max_age_secs` above `i64::MAX` wrapped negative and made every attestation "too old" | review while writing `policy_eval` | `a_huge_max_age_means_no_limit` |
| Git import: a message over 60 000 bytes was cut inside a multi-byte character (panic); an author date above 9.2e15 s overflowed `* 1000` | review while writing `git_parse` | `crates/ag-git/tests/hostile.rs` |
| Git export: a path starting with `"` came out of `fast-import` as a different file name | the same, reproduced against real git | the same |
| `ag_server::control::Control::open`: a control database written by a *newer* server was opened and failed later with an opaque `internal error`; it is now refused at start-up, naming both versions | the compatibility fixtures, not a fuzzer | `crates/ag-server/tests/compat_control.rs` |

Harness bugs found along the way, which cost a run each: `signed_json` split a string at a byte offset inside a
multi-byte character (the target now splits at a character boundary, and has a seed for it); the list of protocol
messages in `fuzz/src/agwp_types.rs` was stale (it is generated by `fuzz/gen_agwp_types.py` and checked by CI).

**What the local campaigns found: nothing further.** Four campaigns on 2026-10-17 and 18 (a 4-core container, the
address sanitizer, each target from the checked-in seeds and the corpus the previous campaign grew; the first 11
targets for 5 minutes each, then all 20 for 2 to 10 minutes, then the 6 with the most logic for 15 minutes, and
last 5 more minutes for the four whose code or harness other changes to `main` had touched meanwhile) ran
**130 million executions in 4.3 hours** without a crash, a hang or an out-of-memory. Per target, over the
campaigns:

| Target | Time | Executions | Target | Time | Executions |
|---|---:|---:|---|---:|---:|
| `git_parse` | 35 min | 23.0 M | `merge_text` | 30 min | 0.7 M |
| `agwp_json` | 30 min | 17.5 M | `merge_trees` | 30 min | 1.9 M |
| `policy_json` | 30 min | 1.5 M | `signed_json` | 30 min | 1.4 M |
| `diff_myers` | 10 min | 0.4 M | `policy_eval` | 10 min | 43 k |
| `changeset_decode` | 7 min | 19.3 M | `tree_decode` | 7 min | 21.7 M |
| `job_spec` | 7 min | 9.9 M | `rename_detect` | 7 min | 1.2 M |
| `object_envelope` | 5 min | 9.3 M | `repo_path` | 5 min | 1.0 M |
| `agwp_proto` | 5 min | 2.9 M | `canon_reader` | 2 min | 15.3 M |
| `sandbox_spec` | 2 min | 3.0 M | `tree_ops` | 2 min | 0.2 M |
| `ignore_rules` | 2 min | 34 k | `glob_match` | 2 min | 13 k |

The 60-second smoke jobs of the workflow passed on GitHub (the first one on 2026-10-18); the 15-minute nightly job has
not run yet. These are modest numbers: the merge, policy and Git-bridge targets in particular have had far less than the
hours a fuzzer is usually given.

**What this does not show.** A coverage-guided run of minutes finds shallow bugs; the nightly runs, which start from the
corpus of the night before, are what deepens it, and the first of them had not happened when this was written. Two
targets are slow by nature and have therefore seen little (`policy_eval` checks real signatures: about 40
executions a second; `glob_match` compiles patterns: about 110). Not fuzzed at all: the HTTP layer of `ag-server`
(routing, authentication, header handling), the command-line parser, the web UI, the PostgreSQL and S3 backends, and
the isolation itself (`sandbox_spec` and `job_spec` fuzz what is *asked of* the sandbox, not the kernel).

## Prompt injection

AgentGit never feeds repository text to a model and never interprets it as instructions. Fields that agents
fill in (`inputs`, `rationale` in provenance, commit messages, file contents) are stored and displayed as
**untrusted data**; the CLI neutralizes terminal control characters. If *you* build an agent on top, treat
everything it reads from the repository the same way — AgentGit gives you identity, isolation and policy so
that a hijacked agent can still only *propose*.

## Known limits (read these)

- **Local mode trusts the local user.** `--as` / `AG_IDENTITY` is a label, not authentication. Anyone who
  can write `.ag/` can edit `meta.db` or the object store. Integrity checks and the audit chain make
  tampering **detectable**, not impossible. Enforcing identities across users requires `ag-server`
  (authentication, short-lived scoped credentials, roles, quotas).
- **`ag validate run` is not a sandbox.** It executes the command on your machine with a cleared environment
  and a temporary directory — that is hygiene, not isolation. For untrusted code use an
  [execution-plane worker](../guides/workers.md), which runs the check in the [sandbox](sandbox.md). That
  sandbox shares the host kernel (it is not a virtual machine, and no microVM boundary has been exercised), its
  OCI backend is verified against Docker 28 and Podman 4.9 (rootful and rootless) and its cgroup limits against
  a delegated cgroup v2, both only on the CI runner's versions; macOS has a limits-only fallback and Windows
  has no sandbox. All of this is spelled out, claim by claim, on its page.
- **Keys are not encrypted at rest** and live in the repository by default. Put them elsewhere with
  `AG_KEY_DIR`, and keep them out of environments that run untrusted code: whoever can read an agent's key
  can forge its provenance; whoever can read an issuer key can forge validations.
- **Run-bound credentials are opt-in per client.** A credential minted without `scope.run_id` is the agent for
  every run, exactly as before; the orchestrator and `ag-mcp` (default `--run-credentials auto`) bind every run, a
  hand-minted agent token does not. A principal with `agent.run` (a `writer`) can still start, heartbeat and complete
  the runs of any agent - attributed in the audit log (flagged `operator: true` when it is not the agent's own) but not
  prevented. Lease ownership between *plain* actors is by principal, as the [agents guide](../guides/agents.md) says.
  Not verified: several replicas serving the new paths at once, and a real agent platform driving `ag-mcp`.
- **Case-insensitive filesystems** (macOS/Windows defaults) can fold paths that differ only by case at
  checkout. Not yet detected.
- **Availability.** In local mode limits bound single inputs only; rate limits, request-size limits and
  repository quotas belong to `ag-server` (see the controls above). The server holds an object in memory
  while it verifies it; the bytes held by uploads in progress, over all callers, are bounded by
  `limits.max_inflight_object_bytes` (default 1 GiB; an upload that would pass it is refused with `unavailable` before its
  body is read; `uploads_in_flight_share_one_byte_budget_that_is_returned_when_they_end`). The budget counts the bytes
  of the objects, not the copies the verification makes (about twice that, not measured): size it to the host. With several replicas the rate limits and the concurrency
  limit apply to each replica separately (the effective limit is their sum), and PostgreSQL is a single point of failure
  unless it is run highly available: [High availability](../operations/high-availability.md#honest-limits).
- **No external audit** has been performed.
