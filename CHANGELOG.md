# Changelog

All notable changes are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres to
[Semantic Versioning](https://semver.org/) — while `0.x`, minor versions may change behaviour, but
on-disk formats are versioned and refuse to open when newer than the build understands.

## [Unreleased]

## [0.2.0-rc.2] — 2026-10-19

Second release candidate. `v0.2.0-rc.1` was tagged but its Release run failed in the last stage before anything
was attached to a GitHub release (the release notes were requested with a bare version instead of a tag name); no
archive was published for it. Code is otherwise identical to rc.1.

### Fixed

- Release workflow: the release notes step passes the tag form (`vX.Y.Z`), and an already existing release page
  (the GitHub UI creates one with a tag) is filled in instead of making `gh release create` fail.

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

[Unreleased]: https://github.com/Paffin/AgentGit/compare/v0.2.0-rc.1...HEAD
[0.2.0-rc.1]: https://github.com/Paffin/AgentGit/releases/tag/v0.2.0-rc.1
[0.1.0]: https://github.com/Paffin/AgentGit/releases/tag/v0.1.0
