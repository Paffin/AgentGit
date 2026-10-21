# Project status and handoff

Maintainer working notes, not user documentation. Rewritten 2026-10-21 after the three first-time-user tracks (Web UI, zero-setup start,
test honesty) landed and `main` was made green; before that 2026-10-20 (waves 3 and 4, crate renaming, `v0.2.0-rc.2`). Everything durable is in git: this file, `dev/agent-brief.md`,
`dev/handoff/*.md` (authoritative per-package state, evidence, run links), `dev/audit/`. User-facing truth:
`README.md` (limits), `CHANGELOG.md`, `ROADMAP.md` (Next).

## Owner's directives
1. Work directly on `main`. Agents never: force-push, tag, release, publish to a registry, open PRs, change repository
   settings. The agents' git proxy also blocks branch deletion and tag pushes, so the owner pushes tags.
2. "200% ready": no mocks, stubs, simulated behaviour or hardcoded values in product code; every claim backed by a test,
   a CI run or an explicit "not verified" (`dev/agent-brief.md`).

## Done
* **Release**: `v0.2.0-rc.2` is published as a pre-release (tag commit `0ad4753`; run 39434938489, 15 jobs green): 4
  archives (Linux and macOS, x86_64 and aarch64), 3 SBOMs (`ag`, `ag-server`, `ag-worker`; none for `ag-mcp`),
  `SHA256SUMS`, `agentgit.rb`, multi-arch images `agentgit-server` and `agentgit-cli`. Verified: checksums, quick
  start, server flow, `install.sh`; image attestations (server and CLI) by the owner with `gh attestation verify`
  (SLSA provenance v1, `release.yml` at `refs/tags/v0.2.0-rc.2`). `v0.2.0-rc.1` failed in the notes step (fixed in
  `202fbdd`) and stays an empty page. Details: `dev/handoff/release-rc2.md`.
* **Crates** are named `agentgit-<x>`; 21 are publishable and dry-run in `Packaging`; nothing is on crates.io
  (`dev/handoff/crate-names.md`).
* **Wave 3** (in rc.2): `agent.operate` for other agents' runs (breaking for writer tokens), atomic quotas, keyset
  paging, trusted proxies, idempotency TTL, in-flight byte budget, `Retry-After-Ms`, derive status and dangling
  references in doctor, readyz, metrics (`dev/handoff/wave3-server.md`).
* **Wave 4** (on `main`, not released): `ag agent reap --remote` and a leased periodic reaper, metadata schema 3 (paging
  indexes, dated idempotency records), 3 durable commits per integration, decoded-tree cache, bounded same-stream
  retry, job-claim fencing token (ADR-0011, ADR-0012) (`dev/handoff/wave4-core.md`). Head `ce114df` was green in CI,
  Quality, Windows, CodeQL, Security, Docs, Packaging, Containers, Performance, Web UI.
* **Dependabot**: all 25 pull requests resolved, none open (`dev/handoff/dependabot.md`). Pages is enabled by the owner
  (the site answers 200).
* **First-time-user work** (on `main`, not in rc.2; `dev/handoff/web-ux.md`, `onboarding.md`, `test-honesty.md`): the Web UI
  (Home, plain labels, blockers with fixes, wizards, rules presets, browser editing, `e2e/jobs.spec.ts`); `ag-server dev`,
  `ag login/logout`, policy presets, `ag policy explain`, `ag explain`; the skip gate (`AG_REQUIRE_COMPONENTS`,
  `tests/skip-allowlist.toml`, `scripts/check_skips.py`). Making `main` green afterwards: patched `rustls-webpki`,
  `webpki-roots`, `certifi`, the licence list, a port race in the multi-replica test, a Windows `dev` test, two CodeQL alerts.
* README (EN and RU) redesigned and kept in step; docs fold of this wave (CHANGELOG, ROADMAP, README, traceability,
  releasing evidence).
* Earlier packages (server hardening and audit, HA, core/storage, execution plane, SDKs/ops, Web UI, MCP, orchestrator,
  security gates, docs site, performance, Windows): all done, limits in their `dev/handoff/` files.

## Open
* Leftovers of the first-time-user work are the first item of `ROADMAP.md` "Next" (one text file per browser edit, no
  tooltips on tab labels, Home polled, no screen-reader pass, `ag explain` local only, dev link not one-time, Docker
  one-liner unverified, rc.2 archives lack `dev`/`login`/`explain`). Test-gate leftovers: root-only sandbox halves run in no
  CI job, TOML scripts not in the Python 3.9 job.
* Unverified for rc.2: archive attestations, macOS and aarch64 archives outside CI, the Homebrew formula; no `ag-mcp`
  SBOM; no Windows archive; no stable tag (`install.sh` needs `--version`).
* Not proven: `ha` and Prometheus Operator overlays on a real cluster, mixed builds in a rolling upgrade, PostgreSQL
  failover, reaper lease with real replicas, PostgreSQL/S3/TLS/multi-replica load, first nightly fuzz and weekly mutation
  runs, `perf.yml` on a runner, `ag-mcp` in real hosts, a second OIDC provider, a screen-reader pass.
* Engineering follow-ups: server option requiring the job-claim fencing token; client address in the audit chain;
  `Arc<Tree>` in the cache; per-stream queue.

## Owner actions
* Push tags (agents cannot). Next candidate (it would carry `ag-server dev`): tag the newest green `main` after bumping version and CHANGELOG with
  `scripts/check_release.py vX.Y.Z --notes`; never move a published tag.
* Optional: `gh attestation verify` of an archive; delete the empty `v0.2.0-rc.1` release page; delete leftover remote
  branches (`claude/quirky-faraday-xr6hiu`, `wip/*`, `web-product-ci`, `web-tail-ci`).
* Switch on private vulnerability reporting (state unknown); dismiss fixed CodeQL alerts.
* crates.io: publish the 21 crates in one sitting (`docs/operations/releasing.md#cratesio`); npm and PyPI need the
  one-time trusted-publishing setup.
* Look at the first runs of the nightly fuzz, weekly mutation, the opt-in live model step and the Pages deploy.

## How to resume
1. Read `dev/agent-brief.md`, this file, then the newest `dev/handoff/` file for your area and `ROADMAP.md` "Next".
2. Work in a worktree, land on `main` (`fetch`, `rebase`, `push origin HEAD:main`), watch CI
   (`https://api.github.com/repos/Paffin/AgentGit/actions/runs?head_sha=<sha>`; the session token gets 403 on dispatch).
3. Checks: `AG=/home/user/ag-target/debug/ag python3 scripts/check_docs.py`, `scripts/gen_cli_docs.py --check`,
   `scripts/gen_readme_assets.py --check`, `scripts/check_no_stubs.py`, `scripts/check_workflows.py --strict`,
   `scripts/check_deploy.py --strict` (needs kustomize, promtool), `scripts/check_release.py vX.Y.Z --notes`,
   `mkdocs build --strict`.
4. Environment: shared cargo target `/home/user/ag-target` (never `cargo clean`; ~25 GB disk), no Docker daemon, no
   Windows host (proof comes from Actions), headless Chromium at `/opt/pw-browsers`.
5. Commit trailers: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>` and
   `Claude-Session: https://claude.ai/code/session_01F8AFjCQ5Q47gBZQNqYFtGQ`.
