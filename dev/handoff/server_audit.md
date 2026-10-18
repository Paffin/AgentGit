# Handoff: server audit (areas `server-security`, `server-api`)

Work package `server_audit` of the "no mocks / no stubs / no hardcodes" phase. Items are from
`dev/audit/CONFIRMED.md` / `audit-final.json` (verified at `5d35ee2`); every item was re-read against the code at
`f30e42c` before editing.

## What landed on `main`

| Commit | Content |
|---|---|
| `59dd88a` | Fail-closed control rows, every binding reaches the authorizer, sound scope containment, quota charged where bytes enter the store, events scan/revalidation/strict queries, real backends in create/list, honest `get_operation`/tree sizes, gRPC limits from config, negotiate from the engine, `trace_id`/`remote` on log lines, `public_url` removed, config validation |
| `31586ff` | Web UI generated API schema (missed in `59dd88a`; the Web UI workflow went red for one push) |
| `9a1fd1d` | Atomic idempotency records, job reaping that is counted and audited, `ag-server admin` from configuration, OIDC (lock, timeouts), queue outcomes, fsck object, lease listing limit, provenance verdicts |
| (final commit) | quota test for integrations, docs, this file |

Design points a reviewer should know:

* **Quota (`crates/ag-server/src/quota.rs`).** `App::with_repo` hands the engine a *view* of the repository
  (`Repository::with_object_store`, new in `ag-integrate`) whose object store is a `MeteredStore`. A `put` of an object
  the store does not hold is charged first. With a total-bytes quota the meter reserves bytes from the control
  database with one conditional `UPDATE` (`Control::reserve_object_bytes`, in steps of `RESERVE_STEP`, exact size as
  the fallback) and hands back what the request did not use, so racing requests/replicas cannot pass the quota
  between them. Without a quota the bytes written are added once at the end. Covers uploads, workspace edit/sync,
  integration merges, everything that goes through `with_repo`. The only other repository access is the job-index
  rescan (reads only).
* **Idempotency (`api/mod.rs`, `ag-agent::*_with`).** `run.start`, `task.create` (with its dependencies) and
  `lease.acquire` write the idempotency record inside the creating transaction; a lost race rolls back with
  `AlreadyExists` and the loser replays the winner's answer.
* **Scope containment (`authz::pattern_within`).** Decided only where provable (`*`, equal patterns, simple prefix
  patterns); everything else is refused. Consequence: a token scoped `acme/*b*` can no longer mint `acme/beta`
  (not provable); a token scoped `acme/*` can mint any longer prefix.

## Evidence

