# Continuous AgentGit progress

Status: **CONTINUE**. Completing a slice does not complete the platform roadmap.

## Recovery and authorization

- Working branch: `codex/roadmap-review-workflows`.
- Original base: `2ad73082106615c5ce1ac0831b9444eb3955b20f` (beta.7).
- Audited current remote main on 2026-10-29: `8a0019424b37bd9ef42739389a0e7a1f7945c660`.
- Local implementation tip before this checkpoint: `ed58caeb2ff4ed10874b80a151c45187be6fe56c`.
- User says **do not merge yet**; accumulate a larger batch. Do not merge, force-push, publish a release or replace tags. Draft PR and CI qualification are the next delivery step.
- Use ub-mode and the existing Hallmark/Web design direction. Keep CLI, SDK and Web behavior aligned; avoid speculative frameworks.
- On resume: read this file, inspect worktree, fetch main, inspect open PR/CI, retain unmerged changes, continue a runnable backlog item.

## Implemented but not merged

| Slice | Evidence |
|---|---|
| Single/range/batched review suggestions with exact-head guard | commits `8e047a9`, `226be48`; Web and browser suites, mutation checks |
| Requested reviewers across core/API/CLI/SDK/MCP/Web, metadata9 | `c5cd1a8`, `29d7a9b`; core/API/client tests; ADR0036 |
| Code-first review page and responsive secondary actions | `97b3df9`; desktop/phone screenshots and browser checks |
| Binary/type/oversized conflict choices and pinned workspace sync | `214a0ea`; core150, HTTP/gRPC, Web812, browser6; ADR0037 |
| Private account review drafts and guarded cleanup, metadata10 | `f8e7fcb`, `5f6bf25`, `e18b594`, `dde140c`; storage/core302, server274, MCP108, Web830, SDK177/202; ADR0038 |
| Rename conflict source paths retained through every client | `58cf3a5`; integration suite, HTTP/gRPC both orientations, Web836, browser12, SDK178/203, CLI56+3; ADR0039 |
| Install/README EN+RU simplification | `c4a7179`; release-script54, ShellCheck, real published beta7 install/login/clone/edit/push/integrate/pull; README quickstart2 |
| Windows ZIP/PowerShell installer and native CI gate | `ed58cae`;15 offline PowerShell/package tests,54 POSIX regressions,9 selected mutations killed, workflow/docs checks; native execution still pending |

Detailed action and validation logs: `/workspace/agentgit-evidence/WORK_LOG.txt` and per-slice logs in that directory. Cumulative binary-safe patch: `/workspace/agentgit-evidence/roadmap-review-workflows.patch`. Logs outside the checkout are supporting artifacts, not substitutes for committed tests.

## Current audit

Read ROADMAP, both READMEs, CHANGELOG, ADR index/0036-0039, architecture traceability/testing, Web UX review, beta gates. GitHub currently has no open PR and four open qualification issues: #50 real MCP host, #51 external pilots, #53 screen reader, #55 physical phone. Main advanced by CI-only #133/#134/#135 (prepared releases, reusable permissions, background runner headroom). Main's background qualification is running; do not call it green merely because fast checks pass.

## Prioritized backlog

| ID | Priority/status | Work and dependency | Acceptance/evidence required |
|---|---|---|---|
| D1 | P0 IN PROGRESS | Reconcile unmerged batch with current main; retain new prepared-release/CI design | clean rebase, no lost Windows/install changes; static CI/skip/docs gates pass |
| D2 | P0 READY after D1 | Push working branch, open draft PR, inspect mandatory CI; **never merge** | exact head checks, fix actual failures; native Windows ZIP/install/PS5.1 evidence |
| R1 | P1 READY | Select next daily-review slice from task2 | small end-to-end deliverable with tests/review; no roadmap reduction |
| R2 | P1 OPEN | Private general discussion drafts | per-account isolation, stale-save/clear protections, UI restore, SDK/CLI parity |
| R3 | P1 OPEN | Account-backed file-edit drafts | immutable base, byte/mode preservation, conflicts keep local work; no automatic publication |
| R4 | P1 OPEN | Multi-path rename/file-directory resolution | complete explicit result, preserve independent clean edits; never infer deletion from failed reads |
| R5 | P1 OPEN | Cross-repository work feed | pagination/scopes, full initial snapshot and reconnect behavior |
| M1 | P1 OPEN | Resumable hosting metadata import: issues/comments first | preview, mapping/idempotency/checkpoint, retry without duplication; PR/releases/identity later |
| Q1 | P1 BLOCKED external | #50 real MCP host | actual named host/version + full bounded task workflow and denied scope; scripted protocol is not enough |
| Q2 | P1 BLOCKED external | #51 five pilots and retention/case studies | external participants and permission; no invented users; requires recruitment authorization/contact |
| Q3 | P1 BLOCKED device | #53 screen reader | actual reader/browser/OS and observed speech/focus |
| Q4 | P1 BLOCKED device | #55 physical phone | actual device/browser and keyboard/network recovery; viewport emulation insufficient |
| P1 | P2 OPEN | Verify Homebrew; publish Windows only after native CI; crates/SDK registry ownership | real install and package ownership/trusted-publishing setup, no unsupported availability claims |
| S1 | P2 OPEN | Deployment/HA: failover, mixed-build upgrade, replica reaping/load/TLS | concrete backend environments and failure evidence; baseline CI isn't production qualification |
| S2 | P2 OPEN | Server follow-ups: required job fencing, audit client addresses, shared tree cache, same-stream scheduling, merge notes on wire | compatibility contracts and concurrency/behavior tests |
| L1 | P3 DEFERRED by roadmap trigger | Hierarchical trees/packfiles/GC generations/streaming, VM isolation/pools | measurements justify scope; no premature architecture |

## Immediate next actions

1. Preserve pre-rebase branch; rebase local batch onto `origin/main`, resolve CI/release conflicts using newest architecture.
2. Run skip-source gate (new Windows tests currently use `unittest.skipUnless`, inspect against repository helper policy), workflow/release/docs compatibility and focused changed-code checks.
3. Open draft PR for whole batch and await/repair CI while implementing the next independent R1 slice.
4. Update this checkpoint with new SHAs, PR, CI URLs, failures and next item. Keep release gates open until evidence really exists.
