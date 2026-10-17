# Track briefs (relaunch prompts)

Each track is one agent working in its own git worktree. To relaunch one after a reset, start an agent with:
"FIRST read `dev/agent-brief.md` completely and follow it. YOUR TRACK: …" followed by the track text below. Before
relaunching, read `dev/STATUS.md` and the track's `dev/handoff/<track>.md` (state, what is verified, next steps) and
check the remote branch `wip/<track>-track` (unlanded work). Skip what already landed on `main`.

All tracks share the rules in `dev/agent-brief.md`: claims are verified, no mocks/stubs/hardcodes in product code,
land small slices on `main` (rebase first, gates green), never tag/release/publish/force-push.

## release — Release & distribution
Prove "install it and run it" in real GitHub Actions (there is no Docker daemon in the dev container).
1. `.github/workflows/containers.yml`: build `deploy/docker/Dockerfile` (small, non-root, HEALTHCHECK, buildx cache) and
   run it; `docker compose up` of `deploy/compose` (server + PostgreSQL + MinIO + observability) with a real end-to-end
   smoke (create-repo, grant, mint, `ag clone`, snapshot, `ag push`, integrate via the server, read back via HTTP, assert
   metrics); kind cluster job applying `deploy/kubernetes` with the same smoke. Logs as artifacts on failure. Fix every
   defect found (manifests, Dockerfile, config, docs). Known defect from the audit: every default points at the `latest`
   image tag but nothing publishes it — make the default resolvable (see `dev/STATUS.md`).
2. `release.yml` with `workflow_dispatch` (`version`, `publish` default false): verify → test → binaries (linux
   x86_64/aarch64, macOS arm64/x86_64, Windows when green) → archives (`ag`, `ag-server`, `ag-worker`, `ag-mcp`,
   completions/man, LICENSE, NOTICE) → SHA256SUMS → build-provenance attestations → CycloneDX SBOM → multi-arch image (no
   push unless `publish`) → release notes from CHANGELOG. `scripts/check_release.py` tolerates the dry-run version.
   Exercise via workflow_dispatch; update `docs/operations/releasing.md` (what dry-run proves; what stays unproven).
3. `scripts/install.sh` (POSIX sh, checksum verification, `--version/--prefix/--dry-run`, shellcheck in CI) + install
   smoke against the dry-run artifacts; `packaging/homebrew/agentgit.rb` and `packaging/scoop/agentgit.json` templates +
   a fill script tested on a fixture.
4. Publish readiness (inert): crates.io metadata for publishable crates (`publish = false` for internal), CI dry-run of
   `cargo package`/`publish --dry-run`, npm/PyPI publish workflows that only publish on manual dispatch with `publish:
   true` (trusted publishing), otherwise build + `npm pack` / `python -m build` + `twine check`.
5. `--version` of `ag`/`ag-server` prints semver + git commit + build date; a test.

## ha — Server high availability
Control DB (credentials, roles, repo registry, agent keys, job queue…) is one SQLite file → single replica. Write
`docs/adr/0007-multi-replica-server.md` first (inventory of all server-side state, invariants), then: a control-plane
store trait with SQLite (default) and PostgreSQL implementations + versioned migrations + advisory-locked startup
migration + schema-compat check; cross-replica correctness (job claim with SKIP LOCKED/lease, SSE fan-out via
LISTEN/NOTIFY or outbox polling with replay by id, credential/role cache invalidation, idempotency across replicas,
documented per-replica rate limits); tests with 2–3 real `ag-server` processes on one PostgreSQL + S3-compatible store
(no lost update across replicas, kill -9 mid-flight, exactly-once job claim, SSE across replicas, bounded revocation
visibility, rolling restart) wired into the CI `backends` job; readiness/liveness reflecting control-DB reachability,
graceful drain, replica/schema/event-lag metrics, `deploy/kubernetes/overlays/ha`, `docs/operations/high-availability.md`;
`ag-server admin backup-control/restore-control` and `migrate-control --from sqlite --to postgres` with a data test.

## web — Web UI to product level
(1) `ag-server` embeds and serves the built UI (feature `ui`, `[ui] dir`/`enabled`, SPA fallback that never shadows
`/v1`/gRPC/`/metrics`/`/healthz`, immutable caching, strict CSP, compression; "UI not built" page; tests + e2e against
the server itself). (2) Responsive: tablet 768 / phone 390 Playwright projects with real assertions; fix breakage.
(3) Accessibility: axe-core on every route in light/dark, fix serious/critical, keyboard-only flows, reduced motion.
(4) Cross-browser: chromium/firefox/webkit matrix in `web.yml` (browsers installed in CI only). (5) OIDC authorization
code + PKCE login (public `GET /v1/auth/config`, discovery, S256, state/nonce, in-memory token, logout) tested against a
fake IdP in e2e; paste-a-token flow stays. (6) First-run onboarding + error pages. Docs: `docs/guides/web-ui.md`.

