# Continuous AgentGit progress

Status: **CONTINUE**. Rebased on current main; draft PR preparation and P0 structural-sync repair underway. Completing a slice does not complete the platform roadmap.

## Recovery and authorization

- Working branch: `codex/roadmap-review-workflows`.
- Original base: `2ad73082106615c5ce1ac0831b9444eb3955b20f` (beta.7).
- Audited current remote main on 2026-10-29: `8a0019424b37bd9ef42739389a0e7a1f7945c660`.
- Local implementation tip before this checkpoint: `ed58caeb2ff4ed10874b80a151c45187be6fe56c`.
- User reiterated on 2026-10-29: **work only in a separate branch; merge all into the next release only when explicitly told**. Do not merge, force-push, publish a release or replace tags. Draft PR and CI qualification are the next delivery step.
- Use ub-mode and the existing Hallmark/Web design direction. Keep CLI, SDK and Web behavior aligned; avoid speculative frameworks.
- On resume: read this file, inspect worktree, fetch main, inspect open PR/CI, retain unmerged changes, continue a runnable backlog item.

## Implemented but not merged

| Slice | Evidence |
|---|---|
| Single/range/batched review suggestions with exact-head guard | commits `fdfd318`, `6c56df3`; Web and browser suites, mutation checks |
| Requested reviewers across core/API/CLI/SDK/MCP/Web, metadata9 | `8b20c45`, `eb8acbf`; core/API/client tests; ADR0036 |
| Code-first review page and responsive secondary actions | `b470792`; desktop/phone screenshots and browser checks |
| Binary/type/oversized conflict choices and pinned workspace sync | `35592e6`; core150, HTTP/gRPC, Web812, browser6; ADR0037 |
| Private account review drafts and guarded cleanup, metadata10 | `1cb6d15`, `a76e99b`, `2eba2d8`, `7e48676`; storage/core302, server274, MCP108, Web830, SDK177/202; ADR0038 |
| Rename conflict source paths retained through every client | `ca0da01`; integration suite, HTTP/gRPC both orientations, Web836, browser12, SDK178/203, CLI56+3; ADR0039 |
| Install/README EN+RU simplification | `dc7f768`; release-script54, ShellCheck, real published beta7 install/login/clone/edit/push/integrate/pull; README quickstart2 |
| Windows ZIP/PowerShell installer and native CI gate | `6210f99`;15 offline PowerShell/package tests,54 POSIX regressions,9 selected mutations killed, workflow/docs checks; native execution still pending |

Detailed action and validation logs: `/workspace/agentgit-evidence/WORK_LOG.txt` and per-slice logs in that directory. Cumulative binary-safe patch: `/workspace/agentgit-evidence/roadmap-review-workflows.patch`. Logs outside the checkout are supporting artifacts, not substitutes for committed tests.

## Current audit

Read ROADMAP, both READMEs, CHANGELOG, ADR index/0036-0039, architecture traceability/testing, Web UX review, beta gates. Initial GitHub audit found no open PR and four open qualification issues: #50 real MCP host, #51 external pilots, #53 screen reader, #55 physical phone. Issue #52 is closed with real published beta.7 macOS arm64 checksum/provenance and57 checks ([evidence](https://github.com/Paffin/AgentGit/issues/52#issuecomment-6508589245)); Finder quarantine/Homebrew remain outside that proof. Main advanced by CI-only #133/#134/#135 (prepared releases, reusable permissions, background runner headroom). Main's background qualification is running; do not call it green merely because fast checks pass.

## Prioritized backlog

| ID | Priority/status | Work and dependency | Acceptance/evidence required |
|---|---|---|---|
| D1 | P0 DONE | Reconcile unmerged batch with current main; retain new prepared-release/CI design | clean rebase, no lost Windows/install changes; static CI/skip/docs gates pass |
| D2 | P0 IN PROGRESS | Push working branch, open draft PR, inspect mandatory CI; **never merge** | exact head checks, fix actual failures; native Windows ZIP/install/PS5.1 evidence |
| R1 | P0 DONE | Fix structural sync losing independent incoming add/edit/delete when file/directory merge falls back to ours | retain raw conflicting candidate, apply explicit choices before tree validation; both orientations/no partial workspace mutation |
| R2 | P1 DONE | Private general discussion drafts | per-account isolation, stale-save/clear protections, UI restore, SDK/CLI parity |
| R3 | P1 OPEN | Account-backed file-edit drafts | immutable base, byte/mode preservation, conflicts keep local work; no automatic publication |
| R4 | P1 PARTIAL | Bounded file/directory Web choices; multi-target rename and larger structural cases remain open | complete explicit result, preserve independent clean edits; never infer deletion from failed reads |
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

