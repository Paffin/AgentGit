# Handoff: clients_ops (areas web-ui, sdks, ops-docs-claims)

Audit items verified at 5d35ee2; every item was re-read against the code before editing. The work was resumed after a
usage-limit reset.

## State (landed on `main`)

* `57af49f` deploy + docs, `f2a16ac` SDKs, `d187456` Web UI + the one server fix it needed.

### Confirmed items

| Area | Item | Result |
|---|---|---|
| ops | default image tag `latest` never published | Compose (`AG_VERSION`), kustomize `newTag` and `deployment.yaml` default to the workspace version `0.1.0`, the `X.Y.Z` tag `release.yml` pushes (`:latest` exists only for a stable release). `scripts/check_deploy.py` fails when the three differ from `Cargo.toml`. Override documented in `docs/operations/deployment.md`. The tag resolves only after the first release tag is pushed (nothing is published yet). |
| ops | Dockerfile HEALTHCHECK `--url` overrides `AG_HEALTHCHECK_URL` | `CMD ["ag-server","healthcheck"]`; `check_deploy.py` asserts no `--url`; documented. |
| ops | NetworkPolicy blocks Prometheus in the Operator overlay | the overlay patches an ingress rule for namespace `monitoring` (to be changed), documented. |
| ops | PrometheusRule `job="agentgit"` dead under the Operator | `ServiceMonitor.spec.jobLabel: app.kubernetes.io/part-of`; `check_deploy.py` verifies alerts, both Prometheus configs and the ServiceMonitor agree on the job. |
| ops | latency alert/dashboard flap on long-poll operations | `operation!~"jobs.claim\|stream.head\|events.subscribe"` (names verified in `grpc.rs`/`http.rs`) in the alert and the four dashboard queries. |
| ops | `alerts.yml` header claims a running server is checked | reworded (names only). |
| ops | stale architecture text | `overview.md` (layers, crate table, "not here yet"), `adr-traceability.md` (OpenTelemetry/Prometheus, virtual workspaces, replay, Compose/kind), `agents.md`, `storage.md`. |
| ops | `public_url`, Dockerfile without UI, `check_docs` not in CI | already fixed on main before this work (config test, UI stage, `ci.yml` step with `--require-ag`). Dropped. |
| sdks | README/guide claim an `objects.put` override that does not exist | removed from TS README, Python README, `docs/guides/sdks.md`; "which bodies are binary" reworded (`bodies` is empty). |
| web | QueuePanel: "stops at the first blocked", wrong state set | `lib/queue.ts` (`QUEUE_TONE`, `summarizeQueueRun`) + spec. |
| web | `diffBase` swallowed errors -> whole tree shown as added | errors propagate; `''` only when the server answers "no base"; `useProposalData.spec.ts`. Integrate dialog warns when the author's leases could not be read. |
| web | `job.complete` without verdict shown as passed | pass / fail / stale / neutral; `policy-events.spec.ts`. |
| web | Approved verdict invented without prepare | `unknown/checking` until the policy check answers; required count never guessed; `blockers-fourstate.spec.ts`. |
| web | Agents running from newest 100 runs of any status | **server**: `agent.list_runs` with `active_only` now filters in SQL before `LIMIT` (`runs_running`), test `active_runs_are_listed_even_when_newer_runs_have_finished`; board asks `active_only`; open proposals show `100+` and a link when the page is full. |
| web | `'main'` literals (Files, History, board, repo form) | removed; Files/History wait for the repository record (error shown if it fails to load); the create form sends no stream unless typed. |
| web | whoami "public", BoardSummary held back/checking, DiffViewer remedy | texts corrected / counts split. |

### Contested items

Fixed: TimelineTab window (note when older events are not searched), LeaseTable 15 min (named constant),
StreamsView reflog (title says "newest 100"), HistoryView/FilesView regex on a 404 message (branch removed: the server
answers an empty listing), web README 429 claim, Python `idle_timeout_ms=0` (now disables the watchdog, as in
TypeScript), Python `_event` dead `None` branch, TS dead `hasBody` argument, Python stream connect/write/pool timeouts
(follow `request_timeout_ms`, 0 = none), reconnect "stable" window named, SDK README runtime claims (CI now runs Python
3.10-3.13 and Node 20: `sdk-runtimes` job in `ci.yml`; TS platform-API sentence completed), `check_deploy.py` hardcoded
PATH, deployment.md "Binary / systemd" (no unit exists: claim removed).

