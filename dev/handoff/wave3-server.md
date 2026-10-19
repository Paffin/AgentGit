# Handoff: wave 3, server hardening follow-ups

The eight items of ROADMAP "Finish server hardening" and the server-side items of "Performance" that were left by
`server_hardening.md`, `server_audit.md` and `perf.md`. One commit per item, each with tests that fail without it.

## What landed on `main`

| # | Item | Commit | Where | Tests that fail without it |
|---|---|---|---|---|
| 1 | `agent.operate` for acting on another agent's run | `7e7d0bd` (+ `c1cbefc`, the `ag-mcp` test that relied on the old behaviour) | `api/agents.rs` (`check_operator_for`: start, heartbeat, complete) | `acting_on_another_agents_run_needs_agent_operate_and_a_writer_does_not_have_it` (`delegation.rs`) |
| 2 | Atomic stream / open-proposal quotas | `e3c2666` | `Repository::create_stream_capped`, `propose_capped` (count + insert in one write transaction), `MetaError::QuotaExceeded` | `racing_creates_cannot_pass_the_stream_and_open_proposal_limits` (HTTP; 16 racing creates; SQLite and PostgreSQL via `storage_backends.rs`), `ag-integrate/tests/quotas.rs` (engine; both backends) |
| 3 | `list_proposals`: SQL filters, keyset paging | `d1e7820` | `Tx::proposals_page`, `api/proposals.rs`, `encode_keyset` | `proposals_are_filtered_and_paged_by_key` (a walk is stable while proposals are opened; old offset tokens are refused), `proposal_pages_are_keyset_pages_with_sql_filters_even_when_created_at_ties` (both backends) |
| 4 | Trusted proxies | `b1711e3` | `proxy.rs` (`TrustedProxies`), `server.trusted_proxies`, `limits.auth_failures_per_second` / `auth_failure_burst` | `proxy.rs` unit tests, `behind_a_trusted_proxy_the_client_address_is_logged_and_anyone_elses_header_is_not_believed`, `failed_logins_are_counted_per_client_address_and_the_header_decides_only_behind_a_trusted_proxy`, `proxy_trust.rs` (real sockets) |
| 5 | Idempotency record TTL | `4e36754` | `repos.idempotency_ttl_secs` (7 days), `repos.idempotency_sweep_interval_secs` (1 h), `api/mod.rs` (`Idem`), `App::sweep_idempotency_records` | `idempotency_ttl.rs`, `meta_keys_are_found_by_prefix_and_value_order_and_deleted` (both backends) |
| 6 | Global in-flight object byte budget | `b812d52` | `inflight.rs`, `limits.max_inflight_object_bytes` (1 GiB), HTTP PUT and gRPC upload | `uploads_in_flight_share_one_byte_budget_that_is_returned_when_they_end`, `inflight.rs` unit tests |
| 7 | `Retry-After` consistent with `retry_after_ms` | `ce019ea` | `http.rs`: seconds rounded up, plus `Retry-After-Ms` with the exact value | `retry_after_headers_agree_with_retry_after_ms` |
| 8 | `derive_status` and `dangling_*` in `ag doctor`, `/readyz`, metrics | `c622825` | `Repository::dangling_refs`, `ag doctor` checks `derive` and `references`, `health.rs` (scan every `repos.health_scan_interval_secs`), `agwp_derive_*`, `agwp_dangling_refs{kind}` | `doctor_reports_the_derive_status_and_dangling_references`, `derive_failures_and_dangling_references_are_in_the_metrics_and_on_readyz` |

## Behaviour changes worth knowing

* **Breaking (item 1):** a person or a service with a plain `writer` role can no longer start, heartbeat or finish
  a run for an agent (`permission_denied`, naming `agent.operate`). `delegator` and `admin` can; an `agent:<name>`
  identity still acts for itself. Clients that drove agents with a writer token need the `delegator` role.
* Page tokens of `GET /proposals` changed (opaque to clients; an old token is `invalid_argument`).
* `quota_exceeded` for streams / proposals is now raised inside the transaction: a replay of a keyed proposal that
  already exists is served even at the limit.
* New defaults: `limits.max_inflight_object_bytes = 1 GiB`, `repos.idempotency_ttl_secs = 604800`,
  `repos.health_scan_interval_secs = 300`; `limits.auth_failures_per_second = 0` (off) and `server.trusted_proxies = []`.

## Not done / limits

* **Audit and `remote` (item 4).** The resolved client address is on the request's log lines and keys the
  failed-authentication limit. It is not written into the hash-chained audit entries: those are produced inside the
  engine's transactions, which know the actor but not the request. Correlate through `request_id`.
* The failed-authentication limit counts missing/invalid credentials per address; it is off by default. Per-credential
  limits are unchanged. There is no limit on anonymous traffic that does not try to authenticate beyond the
  concurrency gate.
* Keyset paging has no index on `(created_at, id)`: the query filters in SQL and sorts, which is fine up to some
  hundreds of thousands of proposals; an index needs a metadata schema migration (additive, not done here).
* Idempotency records written before this change carry no time and are never expired. The engine's own keyed
  operations (publish, propose, integrate; the `intents` table) are kept as operations.
* The in-flight budget counts the bytes of the objects, not the copies verification makes; not measured under load.
* The health scan opens every registered repository (LRU churn with thousands); no multi-replica test of it.
* Not touched: remote `ag agent reap`, queue fencing tokens (ADR), decoded-tree cache, batching writes, `perf.yml`
  on a runner.

## Evidence

See the final report of the package for the CI run links; local: `cargo fmt --all --check`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test -p agentgit-server` (all green), `scripts/pg-test.sh cargo test -p
agentgit-agent -p agentgit-storage -p agentgit-integrate -p agentgit-server` (see below), `gen_cli_docs.py --check`,
`check_docs.py`.