## Reconciliation checkpoint

Rebase on `8a00194` completed without losing application changes. Retained main's fast publish workflow; moved Windows build/verification into `prepare-release.yml`. New candidate manifests require and checksum the Windows ZIP without changing recognition of historical published beta.7. Fixed the new PowerShell tests to use the repository's required-component helper; fast tooling runs only shell-independent Windows archive contracts. Full PowerShell tests are required in native jobs. Beta programme status corrected from beta.5 to the actual published beta.7; external gates remain open.

## Active delivery / CI

- Draft PR [#136](https://github.com/Paffin/AgentGit/pull/136), base main, head `codex/roadmap-review-workflows`. **Do not merge.**
- Pushed head `93bc4776aa2dac3623c2f1a8c39f6cc451a7947a` after rebase and prepared-pipeline adaptation.
- Windows native/PowerShell5.1 run explicitly dispatched on that head: [40962143088](https://github.com/Paffin/AgentGit/actions/runs/40962143088), in progress.
- PR fast CI [40962144429](https://github.com/Paffin/AgentGit/actions/runs/40962144429), Web [40962144494](https://github.com/Paffin/AgentGit/actions/runs/40962144494), Docs [40962144418](https://github.com/Paffin/AgentGit/actions/runs/40962144418); statuses must be refreshed before claiming success.
- P0 structural sync repair passed239 merge/integrate tests (3 existing ignored), targeted regressions and independent review. Virtual sync preserves complete candidate; local sync and legacy pending snapshots fail closed before unsafe writes.
- Overall goal remains CONTINUE. The prior stop marked only a milestone; installation/README completion is not platform completion.

## Next available product slice

R4: bounded whole-file/directory choices in Web. Dependency P0 is repaired. A server-supplied complete candidate path list is necessary: input-tree union can miss synthetic paths from directory renames. Do not infer rename mappings in the browser. Bound metadata and browser work; refuse incomplete previews, preserve exact blobs/types/modes and independent changes, require pinned revision/capability guards. Keep multi-target rename/rename separate until explicitly supported.

### Update after P0 repair

Pushed `a58b8805344085cec15807de40457bea14d95c89` to PR136: complete structural candidate resolution, local fail-closed sync, and legacy pending snapshot guard (force cannot bypass). Merge/integrate239 passed,3 existing ignored, clippy and independent review passed. Latest fast CI run40962581592; Docs40962581342 succeeded. Previous fast CI on93bc477 was cancelled by the newer push, not treated as a code failure. Windows40962143088 still builds the earlier93bc477 head; it cannot qualify the later head until a follow-up native run.

Next R4 implementation contract under review: fresh structural sync conflicts get optional bounded `resolution_scope` with candidate paths and a completeness bit. This covers synthetic directory-rename destinations that are absent from both source trees. Web must refuse incomplete scopes and older servers, group nested conflicts under minimal roots, preserve selected blob/type/mode and explicitly remove other affected paths. File/path/request budgets and branch/proposal/session guards are mandatory. No new account-draft schema is needed for this slice.

### Explicit merge/release hold reaffirmed

Verified PR136 remains Draft, OPEN, mergedAt=null; main does not contain working commits93bc477/a58b880/944730d. Continue development/CI on the separate branch. Do not merge or publish any release until the user directly instructs it; the generic continuous objective never overrides this hold.

### Structural preview dependency validated

Core/API `resolution_scope` implemented: max100 paths per conflict/max1000 per response, complete flag, synthetic rename destinations, exact prefix boundaries and legacy compatibility. Current source passed243 merge/integrate tests (3 existing ignored), HTTP/gRPC scope checks, Clippy, seven selected mutations and independent review. SDK179/204 + MCP3 and CLI31+3 shape checks passed. Web whole-root chooser is now being implemented with a shared request/path budget. Fast CI on a58b880 succeeded (Linux, tooling, SDK, Go); its Web/Docs/Security checks succeeded. Native Windows93bc477 compiled and is running tests, not yet qualified.

### Web structural choice validated

Whole-source file/directory UI completed:849 Web unit tests,12 desktop/phone browser cases (8 structural,4 nontext), types/build/lint passed.43 helper mutations and3 targeted Vue mutations caught. Independent review found no blocker. The real browser test caught `tree.list` kind `file` versus `TreeEntry.file_type` `regular`; normalization and a real-kind regression fixed it before commit. Scope/root containment, shared100-request budget,100-source-path/root and1000-dialog-path bounds are enforced. Renamed multi-target conflicts and over-budget structural cases remain open.

CLI follow-up is implementing actual virtual-workspace list/show/create/sync/delete commands (not merely schema tests), using the same pinned-resolution API and complete/incomplete scope output. Lifecycle tests pass; duplicate-path parity and final CLI checks underway. No merge/release permitted.

### Coordinated CLI/Web/SDK slice complete locally

Web849 tests and12 browser cases passed. CLI virtual-workspace lifecycle now implemented (five explicit commands), with pinned resolution files, capability guards, duplicate/ambiguous JSON rejection, complete/incomplete scope output and no automatic proposal mutation. Final lifecycle/unit/clippy and159 generated reference files checked. SDK179/204 and MCP3 passed for the same scope contract. Independent reviews found no blockers. Next push will include these commits in the existing draft PR only; no merge/tag/release is authorized.

Windows run40962143088 is still on earlier93bc477 and currently in Test; do not count it as latest-head proof. A subsequent native run on the final pushed head is required. Full Web matrix will be dispatched on that final head; local desktop/phone tests are not a substitute for all browser/identity-provider gates. Add OS/build metadata to the next Windows log for exact qualification evidence.

### R2 active: general proposal comment drafts

Pushed head `52ad4cc6f8ed67380fc1dd18801dcce127975e8d`. Full Web run40965163166 queued. Native Windows40962143088 completed with failure in PS5.1 installer tests: Get-FileHash unavailable; investigate inherited module environment, do not claim native qualification.

R2 in progress in parallel: backend schema11 independent comment_drafts table; CLI explicit comment-draft get/save/clear; SDK/MCP same API; Web explicit account-save and restore choice for general ProposalComment only. Contract:16KiB UTF-8, full actor ownership, closed allowed, versioned CAS/tombstones/identical retries, no public events, no-store. Thread replies/file edits remain open. Root owns Web API adapter and ADR0042. No merge or release.

Windows follow-up fix committed/pushed as `c33bc9e`: native PowerShell step now starts from its own shell with Get-FileHash preflight. Native run40965818005 in progress; full Web40965163166 queued. R2 backend build hit full disk; reclaimed13GiB inactive Rust incremental cache and resumed with CARGO_INCREMENTAL=0.

R2 backend green:253 tests (integrate37+4+4, server100+1+1, storage106), schema11 preserves existing review drafts/tombstones through reopen and backup. Fresh OpenAPI and Web types regenerated. Web867 unit tests passed after removing concurrency load; worker-overlay mutation counts discarded as invalid after a control test, direct mutation verification replacing them. CLI cargo slot active, SDK generation and browser checks next.

Next R3 design discovered (not implemented): existing virtual workspaces are readable to repository readers and their blobs enter normal history, so do not use them as private draft storage. Small first slice: existing single-file edits only; owner-private versioned draft keyed by stream/original path with immutable base and original blob/type/mode, bounded private content bytes and title/description, explicit save, private list for recovery after current-file deletion, base retention until clear. Public propose remains existing explicit operation followed by captured-version cleanup. Existing FileEditor does not preserve BOM/CRLF like batch editor: establish real-byte regression before implementation. New-file/batch/rename/delete remain later.

R2 delivered in commits36f99cd (API/schema11),acbdb9f (SDK/MCP),4304dc8 (CLI),d1f19a3 (Web). Backend253, Web867+8browser, SDK182/208, CLI2+1 passed. MCP new integration tests not run; static implementation and generated clients complete. User explicitly requested stopping excess tests/checks and continuing feature development; do not run further broad/mutation loops. R3 implementation now active in parallel core/CLI/SDK-MCP/Web with only basic compilation.