Already fixed on main (dropped): ReposView N+1 counts (`counts_error`/counts in the list), `policy.ts` min_approvals,
SDK operation count in `verify-package.mjs` (derived), `check_docs` in CI.

Stay, with reasons: `useNow.__activeTickers` (a named test seam, not product behaviour); `POLICY_SPEC` duplicate of
ag-policy (needs a generated schema from the server: a follow-up, no wrong value found now); TS `objects.put` response
type and query-array style by parameter name (both match the server today; a generator change would touch the
OpenAPI contract); SDK magic numbers (10 s long-poll headroom, upload batch 1000, `signing.ts` pass/fail mapping): bounded
and covered by tests, listed as follow-up; `otel-collector.yaml` debug exporter (documented as a smoke profile in
observability.md, no trace backend ships on purpose); `examples/quickstart.sh` transcript; OCI/cgroup/OIDC test doubles
(execution-plane and security packages); CHANGELOG/ROADMAP/SECURITY wording about a not-yet-tagged 0.1.0 (folded by the
orchestrator); ADR-0001 truncated (the source text was never received, stated in traceability).

## Evidence

* Local: `cargo fmt`, `clippy -p ag-storage -p ag-server --all-targets -D warnings`, `cargo test -p ag-server --test agents_flow`;
  `scripts/check_deploy.py` (docker compose checks ran; kustomize/promtool not installed locally, run in CI);
  `scripts/check_docs.py` (68 files); web: `npm run typecheck`, `lint`, 358 unit tests, Playwright chromium 150 passed /
  4 skipped before the last small edits (LeaseTable constant, StreamsView title, TimelineTab note); Python SDK 181 tests,
  ruff, mypy; TS SDK `npm run check`.
* CI at `d187456` (run links): CI https://github.com/Paffin/AgentGit/actions/runs/39126073482 (incl. the new `sdk-runtimes` matrix), Containers .../39126073481, Packaging .../39126073476, Docs .../39126073503, Windows .../39126073474, Security .../39126073539, Quality .../39126073396, Fuzz .../39126073514: all green. Web UI .../39126073533 failed once: `states.spec` cumulative layout shift 0.14 caused by my BoardSummary line growing a 'checking' suffix; fixed (always-present, single-line text), `states`+`board` e2e 78/78 over 3 repeats locally; see the final report for the re-run.

## NOT verified

* The prometheus-operator overlay (NetworkPolicy patch, jobLabel) is built by kustomize in CI only; never applied to a
  cluster.
* No browser run of the timeline note and the "100+" tile (no unit/e2e for those two); no test for the leases warning in
  the integrate dialog.
* Firefox/WebKit, real phone, screen reader.

## Unbacked or stale README claims (for the docs fold; README not edited)

1. Line ~253 and README.ru ~257: "Compose (with PostgreSQL and **MinIO**)": the stack uses SeaweedFS (CHANGELOG ~50 too).
2. "Honest limits / One server process ... No high-availability story yet": superseded by the HA work
   (`docs/operations/high-availability.md`).
3. "Credentials <-> agent name ... two concurrent runs can act for each other": run-bound credentials landed
   (ADR 0010, server_hardening).
4. "Deployment files: container images, `docker compose up`, a real cluster and the release workflow had not been run":
   image, Compose and kind now run in `containers.yml`, the release was rehearsed; a real cluster still has not.
5. "Clients: Web UI tested in Chromium on Linux only, at desktop width": tablet and phone projects exist; Firefox/WebKit
   are not green.
6. "Scale: measured at 100 000 files": ADR-0006 numbers are single-thread, in-memory (no PostgreSQL/S3 latency): say so.
7. Release/SDK rows: nothing published; the compose/kustomize default tag `0.1.0` resolves only after the first tag.

## Next steps

* Generate `POLICY_SPEC` in the UI from a server-provided schema.
* Read the SDK batch size from `negotiate().limits.max_batch`.
* Add a trace backend (Tempo/Jaeger) profile if "traces" is to be shown end to end.