* Local: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`;
  `cargo test -p ag-server -p ag-agent -p ag-integrate -p ag-storage` (and under `scripts/pg-test.sh` for
  `ag-storage ag-agent ag-server`, i.e. the same tests on PostgreSQL: job reaping with `RETURNING`, reservations,
  idempotency records); `cargo test -p ag-cli` (`json_shapes` golden: `QueueRunReport.outcomes` is `serde(skip)`);
  `cargo test -p ag-client -p ag-mcp -p ag-worker`; TS SDK `npm run check` (incl. tests against the real server),
  Python SDK pytest/ruff/mypy/`generate.py --check`, Web `check:api`/typecheck/347 unit tests, Go orchestrator
  suite against the real server, `scripts/check_docs.py`, `sdk/tools/sync_doc_samples.py --check`.
* Every new test was shown failing against the defect it covers by re-introducing the defect (mutation run, two
  batches; mutants listed in the commit messages' areas): control fail-closed, containment, grant validation,
  binding cap, backends, operation outcome, tree sizes, gRPC limit, negotiate, event window, SSE revalidation,
  strict queries, metering off, per-repo defaults, body-before-auth upload, job reaping/dedupe/error bound,
  queue outcomes, admin limits, lease truncation.
* CI: `31586ff` all green (CI, Web UI, Windows, Containers) — https://github.com/Paffin/AgentGit/actions/runs/39098144072
  (CI), .../39098144100 (Web UI), .../39098144074 (Windows), .../39098144071 (Containers).
  `9a1fd1d` all green too — CI https://github.com/Paffin/AgentGit/actions/runs/39099362391, Web UI .../39099362316,
  Windows .../39099362275, Containers .../39099362375, Packaging .../39099362305. The last two commits (a test and
  docs) are in the workflow's final report.

## Triage of every item (60 audit rows in these two areas)

Confirmed items are all fixed (rows with the same file:line in both areas are one fix). Contested ones:

| Item | Decision |
|---|---|
| streams.rs:45 quotas check-then-act | **Object bytes: fixed (atomic).** Stream and open-proposal counts stay check-then-insert: the create is an engine call with its own transaction (`ag-integrate`), an atomic count needs an engine hook; overshoot is bounded by the requests in flight. Documented in `agwp-v1.md` and `adr-traceability.md`. Proposal count is now a `COUNT(*)`, not a load of 100 000 rows. |
| main.rs:232 `admin mint` TTL | Fixed (one rule, `AuthSection::credential_ttl`, for API and CLI). |
| session.rs:44 / :57 (negotiate literals, whoami `x`) | Fixed (engine `Limits` read at the source; whoami lists the bindings whose *subject* applies, with their own repo pattern). Making engine limits operator-configurable stays out: they are format limits and repositories must stay portable between servers. |
| convert.rs:303 operation outcome | Fixed. |
| server.rs:123 CORS | Fixed (validated at start-up). |
| admin.rs:210 / :171 dead branches, fsck object | Fixed (branches removed; `FsckIssue.object` added to the engine and filled). |
| limits.rs:3 rate-limit key | **Stays per credential** (documented in `agwp-v1.md` and the traceability row); OIDC sign-ins are keyed by identity because their token id changes per login (`app::bucket_key`). Doc comment corrected. Pre-auth rate limiting of anonymous traffic is not done. |
| app.rs:47 `Ingress.remote` | Fixed for logs (HTTP and gRPC). `X-Forwarded-For` / a trusted-proxy list is **not** implemented; behind a proxy `remote` is the proxy. |
| config.rs:1 | Fixed (module doc; `log_format`, `default_stream`, size limits, `burst`, CORS validated). `jwks_file`/TLS file existence is not validated at load: they fail at start-up with the path in the message. |
| grpc.rs:95 / objects 57 buffering | **Partly.** The PUT body is now read after authentication/rate limit/authorization and only up to the repository's limit. No global in-flight byte budget; `security.md` states the worst case (`max_concurrent_requests × max_object_bytes`, not measured). Objects are buffered whole because they are verified before they are stored. |
| objects.rs:88 snapshot quota | Fixed (atomic reservation). |
| jobs.rs:231 `jobs_queued` | Count instead of list (no cap), per-repo errors logged and skipped, scrape join error no longer reported as 0. The "repositories known to have work" hint remains by design (ADR-0007: rescan every 5 s). |
| agents.rs:327 provenance | Fixed (store faults are errors; only a failed verification is `verified=false`). |
| proposals.rs:67 list 100 000 | **Stays.** Offset paging over an in-memory list; the cap is far above a realistic count. Needs SQL filters + keyset tokens — a separate change. |
| agents.rs:282 lease history | Fixed (`limit`, `truncated`). |
| jobs.rs:215 / :299 index rescan, hint race | Fixed (paged, logged; a repository with any queued job — delayed ones too — stays on the list). |
| http.rs:1300 sentinel | Fixed (`NO_DEADLINE_MS`, not exported as a span attribute). |
| app.rs:116 OTLP headers | Fixed (malformed entries logged by position and name, never value). |
| control.rs:304 (dup of the 5000-binding cap) | Fixed. |

## Not verified / known limits

* No multi-process (multi-replica) run of the new atomic paths: the single-process tests run on SQLite and on
  PostgreSQL (`pg-test.sh`), the reservation and the idempotency record rely on the per-repository metadata write
  lock / one conditional `UPDATE`, but `crates/ag-server/tests/multi_replica.rs` (wip branch) is not on `main`.
* OIDC is tested against an in-process provider (`tests/security.rs`) and a hung TCP endpoint (`oidc.rs` unit
  tests), never a real IdP.
* Idempotency records live as long as the repository (no TTL cleanup).
* `RESERVE_STEP` (8 MiB) is a guess; the cost of the per-object `contains` + control-database round trips on S3 is
  not measured.
* Remaining literal tunables outside the audited rows: control-DB pool timeouts in `Control::open_postgres`,
  `CREDENTIAL_RECHECK` (5 s), `/readyz` 2 s probe, `SCAN_PAGE`.

## Next steps

1. Atomic stream/proposal quota counts (engine hook) and SQL-side proposal filters with keyset paging.
2. A global in-flight object byte budget (`limits.max_inflight_object_bytes`).
3. `server.trusted_proxies` + `X-Forwarded-For` for `remote`.
4. Run the multi-replica tests against the quota reservation and the keyed creations.
5. Idempotency record expiry.
