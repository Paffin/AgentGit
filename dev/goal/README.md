# Platform qualification ledger

Objective: [doc/goal.md](../../doc/goal.md). Baseline: `19bdcd5433328a933154daa9fe1709d5f5a89c04`; worktree was clean. This ledger is an inventory and acceptance backlog, **not a claim of platform readiness**.

## Evidence rules

`coverage.csv` keeps every current public API operation and the additional capabilities named by the goal. An API row means implemented API, not a completed user task. Unknown screen, role or storage mappings stay explicit. `source-inventory.json` records document hashes; inclusion does not claim that every historical assertion has been reviewed. Historical `dev/audit/` candidates and handoffs must be checked against current code.

Previous browser tests used API-created fixtures and scripted execution helpers. They demonstrate regressions and contracts, not the clean-account, browser-only, real-agent lifecycle required by the new goal. The 2026-10-27 PR #62 evidence remains useful within that narrower scope (460 unit tests, 788 browser cases with one isolated WebKit retry, 47 successful CI jobs; historical mutation scores are under harness revalidation); it is not substituted for independent users, physical devices or a real executor.

## Comparison baseline

Pin **GitLab Community Edition 18.4.0**, source tag `gitlabhq/gitlabhq:v18.4.0`, commit `01f1e46b6622f4d88a09a7e2731efddf1476fadf`. This is a reproducible reference, not a claim that it is the newest release. Premium/Ultimate functions are tracked separately; no trial feature counts as CE. GitHub features and agent-native tools need separately dated versions and evidence. No comparative timing or superiority claim has been established. Use identical clean tasks, roles, repository sizes and hardware; capture success, actions, time, p50/p95/p99, memory and errors before drawing conclusions.

## Stages and acceptance

1. **P0 — preserve work.** Recover file drafts after reload, expired sign-in and a failed connection without changing the branch or losing the original base. Keep accounts and tabs isolated. Report storage failure honestly. Prove the path on an empty real server using UI-created data. Tab-close/device-loss recovery needs account-backed storage and is a separate acceptance gate.
2. **P0 — qualify boundaries.** Check private-workspace expectations against read/list/object/audit authorization. Do not put confidential autosaved drafts on the server until that contract is verified. Validate stale heads, repeated submission and lost responses with negative cases.
3. **P1 — complete daily code/review.** Multi-file editing, server search, inline threads, suggestions and reviewer assignment. Each is a UI/API/storage vertical slice; existing four-state invariants remain mandatory.
4. **P1 — identity and tracker.** Organizations, membership, invitations and issue lifecycle; connect user issues to orchestrator tasks. Include archive/restore, dependencies, labels and milestones.
5. **P2 — execution and release.** Configure a real executor and worker from UI, run a real pipeline DAG, retain logs/artifacts, cancel/retry, manage secrets, deploy/approve/roll back, publish a release. External service credentials remain external prerequisites, never simulated success.
6. **P2 — administration and portability.** Setup/diagnostics, backup verification and restore, webhook delivery, registries, notifications, richer migration with read-back reconciliation.
7. **Qualification.** Comparable performance runs; physical phone and screen-reader sessions; independent pilots. Native GUI follows core web paths, beginning with macOS Apple Silicon; CLI maintenance remains current priority, desktop remains later per the user's instruction.

## Open evidence and integration work

Open issues #50–#55 (excluding absent #54) concern real MCP-host, independent pilot, clean macOS installer, screen-reader and real-device qualification; they remain open. Ten open PRs were inventoried, including #47 and dependency updates; none is considered resolved merely by existing. Hosted migration still omits issues/discussions/permissions/releases/CI settings and external LFS payloads. Older roadmap and comparison text includes already-fixed claims (file rename/delete, Home's fixed 30-repository limit, release status); reconcile documents as each capability is qualified.

No production deployment, external messaging, registry publication or secret publication is part of this stage. No subagents are used.
