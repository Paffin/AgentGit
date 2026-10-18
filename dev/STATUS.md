# Project status and handoff

Maintainer working notes, not user documentation. Rewritten 2026-10-18 after the product-readiness waves and the
documentation fold. Everything durable is in git: this file, `dev/agent-brief.md`, `dev/tracks/tracks.md`,
`dev/handoff/*.md` (the authoritative per-package state, evidence and run links), `dev/audit/`, `dev/workflows/`.
The user-facing truth is `README.md` (honest-limits table), `CHANGELOG.md` (Unreleased) and `ROADMAP.md` (Next); this
file only says where the work stands and what is left for people.

## Owner's directives (intent)
1. Everything lives on `main`; work directly on `main` is authorised. Never: force-push, tag, release, publish to a
   registry, open PRs, change repository settings.
2. Bring the project to a product state "from all sides", then to "200% ready, with NO mocks, NO stubs, NO simulated
   behaviour and NO hardcoded values in product code": real components, configuration instead of literals, claims backed
   by tests or CI evidence or removed. Test doubles only for what cannot be real in a test, only in test code, named in
   the docs (the shared brief `dev/agent-brief.md` encodes this).

## State of `main`
CI verifier at `7132198` (docs-only head): CI, CodeQL, Security, Docs and Windows all succeeded. The path-filtered
workflows (Containers, Packaging, Web UI) did not run for that commit; their latest runs on `main` are green
(Containers and Packaging at `c9a08a5`, Web UI at `f49fbd8`). `OCI runtimes and cgroup v2` was red once on `27bbce1` (a
test race, not product code) and is green at `c9a08a5` (`dev/handoff/generic-fix.md`). The documentation fold (`1ec826a`)
was green in CI, CodeQL, Security, Docs and Windows.

Verified at the last full runs: 1,036 Rust tests (CI at `148a98c`), coverage 79.1% of 37,285 lines, mutation testing
of five crates at 100% of the mutants that compile, 20 fuzz targets (60 s smoke in CI; 4.3 h local, no crash).

## Tracks
| Track | State | Handoff |
|---|---|---|
| Release and distribution | Image, Compose (PostgreSQL + SeaweedFS), kind, packaging dry-runs and the release rehearsal (end to end from a work branch, run 39076286737) green; **nothing published, no tag; the rehearsal was not repeated from `main`** | `release.md` |
| Server hardening and audit | Done: run-bound credentials (ADR-0010), `delegator`, operator finish/reap, read-consistent `integration.prepare`, fail-closed control rows, atomic byte quota, SSE revocation. Contested items that stay are in `server_audit.md`'s triage table | `server_hardening.md`, `server_audit.md` |
| Server HA | Done and proved with real processes in CI (`backends` job), `ha` overlay validated but not applied to a cluster | `ha_tail.md` |
| Core, engine, storage | Done: merge limit error, ADR-0009, storage fixes, real S3 server (SeaweedFS) leg | `core-engine-storage.md` |
| Execution plane | Done: OCI sandbox proven against real Docker/Podman, cgroup v2, `ag-worker` start-up checks; microVM, Windows, macOS sandbox open | `policy_cli_execution.md` |
| Clients and ops (SDKs, deploy, Web) | Done: image tag = workspace version, Prometheus Operator overlay fixes, SDK timeouts, Python 3.10-3.13 / Node 20 | `clients_ops.md` |
| Web UI | Done: embedded in `ag-server`, Chromium/Firefox/WebKit/tablet/phone, onboarding, version banner, Keycloak job | `web_tail.md` |
| MCP (`ag-mcp`) | Done: guide, protocol and CLI suites, example client in CI, shipped in release archives; not run inside real hosts | `mcp_tail.md` |
| Orchestrator | Done: no simulated agent, real-server tests, optional model mode (run live once, one model, one provider) | `orchestrator.md` |
| Security and quality gates | Done: Security, CodeQL, Fuzz, Quality workflows, compatibility fixtures; nightly fuzz and weekly mutation never ran on GitHub | `security_tail.md` |
| Docs site and CLI UX | Done: MkDocs `--strict`, tutorials executed in CI, generated reference, `check_docs.py` and `check_no_stubs.py` in CI; Pages deploy never ran | `docs_site.md` |
| Performance | Done: `ag-bench`, `perf.yml`, published numbers, blame fix | `perf.md` |
| Windows | Builds, tests and README demo green in CI; no sandbox, no archive | `windows.md` |
| Documentation fold | Done: README (EN and RU), CHANGELOG, ROADMAP, ADR traceability, docs index, this file | (this file) |

