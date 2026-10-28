# Changelog

All notable changes are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres to
[Semantic Versioning](https://semver.org/) — while `0.x`, minor versions may change behaviour, but
on-disk formats are versioned and refuse to open when newer than the build understands.

## [Unreleased]

### Added

- Repository issues with durable discussion, author/admin edits, close/reopen, revision conflict protection and optional agent-task links; Web UI and HTTP/gRPC APIs. Repository metadata upgrades to schema 4; back up before upgrading.
- Tab-scoped issue drafts and live discussion refresh; generated Python and TypeScript SDK issue operations.

- The import wizard shows transfer coverage before uploading, exports a readable migration summary and checklist, and links directly to imported files, branches and team access. Administration tabs can be bookmarked and survive reloads.

## [0.3.0-beta.3] - 2026-10-28

### Added

- Permanent task pages connect prerequisites, dependant tasks, agent sessions and change requests. Add prerequisites from the browser and confirm completion/cancellation with their consequences explained. Coordination tabs survive reloads.

- Server-side literal code search over HTTP and gRPC, with revision-bound continuation, folder filters, UTF-8-safe snippets and explicit binary/large-file exclusions. Web results open the searched revision at the matching line; generated TypeScript and Python SDKs expose the operation.

- A multi-file browser editor stages text edits, additions, renames and deletions in one change request, with a shared preview and account/repository/branch-scoped tab recovery.

- Review drafts preserve comments, decisions and inspected heads across page reloads in the same tab, separately per account and proposal. Review submission retains its initiating credential and offers reauthentication after expiry.

- Change-request diffs support path filtering, viewed-file marks and hiding reviewed files. Tab-local progress is scoped to the account and exact comparison; it never counts as approval. Server truncation warnings remain visible while filtering.

### Fixed

- Pinned fuzz-tool setup tolerates partial cache restoration instead of failing when the executable already exists.

- Concurrent first requests on a replica publish independent temporary repository configurations atomically, preventing truncated JSON and missing-file errors when a shared repository is first opened.

- Virtualized code and event lists batch scroll updates per frame, preserving keyboard scrolling and avoiding Firefox scroll-linked positioning warnings.

- Code search clears results when the session changes, pins requests to the initiating credential, preserves literal query spaces and emphasizes matching text.

- Update the workspace and fuzz Markdown parser lockfiles to pulldown-cmark 0.13.5, addressing RUSTSEC-2026-0414 in development tooling.

- File editing keeps submission steps and retries attached to the initiating session, blocks duplicate sends and preserves drafts after cancellation or stale responses.

- Diff line-number columns expand for large files, preventing old/new numbers from overlapping each other or the code. Keyboard scrolling remains available on narrow screens.

- Role revocation preserves its confirmation after a failure and ignores late results after leaving the original context. Retries retain the initiating session.

- Role grants retain the submitted binding and initiating session while pending. Late results cannot affect a different repository or account; failures preserve the form for retry.

- Repository search survives opening a result, browser Back and page reload; bookmarked filters keep searching the loaded list without extra requests.

- Git import instructions accept the copied SSH addresses from GitHub and nested GitLab groups, with shell-safe commands and editable destination suggestions.

- History and change-request pagination retain loaded rows after a failure and retry the same page. Filter changes discard stale results; explicit history revisions avoid redundant reloads.
- Obsolete blame and changeset requests are cancelled when their view closes. Large diff expansion follows the selected patch, and comparison labels explain the default first-parent base.
- Uploads can retry a failed branch lookup without losing selected files. Pending branch creation, settings saves and integration attempts keep their submitted values; settings and integration results stay attached to their original target.
- Staged Git imports recover after temporary failures, keep separate references for each account and preserve request ownership across retries. Starting a new import clears only the local reference.
- Import instructions use the shared clipboard fallback; changing the source URL no longer leaves a misleading copied confirmation.
- Repository setup keeps its selected rules and initiating session. Leaving the wizard stops unsent follow-up work and late redirects; a failed rule update clearly reports that the repository was still created.
- Change-request filters are preserved in the URL for bookmarks and returning from a request. Updating a filter keeps keyboard focus in the current field.
- Browser tab titles retain the current repository when filters or anchors change, while navigation to another page still updates the title.
- Long tab strips retain readable labels, and keyboard selection brings the whole tab into view without shifting the page vertically.

### Changed

- The real Keycloak browser job reuses the pinned Playwright runtime, avoiding repeated system-package downloads while retaining real-provider, embedded-server and reviewed-skip checks.

### Known limits

- Draft recovery and viewed-file progress are local to a browser tab. Multi-file JSON drafts can be transferred manually; there is no account-backed draft synchronization.
- Code search scans literal text in files up to 1 MiB with bounded pages and explicit exclusions; it is not an indexed or regex search engine.
- Task pages coordinate existing agent tasks; a full user issue tracker, durable line-review threads and the other roadmap subsystems remain open.

## [0.3.0-beta.2] — 2026-10-27

### Added

- Reviewed version bumps on main can request the existing release pipeline after all checks pass for the exact commit. Existing complete releases are skipped, incomplete releases can be retried, and version tags are never moved.

- Reviews can require the exact inspected head through the negotiated `proposals.review_head` capability; stale submissions are rejected atomically without a review or audit entry.

- File search supports arrow keys, Home/End and Enter on any result, preserves IME input and modified link actions, and explains the keyboard controls in the dialog.

- Faster Web UI test execution with isolated, bounded VM workers, Node environments for pure functions, and cancellable test-worker shutdown. Browser CI shares one verified embedded build and runs each project once in a pinned runtime.

- Tab-scoped file drafts preserve text, original base, path and proposal description across reload and sign-in; storage failures are explicit and drafts remain separated by account/repository/branch/path.
- Repository-wide file finding, reviewed rename/move/delete, binary-safe multi-file upload with replacement review, comparison history/swap, and expandable Home summaries with older-repository selection.
- A platform goal ledger with a 142-row initial coverage inventory, staged backlog and explicit unverified acceptance work. A qualified Vitest mutation adapter detects empty test runs rather than crediting them as killed mutations.
- `ag migrate --dry-run` verifies a full Git conversion and shows its reconciliation report before creating a destination. Previewing losses does not authorize a later lossy import.

- `ag auth status` lists saved server identities and expiration locally; `--check` verifies credentials with a bounded timeout, without printing or rewriting secrets.
- Folder filtering in the web file browser, and download/discard protection for unfinished web edits.

### Fixed

- Python CI and package checks require the committed lockfile; release metadata includes the normalized Python version and regenerated licence list.

- Account and task-graph controls derive their accessible names from matching text; the account identity remains available to assistive technology when its visible label is hidden on a small screen.

- Shared browser code is bundled once while views stay lazy; lazy JavaScript no longer adds speculative preload links. Keyboard focus checks poll rendered styles instead of depending on animation frames.

- Directory navigation removes stale links immediately and retains loaded files after a next-page error. Review submission captures its target and content before waiting; clipboard feedback ignores obsolete requests.

- History follows Back/Forward filters, isolates each changeset's lazy diff, and preserves revision/path context in native parent links. Clipboard fallback restores keyboard focus and cleans up on failure.

- Closing a review dialog keeps unfinished text and decisions. A changed head requires explicit re-review; failed submissions preserve the draft, and successful submissions clear it.

- Lazy route imports no longer preload their own entry twice; dependency and CSS preloads remain, and chunk hashes include the preload policy. Mutation workers now execute their own source copies instead of resolving relative imports into the original tree.

- Content-hashed UI assets with dots in their names now keep immutable caching, including source maps. Reloads no longer revalidate these unchanged Vite chunks.

- Reopening a file removed by another participant still recovers its saved tab draft, including the original revision and review conflict checks.
- Browser qualification uses Playwright 1.64.1 after a native WebKit reload assertion on 1.56.1; the original recovery scenario passes without disabling crash, console or persistence checks.
- Web file edits now start from the exact revision read, preserving conflict detection when another user advances the branch. New-file editing also checks the expected starting point, including empty branches.

## [0.3.0-beta.1] — 2026-10-27

### Added

- Public beta for the single-server Linux/SQLite/filesystem workflow, with open external qualification gates and explicit migration limits.
- Web import from full Git bundles: private preflight, all-branch/tag reconciliation, content-loss acknowledgement, idempotent publication, and downloadable receipt. Credentials stay with local Git.
- Publication moves the complete prepared directory atomically without replacing an existing destination; a retry can recover after the move but before registry insertion.
- `ag migrate` imports a full local Git clone/mirror or bundle into a new working repository. Original bundles and reports are retained and included in backups.
- Deterministic mapping for Git branch names outside AgentGit's naming rules; unsupported refs, submodules, paths, repaired text and external LFS pointers are reported.
- Deployment smoke tests exercise real bundle import and publication in the shipped server image. Git's native bundle/pack runtime is included in the shell-less images.

### Scope

- Hosting issues, pull/merge request discussions, permissions, releases, hosted CI settings and external LFS payloads are not imported. Browser migration currently requires local storage; see [the migration guide](docs/guides/migrate-from-git.md).
- External MCP host qualification, independent pilots, physical-device and screen-reader verification remain open; beta is not a production-readiness certification.

## [0.2.0-rc.3] — 2026-10-27

### Added

- Home now supports searching visible work, filtering by repository and category, manual refresh and a last-successful-refresh timestamp. Partial reads and truncated summaries are explicitly marked; failures never imply that the work queue is clear.
- Change-request search and role-aware links to editing files or giving an agent work; glossary hints in repository navigation and table headers.
- A focused beta programme with public acceptance gates, pilot reporting, contributor entry points and a launch kit. Alpha status remains until external evidence meets the gates.
- `ag remote explain PROPOSAL` reports a server proposal's blockers, reasons and next commands with machine-readable output and distinct ready/blocked/refused exit codes.
- Policy regression coverage for preset actor/author restrictions, provenance, file limits, approval explanations and validation age formatting.

- Load scenarios verify acknowledged stream updates and read back each worker's last successfully written files
  after concurrent integrations. Empty write runs, zero duration and zero workers are rejected. Performance smoke
  runs also cover server/core pull requests and fail on execution or integrity errors without timing thresholds.

- **Zero-setup start.** `ag-server dev` makes a data directory, an administrator with a fresh 8-hour credential (replaced
  and the old one revoked at every start), the demo repository `demo/welcome` with the `agents` preset, and a reviewer
  `user:reviewer`, then prints a link that signs you in (`...#token=...`; the Web UI takes the credential out of the
  address bar at once) and the exact commands. Loopback only unless `--allow-non-loopback`. `ag login URL --token-file
  FILE` / `ag logout URL` keep a credential per server in `credentials.json` (mode 0600), which `clone`, `fetch`, `pull`,
  `push` and `ag remote` find on their own after `AG_TOKEN*`. The link is not one-time (that needs a new endpoint).
- **Policy presets** `solo`, `team`, `agents`, `strict`, as data in `ag-policy`: `ag policy preset list|show|apply`,
  `ag init --preset`, `ag remote policy --preset`, `ag-server admin policy-preset`. `ag policy explain [FILE|current]` and
  `ag remote policy --explain` read any policy document in plain words; a test fails when `Policy` gains a field without a
  sentence.
- **`ag explain [PROPOSAL]`** turns a refusal into plain words and the exact next commands (one explanation per refusal
  code, checked against the variants); the hints under `ag integrate` and `ag remote integrate|prepare` use the same
  strings. The remote workflow also supports `ag remote explain PROPOSAL`.
- **A Web UI a first-time user can use** (`docs/design/web-ux-review.md`): Home ("needs your attention") and navigation by
  job (Overview, Change requests, Agents & work, Code, Settings); plain labels with the AgentGit word in a tooltip
  (`lib/glossary.ts`, checked against the guide); every blocker carries its fix as an action (request validation, ask a
  reviewer, take the lease) and a "Next:" line; guided flows (new-repository wizard, getting-started checklist, "Give an
  agent work" that grants the role and mints the credential); a rules editor with presets and sentences, raw JSON under
  Advanced; editing a file and proposing it from the browser, with a conflict resolver; empty states that say what to do.
  Proven by `e2e/jobs.spec.ts`: a fresh user does each of the five jobs in a real browser against a real server, using
  only what the screen says. No server change.
- **Tests that cannot pass by not testing.** A shared helper per language (`ag-testkit`, Go `testutil`, `ag_testkit.py`,
  `e2e/support/components.ts`): a missing component fails when `AG_REQUIRE_COMPONENTS` names it (or `all`), otherwise it
  skips with an `AG-SKIP` marker and a JSON report. `scripts/check_skips.py` fails a CI job on a skip not in
  `tests/skip-allowlist.toml` (reason, owner and where it runs for real are mandatory) and, with `--scan`, on any raw
  skip outside the helpers; every test job runs it. Inventory: `docs/architecture/test-skips.md`.

- **`ag agent reap --remote [NAME] [--timeout D] [--dry-run]`**: reaps the silent runs of a server from the command
  line (`AgentService.ReapRuns`, needs `agent.operate`, audited under the caller as `run.abandon`); the local form
  gained `--dry-run`. The periodic reaper takes a `run-reaper` lease in the control database before each pass, so one
  replica of several reaps (the lease is time-based; two replicas briefly reaping together is harmless because a
  reap is idempotent). [ADR-0011](docs/adr/0011-proposal-paging-indexes-and-background-leases.md).
- **Metadata schema 3**: three indexes so that every page of `list_proposals` is read in index order without sorting,
  and idempotency records written before they carried a time are dated once by the first sweep and then expire.
  Metadata schema 3 is **not readable by builds that only know schema 2**: in a rolling upgrade start the new build
  first ([ADR-0007](docs/adr/0007-multi-replica-server.md)). The control schema stays at 3.
- **Decoded-tree cache** with a memory budget (`repos.tree_cache_bytes`, default 256 MiB; `0` switches it off) and
  metrics `agwp_tree_cache_*`. At 100 000 files a fast-forward went from about 330 to about 180 ms, a three-way merge
  from 880-1 050 to about 200 ms and the publish of one file from 86 to 32-38 ms; peak RSS of that run grew from
  168 to 714 MiB with a 512 MiB budget ([numbers](docs/architecture/performance.md)).
- **Bounded retry of same-stream contention** inside the call (`repos.integrate_attempts`, default 24;
  `repos.integrate_backoff_ms`, default 32; `1` means one attempt). `aborted` answers at 16 writers went from 14-16 %
  to 3-4 %; the tail is longer because losers now wait inside the call (p99 0.5-0.6 s to 0.8-1.5 s).
- **Job-claim fencing token** ([ADR-0012](docs/adr/0012-job-claim-fencing.md)): optional `fencing_token` on job
  heartbeat, complete and fail, no schema change; a stale claimant of the same worker name is refused. `ag-worker`
  sends it; the OpenAPI document, both SDKs and the Web UI types are regenerated. It is opt-in per request: an old
  worker is as exposed as before.

### Changed

- The Go suite requires `ag-server` in CI (`ag-server-binary`) and finds it through `cargo metadata`; the Keycloak Rust
  tests are no longer `#[ignore]` (they skip in plain `cargo test` and are required in the identity-provider job); the
  scripts run on Python 3.9 (`scripts-py39` job, `from __future__ import annotations`). The Web UI opens on Home; the
  old words (proposal, integrate) became Change requests and Land, with the old word in a tooltip.
- **An integration costs 3 durable commits** instead of 7 (fast-forward) or 8 (merge): the intent is written `ready`,
  tokens only if they changed, one derive commit. `ag-bench load streams` (16 clients over HTTP, 3 alternating rounds):
  87.6 to 100.0 integrations/s (+14 %, on a shared VM where less than 30 % is noise). Crash recovery at every step and
  recovery of an operation left `prepared` by an earlier build are unchanged and tested.
- Storage API: `Tx::job_heartbeat/complete/fail` take a `claim: Option<u32>` argument;
  `Repository::with_tree_cache`, `with_contention_policy` and `Meta::committed_writes` are new; the tree-size estimate
  lives in `agentgit-storage` (`approx_tree_bytes`).
- The README (English and Russian) and the release documents state the published `v0.2.0-rc.2` and what was verified
  about it; `docs/operations/releasing.md` records the owner's `gh attestation verify` of both images.

### Fixed

- Include the shipped `ag-mcp` executable in the release SBOM inventory.
- Reject incomplete archives before replacing installed binaries and explain when an older GitHub CLI cannot verify attestations.
- Keep readable proposals when another detail or the agent-run list cannot be loaded.

- Python and TypeScript SDK integration harnesses enable the startup log used to discover ephemeral ports;
  an inherited `RUST_LOG=error` no longer causes false server-start timeouts.
- The orchestrator's process-group timeout test distinguishes a running child from a dead, unreaped Linux
  child, avoiding false failures in containers whose PID 1 does not reap orphans.
- Server log-capture tests share a persistent subscriber with per-thread buffers so concurrent tests do not
  invalidate each other's tracing callsite cache. The startup-warning test also ignores inherited log filters.
- Update `ctutils` to 0.4.4 in both Cargo lock files (RUSTSEC-2026-0385) and the UI's `happy-dom` to 21.0.4
  (GHSA-834v-8h66-3h79). Remove the obsolete tinypool exceptions after both npm dependency trees became clean.

- Found by the new gate: the `PostgreSQL + S3 backends` job never lifted the AppArmor restriction on user namespaces, so
  every namespace-sandbox test had been silently skipping there; it does now and requires `namespace-sandbox`.
- Test flakes: replicas of one test cluster could be given the same port ("Address already in use"); the `ag-server dev`
  tests expected a clean exit from a killed process on Windows.
- Dependencies: `rustls-webpki` 0.103.16 and `webpki-roots` 1.0.10 (RUSTSEC-2026-0381, -0382), `certifi` 2026.10.19 in
  the Python SDK lock; a CodeQL alert in the rules-change sentence of the Web UI.
- Test races found by CI (no product code changed): `ag-mcp`'s `closing_stdin_ends_the_process_cleanly` waited for
  nothing and raced the stderr drain thread; the server's log-capturing tests lost a line to the shared tracing
  callsite and `sigterm_and_sigint_stop_the_server_gracefully` sent SIGINT before tokio had installed its handler
  (the server now installs the stop-signal handlers before it listens).

### Known limitations

- Browser editing is one text file per change request (no rename, delete or multi-file); some tab labels and table headers still need glossary hints; Home is polled every 15 s, not live; the rules form leaves protected-path approvers and check timing in
  the raw JSON; no screen-reader pass. The Docker one-liner for `ag-server dev` is not verified (no Docker daemon here), and neither `dev`, `login` nor `explain`
  is in the `v0.2.0-rc.2` archives. The root-only halves of the sandbox tests run in no CI job, and the four scripts
  that read TOML need Python 3.11 and are not in the 3.9 job.
- The tree cache clones a tree on every hit (7 ms at 100 000 files); the disk backend and PostgreSQL were not
  measured with it. Batching and the cache were not measured on PostgreSQL or S3. The reaper lease is not tested with
  real replicas on PostgreSQL and S3. Schema 3 adds three index entries per proposal write (not measured as a load).
- Fencing for job claims is not required by any server option yet (an old worker is as exposed as before).

## [0.2.0-rc.2] — 2026-10-19

Second release candidate, **published** as a GitHub pre-release (tag `v0.2.0-rc.2`, commit `0ad4753`; Release run
[39434938489](https://github.com/Paffin/AgentGit/actions/runs/39434938489), all 15 jobs green). `v0.2.0-rc.1` was
tagged but its Release run failed in the last stage, before anything was attached to a release (the release notes
were requested with a bare version instead of a tag name); no archive was ever published for it. rc.2 is rc.1 plus
the release fix and the server-hardening items below.

What the release carries: four archives (Linux x86_64 and aarch64, macOS aarch64 and x86_64), CycloneDX SBOMs for
`ag`, `ag-server` and `ag-worker` (none for `ag-mcp`), `SHA256SUMS`, the rendered Homebrew formula `agentgit.rb`,
multi-arch images `ghcr.io/paffin/agentgit-server` and `agentgit-cli` (`:0.2.0-rc.2`), and build-provenance
attestations. See [Releasing](docs/operations/releasing.md#evidence) for what was and was not verified.

### Breaking

- **Acting on another agent's run requires `agent.operate`** (held by `delegator` and `admin`). A person or service
  with a plain `writer` role can no longer start, heartbeat or finish a run for an agent (`permission_denied`, naming
  `agent.operate`); an `agent:<name>` identity still acts for itself. Clients that drove agents with a writer token
  need the `delegator` role.
- Page tokens of the proposal listing changed (opaque; an old token is `invalid_argument`).

### Added

- **Trusted proxies**: `server.trusted_proxies` decides when `X-Forwarded-For` is believed; the client address is on
  request log lines and keys a new per-address limit on failed authentications (`limits.auth_failures_per_second`,
  `auth_failure_burst`; off by default).
- **Idempotency records expire** (`repos.idempotency_ttl_secs`, 7 days; swept every
  `repos.idempotency_sweep_interval_secs`, 1 h). Records written by earlier builds carry no time and were never swept
  in rc.2 (dated in the next release).
- **A global budget for object bytes held by uploads in flight** (`limits.max_inflight_object_bytes`, 1 GiB; HTTP PUT
  and gRPC upload).
- **`derive_status` and dangling references** are shown by `ag doctor` (checks `derive` and `references`), `/readyz`
  and metrics (`agwp_derive_*`, `agwp_dangling_refs{kind}`); the server scans every
  `repos.health_scan_interval_secs` (300).
- `Retry-After-Ms` beside `Retry-After`; the seconds are rounded up and tested against `retry_after_ms`.
- A 1280x640 social preview image.

### Changed

- Open-stream and open-proposal quotas are counted in the transaction that inserts the row, so racing creates cannot
  pass the limit; a replay of a keyed proposal that already exists is served even at the limit.
- `list_proposals` filters in SQL and pages by key (stable while proposals are opened).

### Fixed

- Release workflow: the release notes step passes the tag form (`vX.Y.Z`), and an already existing release page
  (the GitHub UI creates one with a tag) is filled in instead of making `gh release create` fail.

### Known limitations

- Everything in the rc.1 list below that is not changed above still applies, except that open-stream and
  open-proposal counts are exact, idempotency records expire, there is a global byte budget and `remote` honours
  trusted proxies. Not verified for rc.2: the archive attestations, the macOS and aarch64 archives outside CI, the
  Homebrew formula; no `ag-mcp` SBOM. Image attestations (server and CLI) were verified by the owner with
  `gh attestation verify` (SLSA provenance v1, `release.yml` at `refs/tags/v0.2.0-rc.2`).

## [0.2.0-rc.1] — 2026-10-19

First release candidate and the first tag of the project (the `0.1.0` section below was an alpha that was never
tagged or published).

### Added

**Network and storage**

- **`ag-server`**: the network server. AGWP v1 over gRPC and HTTP/JSON on one port (the protobuf file is the
  single source of truth; the HTTP/JSON API is described by a generated, drift-checked `openapi.json`);
  OIDC providers, scoped expiring credentials and the bootstrap token; per-repository RBAC; request
  quotas and rate limits on every operation; idempotency keys for repositories, objects, proposals,
  integrations, tasks, runs and leases; SSE events with replay and resume; `/healthz`, `/readyz`, bearer-protected
  `/metrics`; OTLP/HTTP trace export; graceful shutdown; the execution-plane job API; `ag-server healthcheck`
  and offline `admin` commands. See [the AGWP reference](docs/protocol/agwp-v1.md).
- **The Web UI is served by `ag-server`** (`ag-ui-assets`, `[ui]` settings, SPA fallback that never shadows API
  paths, hashed assets, CSP without inline script) with OIDC code + PKCE sign-in (`[[ui.login]]`), a first-run
  "Connect your first agent" panel, a version-mismatch banner and a session-ended page. Tested in Chromium, Firefox
  and WebKit, tablet and phone layouts (emulation) and against a real Keycloak 26 (login, groups to roles, expiry,
  sign-out) in CI ([guide](docs/guides/web-ui.md)).
- **Several `ag-server` replicas** on one PostgreSQL control plane and one S3 bucket
  ([ADR-0007](docs/adr/0007-multi-replica-server.md), [guide](docs/operations/high-availability.md)): LISTEN/NOTIFY
  wake-ups, drain on shutdown, readiness that follows the control database, `admin backup-control`, `restore-control`
  and `migrate-control`. Proved by tests that start real server processes (no lost update across replicas, `kill -9`
  mid-flight, idempotency and exactly-once job claims across replicas, SSE and `Last-Event-ID` replay across
  replicas, bounded revocation, rolling restart under load, race-free migration) in the `backends` CI job on moto and
  on SeaweedFS; Kubernetes overlay `deploy/kubernetes/overlays/ha`.
- **PostgreSQL metadata and S3-compatible object storage**, selectable per repository
  ([guide](docs/operations/postgres-and-s3.md), [ADR-0004](docs/adr/0004-postgres-and-s3-backends.md)); the S3
  store, the engine and the server backends are tested in CI against a real S3 server (SeaweedFS, signatures
  verified, HTTP and HTTPS with a private CA) and against moto. New `storage.objects.request_timeout_secs`,
  `max_retries` and `list_page_size`.
- **Remote workflow in `ag`**: `clone`, `fetch`, `pull`, `push` and `ag remote …` over gRPC (new crate
  `ag-client`); credentials never stored in the repository's configuration
  ([guide](docs/guides/remote.md)). Paging for large listings, `AG_TIMEOUT`, stream idle timeouts, `-n/--limit`
  with a hint when a listing stopped at its cap, NDJSON feeds.

**Agents**

- **Run-bound credentials** (`scope.run_id`, [ADR-0010](docs/adr/0010-run-bound-credentials.md), control schema 3):
  a credential acts only as one run, is checked against the run on every request and dies with it. New `delegator`
  role (mints, lists and revokes credentials for `agent:*` only; the Go orchestrator needs it instead of `admin`),
  operator finish of tasks and runs, and reaping of silent runs (`POST …/runs/reap`, `ag-server admin reap-runs`,
  optional periodic reaper, metric `agwp_agent_runs_reaped_total`). Additive AGWP changes: `CredentialScope.run_id`,
  `ReapRuns`, `Repo.counts_error`, features `auth.run_credentials`, `auth.delegation`, `agents.operate`.
- **`ag-mcp`**: an MCP server (stdio; MCP 2026-07-28 and three older revisions; 31 tools in six groups, `integrate`
  only with `--enable-integrate`, no administration; per-run credentials with `--run-credentials`)
  ([ADR-0008](docs/adr/0008-mcp-server.md), [guide](docs/guides/mcp.md)), shipped in the release archives; protocol
  and CLI test suites, and a scripted client in CI (`examples/agents`).
- **Agent platform** (`services/agent-orchestrator`, Go): task scheduler with lease- and prediction-aware
  placement, agent sessions with run-bound credentials, execution scheduler, conflict advisor, reviewers, incremental
  semantic index, admin API; it proposes and never integrates ([guide](docs/guides/orchestrator.md)). Advisor,
  reviewers and semantic-index judge have an optional `model` mode through any OpenAI-compatible endpoint (you
  configure the endpoint, the model and the NAME of the key variable; budgets, retries, secret masking; models only
  propose comments). The simulated "scripted" agent is gone from the binary; tests run against the real `ag-server`
  through a shared harness.
- **Execution plane**: `ag-sandbox` (rootless namespaces, OCI runtime, limits-only fallback that reports itself as not
  isolation; every run returns an `IsolationReport` that claims only what was set up and is read back from the
  runtime) and `ag-worker` (claims jobs, verifies the exact tree, runs the check isolated, signs; keeps its lease;
  graceful and forced stop hand the job back) ([sandbox](docs/architecture/sandbox.md),
  [workers](docs/guides/workers.md)). Proven in CI against real Docker, rootful and rootless Podman and a delegated
  cgroup v2 (`oci-runtimes.yml`), from inside the sandbox.

**History and merge**

- **Rename-aware merge** (`ag-merge/2`): a file moved on one side and edited on the other merges at its new
  path; `RenameRename` and `RenameDelete` conflicts ([ADR-0005](docs/adr/0005-rename-aware-merge.md)).
- **`ag blame`** (local, `ag remote blame`, and `history.blame` over the API): line-level, follows renames,
  reports truncation honestly ([guide](docs/guides/history.md)).
- `ag verify` also checks that pins, open proposals and workspaces name changesets the object store can produce
  (`dangling_pin`, `dangling_proposal_head`, `dangling_workspace_ref`).

**Clients and CLI**

- **Client SDKs** for TypeScript (`@agentgit/client`) and Python (`agentgit-client`) covering the whole API,
  with idempotency-aware retries, deadlines, pagination, resumable SSE and Ed25519 provenance signing
  ([guide](docs/guides/sdks.md)); CI runs Python 3.10 to 3.13 and Node 20. Not published to npm or PyPI.
- **CLI**: `ag doctor`, richer `ag status`, `ag config`, `ag help TOPIC`, `ag version` (version, commit, date),
  `ag completions` and `ag man`, global `--color/-q/-v`, error hints, JSON errors under `--json`, `ag portability`;
  `--stream` next to the positional stream argument, `--idempotency-key` as an alias of `--key`, bare ids under
  `--quiet`. Stable conflict and blocker codes with `Conflict.message`.
- **Web UI** (`apps/web`, Vue 3 + TypeScript): live board, proposals with four independent verdicts, history with
  blame, agents, administration; types generated from the OpenAPI document.

**Windows and portability**

- Windows builds and passes the test suite and the README demo (PowerShell 7 and 5.1) in CI; checkouts refuse trees
  that cannot be represented on the target (reserved names, NTFS aliases of `.git`, case collisions) before writing
  ([guide](docs/operations/windows.md)). `ag-sandbox` and `ag-worker` are Unix-only.

**Operations**

- **Deployment and observability files** (`deploy/`): Dockerfile (with the Web UI stage), Compose (PostgreSQL +
  SeaweedFS), Kubernetes (kustomize; overlays for CI, high availability and the Prometheus Operator), Prometheus
  rules, a Grafana dashboard, an OpenTelemetry collector config. The image, the Compose stack and a kind deployment
  run end to end in the `Containers` workflow ([deployment](docs/operations/deployment.md),
  [observability](docs/operations/observability.md)).
- **Release pipeline** (`release.yml`, rehearsal mode): archives for Linux and macOS with `ag`, `ag-server`,
  `ag-worker` and `ag-mcp`, multi-arch images, CycloneDX SBOMs, attestations, `scripts/install.sh`, Homebrew and
  Scoop templates, manual npm/PyPI publishing workflow, `cargo publish --dry-run` for the publishable crates. Nothing
  is published ([releasing](docs/operations/releasing.md)).
- **Performance**: `ag-bench` (engine benchmarks at 1k/10k/100k files; a load driver over HTTP/JSON and gRPC against
  a real `ag-server`), `perf.yml`, and the measured numbers with their conditions
  ([engine](docs/architecture/performance.md), [server](docs/operations/performance.md)).

**Quality, security and documentation**

- **Gates**: cargo-deny, npm audit, pip-audit, govulncheck, dependency review (skipped while the dependency graph
  is off), SBOMs and a time-boxed exception file (`security.yml`); CodeQL for Rust, TypeScript, Python and Go;
  Dependabot for seven ecosystems; every action pinned by SHA; 20 libFuzzer targets (`fuzz.yml`); coverage floors per
  crate and mutation testing of `ag-object`, `ag-diff`, `ag-tree`, `ag-merge` and `ag-policy` at 100% of the mutants
  that compile (`quality.yml`); control-database compatibility fixtures written by real old servers.
  See [Testing](docs/architecture/testing.md), [Supply chain](docs/operations/supply-chain.md),
  [Hardening](docs/operations/hardening.md), [Compatibility](docs/architecture/compatibility.md).
- **Documentation site** (MkDocs Material, built with `--strict`; deploys to GitHub Pages only once Pages is
  enabled), five tutorials whose commands are executed in CI, FAQ, glossary, a comparison with Git, GitHub, Gerrit
  and Jujutsu, generated reference pages (help topics, `--json` shapes, completions, man pages), and
  `scripts/check_docs.py` and `scripts/check_no_stubs.py` in CI.

### Changed

- Decoding a cached tree manifest is about 4× faster and a one-file three-way merge about 12× faster at 100 000
  files (change-driven merge; the previous algorithm is kept as the reference and as the fallback). Measurements
  and the conditions for hierarchical trees: [ADR-0006](docs/adr/0006-flat-trees-measured.md).
- `ag_merge::ALGORITHM_VERSION` is `ag-merge/2+ag-diff/1` (recorded in every merge changeset).
- The wire name of a file entry's type is `regular` or `symlink` (it said `file` on reads before the server
  was released); writes still accept `file`.
- **Cargo packages are named `agentgit-<x>`** (directories, library names and binaries are unchanged; `cargo -p`
  flags change). 21 crates are publishable and CI dry-runs `cargo publish --workspace`; the protocol and the migrations
  are packaged inside their crates (`scripts/sync_crate_files.py`). Nothing is published to crates.io
  ([names](docs/operations/releasing.md#cratesio)).
- Container images and Compose/Kubernetes default to the image tag of the workspace version instead of `:latest`;
  Compose uses SeaweedFS instead of MinIO (whose images were withdrawn).
- Object bytes are charged against the repository quota on every path that writes objects, atomically in the control
  database; upload bodies are read only after authentication, rate limiting and authorization, and only up to the
  repository's limit. `ag-server admin` applies the same limits, auth, quota and credential-TTL rules as the server.
- `server.public_url` is removed from the configuration. Repository `create`, `describe` and `list` report the real
  storage backends and real stream and changeset counts; `negotiate` advertises the engine's own limits.
- `integration.prepare` reads the policy verdict, attestations and validation jobs in one read transaction.
- The README (English and Russian) is redesigned: a banner and an architecture diagram generated by
  `scripts/gen_readme_assets.py` (checked in CI), the real Web UI screenshots, the four verdicts and their
  triggers, and the honest limits as a table that is rewritten from verified facts.

### Fixed

- **Security**: control-plane rows (credentials, scopes, repositories, role bindings) that cannot be parsed fail
  closed instead of widening access; every role binding reaches the authorizer (the silent 5000-binding cap is
  gone); the SSE stream ends when its credential is revoked or expires, and the SSE and replay feeds no longer skip
  events behind a topic filter; malformed query parameters are rejected instead of ignored; `ag-server` creates its
  data directory with mode 0700.
- A three-way merge whose result exceeds the tree entry limit is an error instead of a "clean" merge that dropped
  the other side's changes.
- An identity name may no longer start with `#`, so the principal of any identity is itself an identity
  ([ADR-0009](docs/adr/0009-identity-names-and-principals.md)); `Timestamp::now()` reports a pre-1970 clock as
  negative instead of 0.
- Storage: the filesystem store removes its staging file on every failed write and reclaims orphans of killed
  writers; the PostgreSQL object lock ends its session when the server refuses an unlock; an S3 listing entry
  without a size is an error; a PostgreSQL that refuses a connection yields a retryable `unavailable`, not
  `internal`; a control database written by a newer server is refused at start-up with both versions named.
- Idempotency records for `run.start`, `task.create` and `lease.acquire` are written in the creating transaction, so
  a lost race replays the winner's answer; expired job leases are reaped and audited; `agent.list_runs` with
  `active_only` filters before the limit.
- `blame` built the whole tree of every visited changeset, which was quadratic over a chain of unintegrated
  changesets (10.6 s to 168 ms at 100 000 files).
- Found by fuzzing and mutation testing, each with a regression test: overflows in `ag-diff` and `ag-policy`
  (including a `max_age` wrap), a panic and a date overflow in `ag-git`, an exported path starting with `"`, two
  quadratically backtracking regular expressions in the TypeScript SDK, and a trusted signed pass for another check
  being accepted.
- Web UI: a scroll region follows its content (the flaky `scrollable-region-focusable` axe failure); a failed
  diff-base lookup is an error, not an empty tree; a job without a verdict is not shown as passed; `Approved` is
  "checking" until the policy check answers.
- Orchestrator: backoff jitter is random, the retry budget survives restarts, conflict-prediction failures are
  visible, role globs match the server's, and credentials are never echoed.
- `scripts/e2e_observability.py` waits for Prometheus' first rule evaluation instead of racing it.

### Known limitations

- This is the first tag: before it the publishing half of the release workflow (GHCR push, GitHub release,
  attestation of the published files) had only been rehearsed from a work branch. No npm, PyPI or crates.io package is
  published, and GitHub Pages is not enabled.
- Replicas: mixed builds during a rolling upgrade, a PostgreSQL failover, load, and the `ha` Kubernetes overlay on a
  cluster are not tested. Rate limits are per replica; `remote` in logs is the proxy's address (no `X-Forwarded-For`).
- Open-stream and open-proposal counts are checked just before the insert and can overshoot by the number of
  requests in flight; idempotency records have no TTL; there is no global budget for request bytes in flight.
- A credential minted without `scope.run_id` is still the agent for every run, and a writer can start, heartbeat
  or finish another agent's run (attributed and audited as the operator when it is not the agent's own, not prevented).
- The sandboxes share the host kernel; they are not virtual machines. microVM backends, Windows, macOS beyond a
  limits-only fallback, runtimes other than Docker 28.0.4 and Podman 4.9.3, images other than BusyBox and SELinux
  enforcing hosts are not verified. Memory and process limits are sampled unless a cgroup is delegated.
- Storage is tested against SeaweedFS and moto, not against Amazon S3, MinIO, Ceph or R2. There is no background
  outbox dispatcher (derivation runs inline after each commit; failures are recorded but not yet shown by `ag doctor`
  or metrics); the integration queue carries no fencing tokens.
- Write throughput on the default SQLite metadata is bound by fsync (about 80 integrations/s across 16 streams on the
  measurement VM) and same-stream contention yields retryable `aborted` answers (11-14% at 16 writers); no PostgreSQL,
  S3, TLS or multi-replica measurements.
- Windows: no sandbox or worker, no release archive, owner-only ACLs on key files are set but not read back.
- The Web UI has no screen-reader or real-device pass; OIDC was verified against Keycloak only. `ag-mcp` is stdio only
  and was not run inside Claude Code, Claude Desktop or the Agent SDKs; `--ca-file` replaces the built-in roots. The
  model-backed orchestrator was run live once with one model through one provider.
- The nightly fuzz job and the weekly mutation run have never run on GitHub; the dependency graph is off. No external
  security audit.
- Rename detection is per file (no directory renames or copies); blame follows renames but not copies, and
  for merge parents beyond the first only the same path.


## [0.1.0] — 2026-10-17

First public alpha.

### Added

- **Deterministic core**: SHA-256 domain-separated content IDs, strict canonical binary encoding,
  `ChangesetV1`, derived trees with delta/first-parent semantics, DAG algorithms (ancestors, merge bases),
  Myers diff, deterministic three-way merge for text and trees with typed conflicts. Formats verified
  against an independent Python implementation (`tests/conformance/`).
- **Storage**: filesystem object store (fsync + atomic rename, verified on read, self-healing, GC-safe
  deletes) and a SQLite metadata store (WAL, `synchronous=FULL`) with changeset index, streams + reflog,
  intents, transactional outbox, hash-chained audit log, policies, proposals, reviews, attestations, pins,
  workspaces and agent state.
- **Engine** (`ag-integrate`): verified publication; integration with candidate computation outside the
  lock and policy + compare-and-swap inside one transaction; idempotent, resumable operations with fault
  injection at every step; integration queue; garbage collection that fails closed and cannot lose a
  concurrently re-referenced object; `ag verify`; rebuildable, sparse (keyframe) derived tree cache;
  consistent verified backups.
- **Policy**: versioned per-stream JSON policy; trusted issuers; signed attestations bound to tree, policy
  version and environment; approvals bound to the reviewed head; protected paths; agent autonomy envelope;
  provenance requirements.
- **Agent-native**: agent identities and runs, tasks with dependencies, soft and exclusive leases with
  fencing tokens, signed provenance stored beside immutable changesets, path-level conflict prediction.
- **Workspaces**: isolated directories, snapshots, sync with conflict markers and resolution.
- **Git bridge**: import and export through git plumbing; trees round-trip with identical SHAs.
- **CLI** `ag` with stable exit codes (0 ok, 1 error, 2 usage, 3 blocked, 4 refused) and `--json`.
- **Docs and tooling**: architecture, guides, operations, generated CLI reference, ADR traceability,
  documentation consistency checks, runnable end-to-end demo, CI.

### Known limitations

Local mode trusts the local user; no sandbox for `ag validate run`; flat tree manifests; no rename
detection; no network server. See [ROADMAP.md](ROADMAP.md) and
[docs/architecture/security.md](docs/architecture/security.md).

[Unreleased]: https://github.com/Paffin/AgentGit/compare/v0.3.0-beta.3...HEAD
[0.3.0-beta.3]: https://github.com/Paffin/AgentGit/releases/tag/v0.3.0-beta.3
[0.3.0-beta.2]: https://github.com/Paffin/AgentGit/releases/tag/v0.3.0-beta.2
[0.2.0-rc.1]: https://github.com/Paffin/AgentGit/releases/tag/v0.2.0-rc.1
[0.1.0]: https://github.com/Paffin/AgentGit/releases/tag/v0.1.0
