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

## Product-readiness tracks (7 parallel agents; briefs in `dev/tracks/tracks.md`)
| # | Track | Owner files | State at snapshot |
|---|---|---|---|
| 14 | release & distribution | release.yml, containers.yml, deploy/, scripts/install.sh, packaging/ | **landed on main (6a87d70)**; handoff `dev/handoff/release.md`. Proven in CI: image, layer cache, kind, compose up+smoke+restart (observability step still `continue-on-error`), packaging dry-runs. Release rehearsal (`workflow_dispatch`) partly green (3/4 binaries, both image arches); `x86_64-apple-darwin` on `macos-15-intel` hung >25 min so release assembly/attestation/install jobs never ran |
| 15 | server HA (control plane on PostgreSQL) | ag-server, ag-storage | **part 1 landed on main** (`e5a8146`): ADR-0007 (state inventory, invariants — these are design goals until the multi-replica tests pass), `ag_storage::sqldb`, control DB on SQLite (default) or PostgreSQL (`[control] url|url_env`) with versioned migrations in `migrations/control/`, advisory-locked startup migration, newer-schema refusal; repo registry stores per-repo config so another replica can open it; LISTEN/NOTIFY wakes SSE/long-poll/job claimers and drops role caches (polling stays the floor); `/readyz` reflects control DB + draining; `admin backup-control/restore-control/migrate-control`; transient DB errors → `unavailable`/`deadline_exceeded`. Passes `scripts/pg-test.sh cargo test -p ag-storage -p ag-server`. **Not on main**: `crates/ag-server/tests/multi_replica.rs` (9 real-process tests) + `scripts/ha-test.sh` on branch `wip/ha-track-rebased` — compile, never run to green. **Missing**: `docs/operations/high-availability.md`, deployment/traceability/security doc updates, `overlays/ha`, Compose profile, CI wiring, CLI migrate-control test. CI after the push not checked. Handoff `dev/handoff/ha.md` |
| 16 | web product (embed UI, responsive, a11y, cross-browser, OIDC PKCE) | apps/web, web.yml | in progress; branch `web-product-ci` |
| 17 | MCP server `ag-mcp` | crates/ag-mcp, docs/guides/mcp.md | **core landed on main** (`c396e23`): hand-written protocol layer (no rmcp: 89 extra crates), stdio only, MCP 2026-07-28 + 2025 handshakes, 31 tools in 6 groups (`integrate` only with `--enable-integrate`, no admin), resources, prompts, sanitised/bounded results; 54 unit + 14 end-to-end tests against a real in-process `ag-server` pass locally; ADR-0008. **Not done**: `docs/guides/mcp.md` (ADR links removed meanwhile), `tests/protocol.rs`, `tests/cli.rs`, `examples/agents/` + CI job; CI for the push not checked; not exercised with any live model/host; add `ag-mcp` to the release artefacts (release agent). Handoff `dev/handoff/mcp.md` |
| 18 | security & quality gates | fuzz/, deny.toml, security workflows | **partly landed** (checkpoint). On main: 4 bug fixes found by fuzzing/review, each with a regression test shown failing on the old code (ag-diff overflow, ag-policy overflow + max_age wrap, ag-git truncate panic / date overflow / export of `"`-prefixed paths); orchestrator image on Go 1.27 (govulncheck: 36 reachable stdlib advisories on 1.24); 20 libFuzzer targets + seeds; `tests/compat` repo fixture + 7 tests; `deny.toml` (passes locally). **Not landed**: `security.yml` (on `wip/security-track`, never run, references missing scripts), control-DB compat fixture (regenerate from a main build; the one made was schema v2). **Not done**: `fuzz.yml`, coverage/mutation workflow, CodeQL, dependabot extension, `sbom.sh`, THIRD_PARTY_LICENSES, all docs (security.md Fuzzing, testing.md, compatibility.md, hardening checklist, SECURITY.md); fuzzing ran only 15–45 s per target (asked ≥5 min on key ones); `signed_json` harness has a `split_at` bug. CI for the last commit was still queued. Handoff `dev/handoff/security.md` |
| 19 | CLI UX + docs site | ag-cli, docs/ | **CLI half landed on main** (`f0cf3d4`, `befa4dc`, `c5f040d`): `ag doctor`, richer `ag status` (+ JSON `summary`), `ag config`, `ag help TOPIC` (8 topics), `ag version`, `ag completions`, `ag man`, global `--color/-q/-v`, error hints, JSON errors under `--json`, quiet closed-pipe; tests `ux`, `doctor_status`, `exit_codes`, `json_shapes(_remote)` (golden shapes + a guard that fails when a command has none), `completions` (bash verified; zsh/fish/pwsh/elvish skipped locally — set `AG_REQUIRE_SHELLS=1` in a CI job that installs them). CI for the last commit unconfirmed. **Docs half not started**: MkDocs Material site (chosen over mdBook: strict link/anchor checks, offline search), `docs.yml` + Pages deploy, 5 tutorials, FAQ, glossary, comparison, snippet-test harness, policy-cookbook parser test, generated reference pages. Known defects left (would break scripts): positional vs `--stream` inconsistency; `integrate --key` vs `validate run --key` differ; not-found/conflict errors exit 1 not 4. Handoff `dev/handoff/cli-docs.md` |
| 20 | Windows & portability | cfg-gating, windows.yml | **code+docs landed on main** (`4c30244`…`b3eb6b1`, also `wip/windows-track`); handoff `dev/handoff/windows.md`. Windows CI **not yet green**: first run built the whole workspace, only 10 test targets failed (fixed in `ef20d10`, unconfirmed); console (Ctrl+C), ACL tests and `quickstart.ps1` never ran on a Windows runner. Do not make it a required check until green twice in a row. Open: NASM / VC++ redistributable (`+crt-static`) question for the release binary |
Each agent was told (at snapshot time) to commit, land what passes the gates on `main`, push the rest to
`wip/<track>-track`, and write `dev/handoff/<track>.md`. **Check those files and branches first** — they are the
authoritative per-track state.

