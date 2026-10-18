# Handoff: server hardening (work package `server_hardening`)

The five items of ROADMAP "server hardening that the integration work exposed", for real, with tests and docs.
Design: [ADR-0010](../../docs/adr/0010-run-bound-credentials.md) (the brief asked for `0009-run-bound-credentials.md`;
number 0009 had been taken meanwhile by "identity names and principals", so this one is 0010).

## What landed on `main`

| Commit | Content |
|---|---|
| `d9d7184` | `ag-agent` / `ag-integrate` / `ag-storage`: `reap_runs`, operator finish of tasks and runs, one run per task, `prepare_with` (one read transaction) and the failpoint gate |
| `e9b948c` | `ag-server` + protocol: run-bound credentials, the `delegator` role, operator calls (`ReapRuns`, `admin reap-runs`, periodic reaper), exact repository counts, control schema 3, OpenAPI golden, CLI JSON golden |
| `f6c4b6c` | `ag-mcp`: run credentials |
| `fe116ad` | Go orchestrator |
| `c3fac03` | SDKs, Web UI, generated CLI reference |
| `eaa4172` | ADR-0010, docs, this handoff |

| ROADMAP item | What it is now | Where |
|---|---|---|
| Credentials bound to a **run** | `CredentialScope.run_id` (+ exactly one literal repository). Minting needs a `running` run of the subject; the credential acts only as that run (with or without `ag-run`), is checked against the run in `App::dispatch` for every operation, and dies with the run (revoked on finish/reap, refused at use time even if the revocation was lost). Workspaces, proposals, leases, tasks, provenance and the credentials of a run belong to that run alone. Control schema 3 (additive, both backends) | `authz.rs` (`Scope`, `run_id`), `app.rs` (`ensure_bound_run_alive`, `effective_actor`), `api/mod.rs` (`owns`), `api/admin.rs`, `control/*`, `migrations/control/*/0003_*` |
| **Delegator** role | `credentials.delegate` (mint/list/revoke for `agent:*` only) + `agent.operate`. A delegated credential is always explicitly scoped to the intersection, over the repositories of the scope, of what the delegator's roles grant and its own credential allows, less `NEVER_DELEGATED`; a unit test fails if an administrator-only capability is not on that list | `authz.rs` (`Role::Delegator`, `Authorizer::delegable`, `NEVER_DELEGATED`), `api/admin.rs` (`mint_credential`) |
| Server-side **finish/reap** by an operator | `task.finish` as a non-owner with `agent.operate` (audit: `operator`, the owner overridden); `run.finish` flagged `operator` when not the agent's; `POST …/runs/reap` (+ `ReapRuns` gRPC, `dry_run`, minimum threshold), `ag-server admin reap-runs`, optional periodic reaper (`[agents] reap_interval_secs`, audited as `system:reaper`); releases the leases of reaped *and* of already ended runs (`lease.reap`); one `running` run per task | `ag-agent` (`reap_runs`, `finish_run_as`, `finish_task_as_operator`), `api/agents.rs`, `server.rs`, `main.rs`, `config.rs` (`[agents]`) |
| `integration.prepare` **read-consistent** | `Repository::prepare_with` reads the verdict, the attestations and the jobs that validate the candidate in one read transaction; with `enqueue_validations` the whole answer is read again after queueing | `ag-integrate/prepare.rs`, `api/integration.rs`, `api/jobs.rs` (`validation_jobs_in`) |
| Repository listings with **real counts** | `stream_count`/`changeset_count` from the repository's own metadata in `create`, `describe` and `list` (page-bounded); a repository that cannot be read reports `counts_error` and zeros, never silent zeros. The Web UI no longer asks for each row separately | `api/repository.rs`, `ReposView.vue` |
| Clients | Go orchestrator: `delegator`, a run-bound credential per session (refuses one the server did not bind), `FinishTask`/reap as operator, no `FinishTaskAsOwner`, "share one identity" warning and its pinning test removed, `check` demands the new features. `ag-mcp`: `--run-credentials auto|required|off` (default `auto`), minted per run, kept in memory, scrubbed from output, forgotten on finish; `run_ended` error hint. TS/Python SDKs and the Web UI schema regenerated (`run_id`, `ReapRuns`, `counts_error`, role `delegator`); Web UI shows the run binding of a credential and the new role | `services/agent-orchestrator`, `crates/ag-mcp`, `sdk/*`, `apps/web` |