Superseded handoffs (`mcp.md`, `web.md`, `security.md`, `cli-docs.md`) carry a banner pointing at the later file; `ha.md`
was folded into `ha_tail.md` and removed. `dev/tracks/tracks.md` and the workflow scripts in `dev/workflows/` are history.

## Audit (`dev/audit/`)
389 candidates, 93 confirmed, 105 contested, 191 refuted. Per the work packages' reports, every confirmed item of every area was assigned to a work
package, fixed with a test (or dropped as stale, and said so in that package's handoff); contested items were triaged
in the handoffs (fixed, or kept with the code or document that makes them legitimate). Items that remain deliberately:
see the "not fixed" lists in `server_audit.md`, `policy_cli_execution.md` (digest check order, digest profile version
needing an ADR), `core-engine-storage.md` (Windows ACL read-back, queue fencing tokens) and `clients_ops.md`. The
critic's list of 65 files no scanner read was not worked through.

## Owner action items
* Enable GitHub Pages (Settings, Pages, Source: GitHub Actions); the first deploy has never run.
* Switch on the dependency graph (dependency review is skipped while it is off) and private vulnerability reporting
  (state unknown); dismiss fixed or accepted CodeQL alerts.
* Triage the 18 Dependabot pull requests (#6, #10 to #26); several are major-version bumps that will not be green.
* Run the release rehearsal from `main` (Actions UI: Release, Run workflow, version `0.0.0-rc.dry`, publish off); the
  agents' token gets 403 on dispatch. Decide when to cut the first tag: that publishes images, archives and (manually
  triggered) SDKs. Until then the default image tag `0.1.0` resolves nowhere.
* crates.io: `ag-agent`, `ag-git`, `ag-protocol`, `ag-worker` are taken by others and two crates read outside their
  directory; nine crates are `publish = false`. Decide a naming scheme (for example `agentgit-*`) before any publish.
* Delete leftover remote branches: `claude/quirky-faraday-xr6hiu`, `wip/*`, `web-product-ci`, `web-tail-ci`.
* For the Web UI Keycloak job, the opt-in live model step (needs the `OPENROUTER_API_KEY` secret and `AG_LLM_MODEL`
  variable) and the weekly/nightly schedules: look at their first runs.

## Next steps (the engineering list is `ROADMAP.md` "Next"; the order that unblocks most)
1. After the owner actions: first `-rc` tag through the release workflow, then fix what it finds.
2. Apply the `ha` and Prometheus Operator overlays to a real cluster (kind with PostgreSQL and S3 in `Containers`).
3. Read the first nightly fuzz and weekly mutation results; fix and add regression tests.
4. Require `agent.operate` for acting on another agent's run (breaking for the SDK examples and Web UI demo).
5. Measure PostgreSQL, S3, TLS and several replicas; decide the decoded-tree cache budget.
6. Run `ag-mcp` in the real hosts; a second OIDC provider; a screen-reader pass.

## Re-running the checks
`scripts/check_docs.py` (needs `AG=/home/user/ag-target/debug/ag`), `scripts/gen_cli_docs.py --check`,
`scripts/gen_readme_assets.py --check`, `scripts/check_no_stubs.py`, `scripts/check_workflows.py --strict`,
`scripts/check_deploy.py --strict`, `mkdocs build --strict` (see `docs/README.md`).

## Dev-environment notes (ephemeral container)
* The cargo target dir is shared (`~/.cargo/config.toml` points at `/home/user/ag-target`, incremental off). Concurrent
  agents overwrite each other's workspace-crate artifacts and the disk is small (~25 GB usable): run few Rust agents at
  once, delete stale test executables
  (`find /home/user/ag-target/debug/deps -maxdepth 1 -type f -perm -u+x ! -name '*.so' ! -name '*.d' ! -name '*.rlib' ! -name '*.rmeta' -size +2M -mmin +30 -delete`),
  never `cargo clean` while agents run.
* No Docker daemon and no Windows host in the container: container, Kubernetes and Windows proof comes from GitHub
  Actions (public API `https://api.github.com/repos/Paffin/AgentGit/actions/runs?head_sha=<sha>`; the session token gets 403
  on dispatch and cancel).
* Headless Chromium at `/opt/pw-browsers`; helper binaries (kustomize, kubeconform, promtool) are not durable.
* Commit trailers: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_01F8AFjCQ5Q47gBZQNqYFtGQ`.