## Audit: no mocks / stubs / hardcodes (`dev/audit/`)
Read-only workflow (`dev/workflows/audit-no-stubs-mocks-hardcodes.js`, 13 area scanners + 2 adversarial verifiers per
area + coverage critic). The scan phase finished: **389 candidate findings** (11 high, 129 medium, 249 low) — candidates,
NOT verified; verification was partway done (core, merge, engine, policy verified). Raw data: `dev/audit/audit-snapshot.json`.
Re-run/resume: `Workflow({scriptPath: "dev/workflows/audit-no-stubs-mocks-hardcodes.js"})`; to triage cheaply, filter the
JSON by severity/production_path and verify only high+medium production findings.
High-severity candidates (verify, then fix with a test each):
1. `ag-merge/src/tree.rs:304` — `merge_full` swallows `TreeError::TooManyEntries` (merged tree larger than the limit).
2. `ag-server/src/control.rs:218` — unparsable credential `scope_json` silently becomes the UNRESTRICTED default scope
   (must fail closed; add test with garbage scope).
3. `ag-server/src/api/objects.rs:125` — object quota enforced only on the upload path, not for blobs/changesets written
   via virtual workspaces.
4. `ag-server/src/api/events.rs:137` — SSE topic filter applied after reading 1024 rows; non-matching windows advance the
   cursor and skip matching events permanently (filter in SQL / loop until limit).
5. `ag-server/src/api/events.rs:119` — stream claims to end on credential revocation but only re-checks the connect-time
   snapshot; re-validate on each batch.
6. `ag-sandbox/tests/oci.rs` — OCI backend verified only against a canned shell-script runtime; add tests against real
   docker/podman in CI (digest-pinned image).
7. `services/agent-orchestrator/internal/session/scripted.go` — a simulated agent (`type: scripted`) ships in the product
   binary; remove from the shipped binary, keep as a test-only executable.
8. `services/agent-orchestrator/internal/testutil/fakeagwp` (~2000 lines) — in-memory fake of the AGWP server backs most
   unit tests; replace with a harness that starts the real `ag-server`.
9. `ag-storage/tests/s3.rs` — "real S3" suite actually talks to moto; add MinIO (signature checks, versioning) in the
   backends job.
10. `deploy/kubernetes/base/deployment.yaml:36` (+ compose/docs) — default image tag `latest` is never published; no tag
    or release exists → `ImagePullBackOff`. Publish a real `-rc` and make the default resolvable.
Also known: the orchestrator's advisor/reviewers/semantic index are deterministic heuristics → replace/augment with
model-backed implementations behind the existing interfaces; **no API keys in code** — the session had OpenRouter
reachable with proxy-injected authorization, so a provider (OpenAI-compatible chat API) can be verified live.

Release-track findings not yet fixed: `repo.create`/list responses hardcode sqlite/fs backends (ag-server; fold into the
HA track/audit); `deployment.md` verified-table still says MinIO and "nothing was run" (Compose now uses SeaweedFS);
`ci.yml`'s `images` job duplicates `containers.yml`; drop `continue-on-error` in `containers.yml` once the observability
step is green.

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
* **Run the release rehearsal from the Actions UI** (Release → Run workflow, version `0.0.0-rc.dry`, publish off): the
  session's GitHub token gets `403 Resource not accessible by integration` on `run_workflow`/cancel, so agents cannot
  dispatch. Then fix what it finds (see `dev/handoff/release.md`; likely: `macos-15-intel` runner — cross-compile on
  macos-14 instead — attestation permissions, `install.sh` attestation verification).
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