Protocol additions are all additive (AGWP v1 stays v1): `CredentialScope.run_id`, `ReapRuns`, `Repo.counts_error`, the
role name `delegator`, features `auth.run_credentials`, `auth.delegation`, `agents.operate`, run status `abandoned`
(documented; the old comment said `stale`). Behaviour changes worth knowing: a task cannot be started twice while a
`running` run has it (`failed_precondition`); claiming a task that is owned, blocked or finished is now
`failed_precondition` (the orchestrator already assumed it; it was `invalid_argument`); a run does not start runs.

## Evidence

* Local, on the final tree: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`;
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`; `cargo +1.89.0 check --workspace --all-targets` (the MSRV job);
  `cargo test --workspace` (884 passed and 3 ignored, 1 failed: the golden of `ag remote whoami --json`, which gained `scope.run_id`; regenerated with `AG_UPDATE_GOLDEN=1` and `gen_cli_docs.py`, that test then passes); `scripts/pg-test.sh cargo test -p ag-agent -p ag-storage -p ag-server -p ag-integrate`
  on a real PostgreSQL 16 (409 passed, 0 failed: the engine tests run on PG and the control database migration 0003; the
  preparation test also runs with the repository's metadata in PostgreSQL, where a read is `REPEATABLE READ`);
  Go orchestrator `go vet` + `go test -race ./...` against the real `ag-server` (integration tests included);
  TS SDK `npm run check` (generate:check, types, lint, 157 tests against the real server, package check); Python SDK
  `pytest` (179), `ruff check`, `ruff format --check`, `mypy`, `scripts/generate.py --check`;
  `sdk/tools/sync_doc_samples.py --check`; Web `check:api`, typecheck, lint, 347 unit tests;
  `scripts/check_docs.py`, `scripts/check_no_stubs.py`, `scripts/gen_cli_docs.py --check`, `scripts/gen_readme_assets.py --check`.
* **Mutation checks** (each new guard was removed from the code and the named tests failed; then restored):

  | Mutant | Fails |
  |---|---|
  | `ensure_bound_run_alive` returns `Ok` | `a_credential_stops_working_when_its_run_is_gone_even_if_it_was_never_revoked` |
  | `effective_actor` ignores the bound run | `a_run_bound_credential_acts_only_as_its_run` and 3 more of `run_credentials.rs` |
  | `revoke_run_credentials` matches nothing | `a_run_bound_credential_dies_with_its_run` |
  | lease check of a run (`check_lease_run`) off | `a_run_bound_credential_acts_only_as_its_run` (lease assertions) |
  | `owns` compares principals only | `a_run_bound_credential_acts_only_as_its_run` (workspace assertions) |
  | `NEVER_DELEGATED` not applied | `a_delegator_can_pass_on_what_it_holds_where_it_holds_it_and_no_more`, `a_delegator_mints_credentials_for_agents_and_only_for_agents`, `a_delegator_cannot_escalate_through_an_agent_that_is_more_powerful_than_it` |
  | a delegator may mint for non-agents | `a_delegator_mints_credentials_for_agents_and_only_for_agents` |
  | task owner rule dropped | `a_run_finishes_only_the_task_it_claimed_and_an_operator_finishes_any`, `task_dependencies_block_and_cycles_are_refused`, `an_operator_finishes_a_task_that_belongs_to_somebody_else_and_it_is_on_record` |
  | one run per task dropped | `a_task_is_worked_on_by_one_run_at_a_time`, `a_task_is_not_started_twice_while_its_first_run_lives_and_can_be_once_it_is_reaped` |
  | leases of ended runs not released | `a_reap_ends_stale_runs_and_the_leases_of_ended_runs_and_audits_them_under_the_operator` |
  | minimum reap threshold off | `an_operator_reaps_stale_runs_their_leases_and_their_credentials` (400 expected for 500 ms) |
  | the preparation reads the jobs in a transaction of their own | `a_preparation_is_one_reading_even_when_a_validation_finishes_while_it_is_made` ("pending" next to a job that already succeeded) |
  | listing counts not filled | `repository_listings_carry_the_real_number_of_streams_and_changesets` |
  | `ag-mcp` never uses the run credential | `every_run_gets_a_credential_of_its_own_that_dies_with_it` |
  | the orchestrator does not ask for a run-bound credential | `TestConcurrentSessionsOfOneAgentCannotActForEachOther`, `TestTheOrchestratorIsADelegatorNotAnAdministrator` (against the real server) |

  A mutant that did *not* fail anything was the run check I had added to the event stream's own credential re-check:
  every poll of the log is an operation and meets `effective_actor`, so the line was redundant and was removed (the
  test `an_event_stream_ends_with_the_run_even_when_its_credential_was_never_revoked` pins the behaviour).
* Test doubles: none new in product code. The Go orchestrator's *unit* tests still use `internal/testutil/fakeagwp`
  (an in-memory AGWP server, test code only, updated to the new API); every behaviour of this package is also proven by
  the integration tests against the real `ag-server`. The only test hook in product code is the engine's named
  failpoint gate `prepare.in_snapshot` (`Failpoints::pause`: a lookup in an empty map unless a test arms it).
* CI: CI_LINKS

## Audit triage

The audit rows in this package's files had been fixed by `server_audit` (see its handoff) except one: *repository
listings report zero counts* (`api/repository.rs:21`, confirmed) is fixed here (item 5). Not mine and left for the
packages that own them: `services/agent-orchestrator/internal/app/check.go:197` (`check` re-implements the server's
role patterns with a subset of glob syntax) belongs to the `orchestrator` package; I changed the role names it asks
for (`delegator` or `admin`) and nothing else there.

## Not verified / known limits

* **Several replicas.** The use-time check and the revocation use the control database and the repository, both shared,
  and `reap` on several replicas is harmless by construction (a reap that finds nothing changes nothing), but no
  multi-replica test of the new paths exists: `crates/ag-server/tests/multi_replica.rs` is not on `main`.
* **A real agent platform** driving `ag-mcp` with run credentials was not run; the tests drive `ag-mcp` over its stdio
  protocol against a real in-process `ag-server`.
* **Name-bound credentials remain.** Nothing forces a client to bind a run; a hand-minted agent token is the agent for
  every run. A principal with `agent.run` (a `writer`) can still start, heartbeat and complete the runs of any agent
  (now attributed, and flagged `operator: true` in the audit log when it is not the agent's own, but not prevented).
  Follow-up: require `agent.operate` for acting on another agent's runs (breaking for SDK examples and the Web UI demo).
* Lease ownership between *plain* actors (no run) is by principal, as before.
* The CLI has no remote reap command (`ag agent reap` is local; remote operators use the API, an SDK or
  `ag-server admin reap-runs`); `docs/guides/mcp.md` does not exist on `main` yet (the `mcp_tail` package writes it:
  document `--run-credentials` / `AG_MCP_RUN_CREDENTIALS` and the `run_ended` hint there).
* The Windows and macOS runs of the new tests are whatever CI says (links above); nothing was run on those hosts here.
* `agwp_auth_failures_total{reason="run_ended"}` counts a dead run's credential like any other refusal; the shipped
  alert (`> 5/s` of all failures) was not retuned.

## Next steps

1. Fold the changelog/README/ROADMAP notes of the final report (ROADMAP: delete the "server hardening" bullet; README
   honest-limits row "Credentials ↔ agent name" becomes "Run-bound credentials are opt-in per client").
2. `mcp_tail`: the MCP guide section on run credentials.
3. Decide on making `agent.operate` the requirement for acting on other agents' runs.
4. When `multi_replica.rs` lands, add a reap/run-credential case (revoke on one replica, use on another).
