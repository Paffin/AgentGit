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
| 14 | release & distribution | release.yml, containers.yml, deploy/, scripts/install.sh, packaging/ | in progress; WIP branches `wip/dist-track*` on the remote |
| 15 | server HA (control plane on PostgreSQL) | ag-server, ag-storage | in progress; ADR-0007 first |
| 16 | web product (embed UI, responsive, a11y, cross-browser, OIDC PKCE) | apps/web, web.yml | in progress; branch `web-product-ci` |
| 17 | MCP server `ag-mcp` | crates/ag-mcp, docs/guides/mcp.md | in progress (binary builds) |
| 18 | security & quality gates (deny/audit/CodeQL/fuzz/coverage/mutation/compat fixtures) | fuzz/, deny.toml, security workflows | in progress; first fuzz fixes landed (`2f918df`) |
| 19 | CLI UX + docs site | ag-cli, docs/ | in progress |
| 20 | Windows & portability | cfg-gating, windows.yml | in progress; first slice landed (`4c30244`) |
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
* Enable GitHub Pages (Settings → Pages → Source: GitHub Actions) so the docs site deploys.
* Delete the leftover remote branches listed above.
* Decide when to cut the first release tag (publishing images/crates/SDKs is outward-facing; nothing is published yet).

## Dev-environment notes (ephemeral container)
* Shared cargo target dir: `~/.cargo/config.toml` → `build.target-dir = "/home/user/ag-target"`, `incremental = false`.
  The disk is small (~250 GB total but ~25 GB usable); test executables in `ag-target/debug/deps` are the bulk — delete
  stale ones (`find … -perm -u+x -mmin +45 -delete`), never `cargo clean` while agents run.
* No Docker daemon in the container; container/k8s/Windows proof must come from GitHub Actions (use
  `mcp__github__actions_*`, keep outputs small — list with `per_page` 1–3).
* Headless Chromium at `/opt/pw-browsers`; helper binaries (kustomize, kubeconform, promtool) were downloaded into the
  session scratchpad and are not durable.
* Commit trailers: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_01F8AFjCQ5Q47gBZQNqYFtGQ`.
