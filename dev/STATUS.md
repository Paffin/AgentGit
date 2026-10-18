# Project status and handoff

Maintainer working notes — not user documentation. Snapshot written 2026-10-17 (session ended its 5-hour usage window
shortly after). Everything durable is in git: this file, `dev/agent-brief.md`, `dev/tracks/tracks.md`,
`dev/handoff/*.md`, `dev/audit/`, `dev/workflows/`. If a session resumes from here, read this file first.

## Owner's directives (verbatim intent)
1. "Merge everything into main, delete the branches" — done: all work is on `main`; local branches/worktrees removed.
   Remote leftovers to delete (the git proxy refused deleting the session's own branch; agents created WIP branches):
   `claude/quirky-faraday-xr6hiu` (fully merged), `wip/*`, `web-product-ci`. Delete them on GitHub once their content is
   confirmed on `main`.
2. "Bring the project to a product state from all sides" and then "to 200% readiness **without mocks / stubs /
   hardcodes**". The shared brief (`dev/agent-brief.md`) encodes this; the audit (`dev/audit/`) lists what is left.
3. Work directly on `main` is authorised. Never: force-push, tag, release, publish to registries, open PRs.

## State of `main` (all CI-gated: fmt+clippy+rustdoc, test ubuntu+macOS, MSRV 1.89, docs+demo, PG+S3 backends, Web UI)
Delivered before this phase (tasks #1–#13 all done): deterministic core, engine, agent-native features, CLI, docs, server
(AGWP v1 gRPC+HTTP/JSON+SSE), PostgreSQL+S3, execution plane (`ag-worker`/`ag-sandbox`), Web UI, SDKs (TS/Python), Go
orchestrator, deploy files, observability, README redesign (EN+RU, generated SVG artwork, real UI screenshots).
Verified at last full run: 76 Rust suites / 554 tests green; clippy clean; TS SDK 156, Python SDK 176, Web 309 unit +
e2e, Go suite green.

## 2026-10-18 update (session resumed after the usage limit reset)
* **`main` was red and is GREEN again** (verified in Actions at `938f744`): CI 9/9 jobs, Windows (green twice in a row),
  Containers (image + UI stage, compose with PostgreSQL+SeaweedFS incl. observability, kind), Packaging (11 crates
  dry-run), Web UI. Root causes fixed: rustc 1.89 type inference (`&String` vs `&str` array), ambiguous rustdoc link,
  non-portable macOS allocator test, stale generated SDK clients after `/v1/auth/config`, a real ordering race in the Go
  advisor, Dockerfile not building the embedded UI, `ag doctor` not compiling on Windows + 5 host-assumption tests.
  Open from that round: a flaky Web UI axe race (`scrollable-region-focusable`, dark theme) — in wave 2, web_tail.
  Rule: no landing without watching its CI.
* **Wave 2 is running** (`dev/workflows/product-wave-2.js`, 3 sequential lanes, cap 2 concurrent agents): A server
  (server_audit → server_hardening → ha_tail → perf), B core (core_engine_storage → policy_cli_execution → clients_ops →
  orchestrator incl. model-backed advisors verified live via OpenRouter), C docs/infra (docs_site → security_tail → mcp_tail
  → web_tail incl. real-IdP e2e). Then a CI verifier (+generic fix rounds) and a docs-fold agent that rewrites README/
  CHANGELOG/ROADMAP/traceability from the handoffs. If the session dies: re-run it with
  `Workflow({scriptPath: "dev/workflows/product-wave-2.js"})` after checking which `dev/handoff/<wp>.md` files exist
  (done packages are the ones with a handoff and green CI; remove them from the lane lists first).
* Audit finished (see the Audit section).

## Product-readiness tracks (7 parallel agents; briefs in `dev/tracks/tracks.md`)
| # | Track | Owner files | State at snapshot |
|---|---|---|---|
| 14 | release & distribution | release.yml, containers.yml, deploy/, scripts/install.sh, packaging/ | **landed on main (6a87d70)**; handoff `dev/handoff/release.md`. Proven in CI: image, layer cache, kind, compose up+smoke+restart (observability step still `continue-on-error`), packaging dry-runs. **Release rehearsal finished green end to end** (run 39076286737, from `wip/release-track` commit `2b7dce2`; macOS x86_64 was just slow): tests, SBOMs, 4 binaries, both image arches, multi-arch manifest, release assembly, `install.sh` on ubuntu, ubuntu-arm and macOS; nothing published. A re-run from `main` (Actions UI — the session token gets 403 on dispatch) is now only a confirmation. Main at `570e54b` |
| 15 | server HA (control plane on PostgreSQL) | ag-server, ag-storage | **part 1 landed on main** (`e5a8146`): ADR-0007 (state inventory, invariants — these are design goals until the multi-replica tests pass), `ag_storage::sqldb`, control DB on SQLite (default) or PostgreSQL (`[control] url|url_env`) with versioned migrations in `migrations/control/`, advisory-locked startup migration, newer-schema refusal; repo registry stores per-repo config so another replica can open it; LISTEN/NOTIFY wakes SSE/long-poll/job claimers and drops role caches (polling stays the floor); `/readyz` reflects control DB + draining; `admin backup-control/restore-control/migrate-control`; transient DB errors → `unavailable`/`deadline_exceeded`. Passes `scripts/pg-test.sh cargo test -p ag-storage -p ag-server`. **Not on main**: `crates/ag-server/tests/multi_replica.rs` (9 real-process tests) + `scripts/ha-test.sh` on branch `wip/ha-track-rebased` — compile, never run to green. **Missing**: `docs/operations/high-availability.md`, deployment/traceability/security doc updates, `overlays/ha`, Compose profile, CI wiring, CLI migrate-control test. CI after the push not checked. Handoff `dev/handoff/ha.md` |
| 16 | web product | apps/web, web.yml, ag-ui-assets | **landed on main** (`7335ccd`): `ag-server` serves the UI itself (new crate `crates/ag-ui-assets`, `src/ui.rs`, default-on feature `ui`, `[ui] enabled/dir`, SPA fallback that never shadows API paths, immutable hashed assets, br/gz, CSP without inline script, "UI not built" page, `scripts/build_ui.sh`); public `GET /v1/auth/config`; OIDC code+PKCE login (`[[ui.login]]`, token in memory) with a fake IdP e2e (13 tests); responsive 768/390 projects + axe fixes; empty stream log is `[]` not 404. Evidence: clippy/fmt, ui tests 13+10, npm unit 347, local Chromium 158/159→fixed, tablet+phone 146/146, first CI slice green on `web-product-ci`. **Open**: `web.yml` on main not yet seen green after the merge; Firefox/WebKit not green (left out of main's matrix; full 5-project matrix on branch `web-product-ci`); onboarding panel, version-mismatch banner, 401 page not started; no real-IdP test, no screen-reader/real-phone pass. Release agent: Dockerfile must run `scripts/build_ui.sh` in a Node 22 stage and copy `apps/web/dist` before `cargo build --release -p ag-server`. Handoff `dev/handoff/web.md` |
| 17 | MCP server `ag-mcp` | crates/ag-mcp, docs/guides/mcp.md | **core landed on main** (`c396e23`): hand-written protocol layer (no rmcp: 89 extra crates), stdio only, MCP 2026-07-28 + 2025 handshakes, 31 tools in 6 groups (`integrate` only with `--enable-integrate`, no admin), resources, prompts, sanitised/bounded results; 54 unit + 14 end-to-end tests against a real in-process `ag-server` pass locally; ADR-0008. **Not done**: `docs/guides/mcp.md` (ADR links removed meanwhile), `tests/protocol.rs`, `tests/cli.rs`, `examples/agents/` + CI job; CI for the push not checked; not exercised with any live model/host; add `ag-mcp` to the release artefacts (release agent). Handoff `dev/handoff/mcp.md` |
| 18 | security & quality gates | fuzz/, deny.toml, security workflows | **partly landed** (checkpoint). On main: 4 bug fixes found by fuzzing/review, each with a regression test shown failing on the old code (ag-diff overflow, ag-policy overflow + max_age wrap, ag-git truncate panic / date overflow / export of `"`-prefixed paths); orchestrator image on Go 1.27 (govulncheck: 36 reachable stdlib advisories on 1.24); 20 libFuzzer targets + seeds; `tests/compat` repo fixture + 7 tests; `deny.toml` (passes locally). **Not landed**: `security.yml` (on `wip/security-track`, never run, references missing scripts), control-DB compat fixture (regenerate from a main build; the one made was schema v2). **Not done**: `fuzz.yml`, coverage/mutation workflow, CodeQL, dependabot extension, `sbom.sh`, THIRD_PARTY_LICENSES, all docs (security.md Fuzzing, testing.md, compatibility.md, hardening checklist, SECURITY.md); fuzzing ran only 15–45 s per target (asked ≥5 min on key ones); `signed_json` harness has a `split_at` bug. CI for the last commit was still queued. Handoff `dev/handoff/security.md` |
| 19 | CLI UX + docs site | ag-cli, docs/ | **CLI half landed on main** (`f0cf3d4`, `befa4dc`, `c5f040d`): `ag doctor`, richer `ag status` (+ JSON `summary`), `ag config`, `ag help TOPIC` (8 topics), `ag version`, `ag completions`, `ag man`, global `--color/-q/-v`, error hints, JSON errors under `--json`, quiet closed-pipe; tests `ux`, `doctor_status`, `exit_codes`, `json_shapes(_remote)` (golden shapes + a guard that fails when a command has none), `completions` (bash verified; zsh/fish/pwsh/elvish skipped locally — set `AG_REQUIRE_SHELLS=1` in a CI job that installs them). CI for the last commit unconfirmed. **Docs half not started**: MkDocs Material site (chosen over mdBook: strict link/anchor checks, offline search), `docs.yml` + Pages deploy, 5 tutorials, FAQ, glossary, comparison, snippet-test harness, policy-cookbook parser test, generated reference pages. Known defects left (would break scripts): positional vs `--stream` inconsistency; `integrate --key` vs `validate run --key` differ; not-found/conflict errors exit 1 not 4. Handoff `dev/handoff/cli-docs.md` |
| 20 | Windows & portability | cfg-gating, windows.yml | **code+docs landed on main** (`4c30244`…`b3eb6b1`, also `wip/windows-track`); handoff `dev/handoff/windows.md`. Windows CI **not yet green**: first run built the whole workspace, only 10 test targets failed (fixed in `ef20d10`, unconfirmed); console (Ctrl+C), ACL tests and `quickstart.ps1` never ran on a Windows runner. Do not make it a required check until green twice in a row. Open: NASM / VC++ redistributable (`+crt-static`) question for the release binary |
Each agent was told (at snapshot time) to commit, land what passes the gates on `main`, push the rest to
`wip/<track>-track`, and write `dev/handoff/<track>.md`. **Check those files and branches first** — they are the
authoritative per-track state.

## Audit: no mocks / stubs / hardcodes (`dev/audit/`) — COMPLETE
Read-only workflow (`dev/workflows/audit-no-stubs-mocks-hardcodes.js`: 13 area scanners, 2 adversarial verifiers per
area, coverage critic). Result: 389 candidates → **93 confirmed** (both verifiers: 6 high, 58 medium, 29 low; 87 on
production paths), **105 contested** (one verifier agreed — triage individually), 191 refuted. The critic reports 65
source files no scanner read (list in the JSON). Files: `dev/audit/CONFIRMED.md` (the work list, with the verifier-refined
fix for each), `dev/audit/audit-final.json` (everything incl. contested + reasons).
Confirmed HIGH items (fix each with a test): `ag-merge/src/tree.rs:304` merge_full swallows `TooManyEntries`;
`ag-server/src/control.rs:218` unparsable credential scope becomes UNRESTRICTED (fail closed); `ag-server/src/api/objects.rs:125`
object quota not enforced on virtual-workspace writes; `ag-server/src/api/events.rs:137` SSE topic filter after a 1024-row
window skips events permanently; `services/agent-orchestrator/internal/session/scripted.go` simulated agent ships in the
production binary. Earlier candidates still worth verifying from the first pass: SSE stream does not re-check revocation
(`events.rs:119`); OCI sandbox tests use a canned shell runtime (`ag-sandbox/tests/oci.rs`); orchestrator tests run against an
in-memory `fakeagwp` instead of the real server; S3 tests use moto (add MinIO/SeaweedFS); `deploy/kubernetes` default image
tag `latest` is never published.
Also known: the orchestrator's advisor/reviewers/semantic index are deterministic heuristics → model-backed
implementations behind the existing interfaces; **no API keys in code** — OpenRouter was reachable with proxy-injected
authorization in the session, so an OpenAI-compatible provider can be verified live.

## Next wave (after the tracks land)
1. Fold agent reports into `README.md`, `README.ru.md`, `CHANGELOG.md`, `ROADMAP.md`, `docs/architecture/adr-traceability.md`
   (agents were told not to edit these; each final report has "Changelog entries" and "README/ROADMAP notes").
2. Fix audit findings (above + verified mediums). 3. Server hardening: run-bound credentials, delegator role,
   server-side finish/reap of tasks/runs, `integration.prepare` read-consistency, repo list counts.
4. Execution-plane proof: OCI backend vs real runtimes, cgroup v2, macOS, per-job logs. 5. Model-backed advisors/reviewers.
6. Benchmarks and load tests with published numbers. 7. Prove the deployment story end to end (a real `-rc` through the
   release workflow — needs the owner's go-ahead because tagging publishes). 8. Make windows/containers/docs jobs merge
   gates once stable. 9. Final pass: all workflows green on `main`, README/limits rewritten honestly, run
   `scripts/check_docs.py`, `scripts/gen_cli_docs.py --check`, `scripts/gen_readme_assets.py --check`.

## Owner action items
* **Confirm the release rehearsal from `main`** (Actions UI: Release → Run workflow, version `0.0.0-rc.dry`, publish off).
  It already passed end to end from a wip branch; the session's GitHub token gets `403` on `run_workflow`/cancel, so
  agents cannot dispatch it. (Optional: cross-compile macOS x86_64 on macos-14 — the intel runner is slow.)
* crates.io: names `ag-agent`, `ag-git`, `ag-protocol`, `ag-worker` are taken by others; two crates read files outside
  their crate dir — 9 crates are `publish = false` for now. Decide on a naming scheme (e.g. `agentgit-*`) before any
  crates.io publish.
* Enable GitHub Pages (Settings → Pages → Source: GitHub Actions) so the docs site deploys.
* Delete the leftover remote branches listed above.
* Decide when to cut the first release tag (publishing images/crates/SDKs is outward-facing; nothing is published yet).

## Dev-environment notes (ephemeral container)
* **Shared target dir hazard**: concurrent agents overwrite each other's workspace-crate artifacts in `/home/user/ag-target`
  and the disk hit 0 bytes once. Next wave: run fewer concurrent Rust agents, or give each its own `CARGO_TARGET_DIR` and
  `CARGO_PROFILE_DEV_DEBUG=0`, and clean stale test executables regularly.
* Shared cargo target dir: `~/.cargo/config.toml` → `build.target-dir = "/home/user/ag-target"`, `incremental = false`.
  The disk is small (~250 GB total but ~25 GB usable); test executables in `ag-target/debug/deps` are the bulk — delete
  stale ones (`find … -perm -u+x -mmin +45 -delete`), never `cargo clean` while agents run.
* No Docker daemon in the container; container/k8s/Windows proof must come from GitHub Actions (use
  `mcp__github__actions_*`, keep outputs small — list with `per_page` 1–3).
* Headless Chromium at `/opt/pw-browsers`; helper binaries (kustomize, kubeconform, promtool) were downloaded into the
  session scratchpad and are not durable.
* Commit trailers: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_01F8AFjCQ5Q47gBZQNqYFtGQ`.