## mcp — MCP server for agents
`ag-mcp` crate/binary: stdio (and optionally streamable HTTP) MCP server over AGWP with an operator-supplied scoped
credential; allow-list groups (read, workspace, propose, review, coordinate; `integrate`/`admin` off by default); tool
results are data (sanitised, bounded, delimited). Tools: repo/stream info, log, show, blame, read_file (+resources),
diff, predict_conflicts, task and lease operations, workspace write/snapshot/propose, proposal_status (four verdicts +
blockers), request_validation, review (if allowed), queue status. `docs/adr/0008-mcp-server.md`; conformance tests that
spawn a real `ag-server` and a real `ag-mcp` over stdio and drive full agent stories incl. refusals, expiry, truncation,
hostile content; `docs/guides/mcp.md` (Claude Code / Desktop / Agent SDK config; say what was not exercised with a live
model); `examples/agents/` scripted client run in CI.

## security — Security, supply chain, quality gates
`deny.toml` + CI; RustSec/OSV/npm/pip/govulncheck audits; dependency-review; CodeQL; SHA-pinned actions + extended
Dependabot; least-privilege `permissions:` on every workflow; `THIRD_PARTY_LICENSES` with freshness check;
`scripts/sbom.sh`. Fuzzing: `fuzz/` cargo-fuzz targets for every untrusted-bytes parser (object envelope, changeset,
tree manifest, attestation/provenance/policy JSON, AGWP bodies/protobuf, path normalisation, merge/diff invariants,
rename detection, glob matching, git-bridge parsing, job spec) with corpora; run locally ≥5 min on the important ones;
fix every crash with a regression test; document in `docs/architecture/security.md`; `fuzz.yml` smoke on PRs + nightly.
Coverage (llvm-cov, per-crate numbers in `docs/architecture/testing.md`, modest floor on core); cargo-mutants on
ag-merge/ag-tree/ag-diff/ag-policy/ag-object (triage survivors). `tests/compat/` golden fixtures (repository + control DB)
+ `docs/architecture/compatibility.md`. Reconcile `SECURITY.md`; operator hardening checklist verified against code.

## cli-docs — CLI polish + docs site
CLI: `ag completions`, man pages, `ag doctor`, `ag status`, `--json` on every read command with golden-tested schemas
(`docs/reference/cli-json.md`), documented exit codes, NO_COLOR/`--color`/`--quiet`/`--verbose`, helpful errors, `ag
config`, `ag help <topic>`. Docs site (mdBook or MkDocs — decide, justify), strict build in `docs.yml`, Pages deploy only
if enabled (owner must enable Settings → Pages → GitHub Actions). New docs: `docs/tutorials/` (first agent-safe repo,
team server, policy cookbook, migrating a Git repo, troubleshooting a blocked integration — command snippets executed in
CI), `docs/faq.md`, `docs/glossary.md`, `docs/comparison.md` (honest vs Git, GitHub PRs/merge queue, Gerrit, Jujutsu).

## windows — Windows & portability
Compile and test the portable crates on `x86_64-pc-windows-msvc` in `.github/workflows/windows.yml`; cfg-gate unix-only
parts (sandbox/worker stay Linux-only with a clear error); path normalisation and Windows-specific checkout-escape
defences (reserved names, trailing dots, ADS, 8.3 names, UNC, drive prefixes), case-insensitive collisions (also macOS),
CRLF = bytes are bytes, symlink/exec-bit policy, locking/atomic replace, long paths, config dirs, Ctrl+C shutdown;
`examples/quickstart.ps1` run in the Windows job; `docs/operations/windows.md` (support matrix, ignored tests with
reasons).

## Not yet started (next wave — see dev/STATUS.md)
hardening (run-bound credentials, delegator role, server-side finish/reap, prepare read-consistency, repo counts),
execution-plane proof (OCI backend vs real docker/podman, cgroup v2, macOS), model-backed advisors/reviewers (verify
live through OpenRouter; no keys in code), benchmarks/load tests, and the fixes from `dev/audit/`.
