# Evidence journal

## Baseline and first slice

- Read the objective from GitHub and imported its exact commit `19bdcd5` after verifying the Git blob/tree/commit hashes. Baseline was clean; no user changes were discarded.
- Enumerated all open GitHub items (15 on one page: five issues, ten PRs), all open PRs (ten on one page), and four published prereleases. Current published beta is `v0.3.0-beta.1`; source head contains subsequent web improvements.
- Reviewed current API inventory, Web UI/editor/auth code, architecture/security/storage/testing material and relevant web/test handoffs. Full historical-document reconciliation remains open; the source inventory explicitly distinguishes inventory from review.
- Verified a concrete gap: FileEditor only holds pending text in component memory. Navigation guard and download help, but accepting a reload or returning after authentication destroys the draft. The first implementation slice addresses that gap using tab-scoped storage; server persistence is not claimed.
- Existing tests often seed user data through APIs. New acceptance coverage must create repository and edited content through UI on a fresh real server, with bootstrap authentication treated as installation setup.

- Baseline browser reproduction on an empty real server: bootstrap sign-in, project creation with Solo policy, initial file creation/proposal/integration all performed through UI. After typing Unicode text and reloading, reopening the editor showed the original content instead of the draft. The test failed on that exact data-loss assertion. Earlier interrupted harness attempts are not counted as evidence.
- Draft-storage tests cover: account/repository/branch/path isolation, exact base/content restoration, no incidental credential serialization, malformed-data preservation, explicit removal and truthful quota-failure status.

## Mutation harness qualification

The first draft mutation run reported kills even for a changed timestamp boundary that no test exercised. Diagnostics showed that Vitest in the tool's symlink worker could not load `/@fs/.../tests/setup.ts`; no tests ran. That score is invalid. Earlier reported mutation counts from the same setup (including the 44-site figure associated with PR #62) are not treated as established evidence until rerun with a qualified worker. Ordinary unit, browser and CI runs are independent and unaffected.

The qualified worker copies its Vite configuration and test setup into the isolated root, resolves aliases to the mutated tree, permits reads of shared unchanged modules, and rejects a result that reports no executed tests. An untested boundary mutation must survive before adding the boundary test; this checks the checker itself.

- Added `apps/web/scripts/mutation-test.mjs` and three subprocess tests for the harness itself. A failed baseline is rejected; a worker that runs no tests remains an unverified survivor rather than being counted as a kill. The timestamp boundary survived before its missing test was added, demonstrating that the corrected runner can distinguish survival from failure.
- Draft restoration's UI-created workflow passed Chromium, Firefox, tablet and phone. A WebKit attempt hit a close/navigation timing failure and a second attempt crashed the page during competing sign-out navigation; these are not counted as passes. The test now waits for the actual landed state and sign-in route before further navigation. WebKit remains under investigation until a fresh run succeeds.

- Revalidated the 35 older file/modal mutations using the corrected adapter: 31 assertion-detected failures and four narrowly reviewed equivalent survivors, retained in `mutation-review.md`. Added missing exact upload limits, empty-branch/missing-file and modal-root/layering tests. No unexplained kill claims are carried forward.
- WebKit qualification exposed a test assumption: returning after sign-in correctly preserves the requested file route, not necessarily Home. The browser test now waits for authenticated identity and the requested route; it does not race a second document navigation against the SPA's sign-in transition. Fresh verification is required before marking WebKit done.
- The corrected adapter also revalidated all nine selected Home catalogue/snapshot mutation sites with actual tests. They were caught. The 35 file/modal sites have four reviewed equivalent survivors rather than the previously reported all-killed score.
- WebKit's rapid document navigation during unfinished SPA transitions triggered local engine crashes. The UI workflow test now waits for the landed state, the completed sign-in route, and loaded file content before reloading. This preserves the intended user assertions instead of ignoring crashes or console failures. Qualification remains pending until the stabilized scenario passes.

## Current verification snapshot

- 478 unit tests passed across 50 suites, including storage-getter denial (red before the fix), quota failure, malformed records and the mutation-adapter protocol.
- Documentation command checks pass across 81 files using the current built CLI. Running them with the published beta.1 CLI correctly fails on later `ag auth status` documentation; an old release is not substituted for current source validation.
- Local WebKit still crashes on the final read-back reload, with and without tracing. A minimal sessionStorage/reload probe passes ten iterations; this does not prove the product scenario. The failure remains visible and requires official CI/browser qualification. Chromium, Firefox, phone and tablet passed the earlier complete workflow; the latest storage-denial fix requires fresh regression.
- The browser-only scenario uses the bootstrap administrator on a newly started empty server. It does not yet qualify a separate writer role, actual credential expiry, offline operation, a physical device or cross-device recovery.

- Fresh final draft recovery passed Chromium, Firefox, phone and tablet (40.4 s aggregate suite). Two additional real-server race regressions passed all three desktop engines (six cases): restored drafts retain the old changeset parent after another writer advances the branch, and a new-file draft from an empty branch is not silently rebased onto another writer's first commit. The latter must reject proposal creation and preserve the draft. These race regressions use API-created fixtures and are not counted as clean-account UI acceptance.
- Static type checks, ESLint/Prettier and the production build passed for `d5e2383`. Three equivalent storage-fallback mutations are documented alongside the 37 detected mutations.

## Browser engine investigation

The original Playwright 1.56.1 WebKit also failed in the official `mcr.microsoft.com/playwright:v1.56.1-noble` image (digest `sha256:f1e7e01021efd65dd1a2c56064be399f3e4de00fd021ac561325f2bfbb2b837a`). Native browser diagnostics reported an internal C++ `std::optional<unsigned long>` assertion. Removing the page DOM before the final reload did not resolve it. Those diagnostics are separate from the unchanged acceptance test.

Upgrading the test dependency to Playwright 1.64.1 removes the duplicate older Playwright core and uses the matching official image (digest `sha256:47b510dd9230bf0b9bfc9b20102241e577ea7a99334ab4c3ca19f19770e9b8ea`). The original complete WebKit acceptance path passed. Repeated qualification passed all 34 cases (two passes of the selected suites across Chromium, Firefox, WebKit, tablet and phone) in 1.8 minutes, without retries. No reload, assertion, console guard or crash check is disabled. Full embedded-server CI qualification remains pending. Unit tests (478), types and lint pass with the new dependency.

CI also exposed colored terminal output in the mutation-harness self-test. The adapter already stripped it correctly; the self-test now strips ANSI styling and forces colored subprocess output. Its three protocol cases pass. The earlier CI failure remains a recorded test defect, not a product pass.

## Recovery after another writer removes the path

A new real-server regression demonstrated that an existing saved draft became inaccessible after another writer deleted its file: the missing-path screen had no editor. The missing-path view now recovers that account's draft, preserving the original base and file mode. Cancelling first removes the editor's guard before returning to the repository root. Reopening the saved URL restores the draft; proposing it retains the old parent and detects the edit/delete conflict.

The new path passed all five browser/device projects, including cancel/keep/reopen, after the final UI change. Types, lint, 478 unit tests, production build and 81 documentation checks passed. A broader 22-case run had 21 passes and one WebKit unused-modulepreload warning after its functional assertions completed; the original report is retained. Five unchanged isolated WebKit repetitions then passed with the same console guard. This is an intermittent browser warning, not a claimed all-green broad run or a reason to disable console checks. Independent CI qualification remains required.


## Test throughput and keyboard file navigation (2026-10-27)

PR #68 (`4550df69921f8fe6a4a198ecd30c3737ba1373bb`) retains isolated unit environments while using a bounded VM worker pool and Node environments for pure helper tests. Three before/after runs of the identical 478-test set on Node 24.19.0, constrained to CPUs 2 and 3, measured median wall times of 30.358 s and 5.856 s (80.7% reduction). This is a local controlled unit-suite measurement, not a whole-pipeline or product performance claim. Heartbeat teardown now cancels idle waits and awaits an active heartbeat.

Browser CI builds one shared embedded server and checks its archived binary hashes in six separate project jobs. Every existing scenario remains in the manifest. Real Keycloak stays independently exercised. The first hosted run exposed two setup regressions: Firefox rejected a home directory owned by the host UID; version-compatibility tests needed the on-disk UI fixture in addition to the embedded binary. PR #69 restores the checked UI artifact and corrects the existing home directory ownership. Five workflow contract tests and 23 workflow-checker tests pass. The version suite passes all eight Chromium/Firefox cases locally in the matching official container, with retries disabled. Hosted confirmation is pending.

The same work fixed immutable caching for dotted asset names and avoided preloading a lazy entry a second time while retaining its dependencies. Changing the preload policy also changes chunk hashes. Thirteen server UI tests pass. Strict browser console checks remain in force; no warning or crash suppression was added.

PR #69 adds complete keyboard file selection: arrows, Home/End, Enter, IME and modified-link preservation, plus visible help. The new real-server regression failed on keyboard focus before the implementation; afterward both finder scenarios pass across Chromium, Firefox, WebKit, tablet and phone (10/10, no retries). The integrated unit suite has 494 passing cases. Lint, static types, production UI and embedded-server builds pass. These finder fixtures are API-created; this does not qualify the entire clean-account user journey, screen readers or physical mobile devices.

Five targeted keyboard semantic mutants were caught. The mutation harness now materializes source files in isolated workers so relative test imports exercise the mutated helper instead of the original. Its four self-tests pass, including the symlink-import sentinel. Preload-policy and heartbeat mutants were also caught. This is scoped evidence, not a global mutation score.

### Deterministic event-feed tests

The 19 event-feed unit cases now yield with Node's event-loop primitive and drive the watchdog clock explicitly; they no longer sleep for 260 ms and six 50 ms intervals. Stream parsing still uses real `ReadableStream`/`Response` objects. A local before/after sample measured 861.5 ms versus 28.7 ms inside that test file; this small sample is not a full-suite benchmark. The tests additionally check the instant before the first watchdog tick. Disabling stall disconnect and removing heartbeat activity updates each caused the expected assertion failure. All 494 unit cases pass with shuffled seed 2050.

## Review drafts and atomic reviewed-head checks

The next slice reproduced a separate data-loss issue: closing the review dialog to inspect the diff discarded the comment and decision. The dialog now keeps both while the same proposal page remains mounted, clears after a confirmed successful submission, and preserves failed submissions. A visible head change keeps the text but requires explicit re-review. It does not persist review drafts across a page reload.

A real HTTP test then showed that an author update between inspection and submission caused the old approval to be accepted on the new head (HTTP 200). The new optional `expected_head` is compared in the same metadata write transaction as review/audit insertion. Stale approve, request_changes and comment requests return HTTP 412 without a review or audit entry. Existing role checks remain; malformed heads are rejected. Older callers omitting the field retain current-head semantics. The new `proposals.review_head` capability lets Web UI refuse an older server instead of silently losing this guarantee. ADR-0014 records the compatibility boundary; OpenAPI and Python/TypeScript SDKs are regenerated.

Validation: 502 unit cases, including capability refusal and draft preservation; all five HTTP flow tests; seven protocol cases including legacy requests and new-field transport round trips; 12 Python generated-client tests and TypeScript SDK type checking; Web UI types/lint/build. A real-browser request interception advances the author's head after the reviewer has sent the request: all five browser/device profiles reject it, preserve the text, then accept an explicitly re-reviewed head. An initial locator ambiguity between the Comment radio and textbox was fixed by addressing the textbox role; that failed harness run is retained. The core was subsequently split to keep review recording distinct from its precondition; HTTP tests remain green. Final embedded-binary verification and hosted qualification are still pending at this entry.

The external mutator's selected `!=` → `==` head comparison was confirmed by an executed HTTP assertion (unexpected 200 versus required 412), not a compiler or setup failure. The first unlogged report was not accepted without inspecting the actual command result. Crapper reports complexity 9 for review_at_head, 6 for the API adapter and 4 for store_review; local Rust coverage is not measured, so no numerical CRAP claim is made yet. Dryer reports no duplicate candidates for the changed engine/API files. Four UI semantic mutations were caught by the agent's component tests.


## Continued UI navigation qualification

The UI agent reproduced and corrected keyboard focus loss and leaked temporary textareas in clipboard fallback; URL/filter disagreement after Back/Forward in history; a previous changeset's lazy diff appearing under a new heading; and parent links losing filters or changing the current selection on a modified click. Root reviewed the changes and integrated commits 8512d82, 03d1a9b, 02b417a and 951abc2. Each slice has behavior tests and separate real desktop/phone browser evidence. History applies a keyed component boundary and native links rather than maintaining a second navigation mechanism.

The earlier full local desktop run completed with 619 passes, 18 reviewed Keycloak skips and two WebKit console failures (an interrupted module import during immediate navigation, and unused preload warnings during repository creation). This run is not called green. The hosted PR #69 Web UI run 40550486379 passed every browser job plus real Keycloak, unit/type/lint and the shared embedded build. The local failures remain under investigation during the Vite 8 integration. Console/crash guards are unchanged.


### Vite 8 integration

Merged the checked Vite 8.3.4 upgrade into the browser branch. With locked dependencies, all 525 unit cases pass (shuffled seed 2054) and the production build succeeds. Both WebKit scenarios that failed in the earlier broad local run pass five repetitions each with strict console checks and no retries. The read-only scenario now waits for the actual board before checking that Admin is absent, so it neither asserts against an unfinished page nor cancels its initial module load. Vite's native-config warning is addressed with an explicit TypeScript import extension, not a warning override. This is targeted verification; a full current-tree browser/CI run is still required.


## Checked release requests

A main-only release request workflow waits for successful CI, CodeQL, Containers, Docs, Packaging, Quality, Security, Web UI and Windows for the exact commit, including other triggered workflows. It creates a version tag only after validation, refuses to move an existing tag, and dispatches the existing release pipeline rather than publishing unchecked artifacts itself. The workflow uses neither pull_request_target nor workflow_run, and never checks out pull-request code with its write token.

Twenty-three behavior/contract tests pass. Fifty-three selected mutations are caught, including missing/failed checks, stale main, tag mismatch, incomplete assets and progress output before a wait. Measured Python CRAP is at most 7; no duplicate candidates were found. The actual beta.1 release payload is recognized as complete (four archives, four SBOMs, checksums and Homebrew formula), so installing the workflow does not republish it. Workflow pin/permission checks and release-tooling tests pass. A live request and next release remain pending until the workflow and a version bump are merged; no new release is claimed here.


Release-gate follow-up: two negative tests demonstrated that a second workflow with the same display name could mask a failed required workflow or be mistaken for the request workflow itself. Selection now uses exact workflow paths. The final set has 26 passing tests and 59 detected mutations; measured maximum CRAP is 8. Actual GitHub run metadata confirms the expected workflow-path format.

## Directory and pending-action isolation

Integrated the agent's clipboard-generation guard, directory-reset and pagination-retry changes, plus immutable pending-review inputs. A root regression showed that a capability request resolving after unmount could still start a review POST; a context check now prevents that unsent mutation. Two tests failed before the fix, then passed. An already transmitted POST is not claimed cancellable. The agent separately verified delayed clipboard APIs, real directory resolution and real paginated requests with temporary 503 responses on desktop/phone. New-directory links never reuse previous rows, and failed Load more requests preserve the existing rows and cursor.


## Beta.2 integrated browser qualification

The post-integration WebKit baseline completed with 205 passes, six reviewed Keycloak skips and three failures. The keyboard failure spent about 55 seconds waiting for animation frames under concurrent load; it now polls the same computed focus visibility with a bounded assertion. The SSO reload case now waits for rendered destinations before deliberately replacing the document. Neither console assertions nor crash checks were disabled. A separate latest embedded recovery/review/navigation sweep passed all 50 scenarios across Chromium, Firefox, WebKit, tablet and phone with no retries.

A CSS-only lazy preload experiment with only a shared UI module regressed cold navigation and was rejected. Grouping shared application code while keeping views lazy measured 569/500/491 ms versus 630/584/597 ms for the existing layout, using three fresh Chromium contexts per variant and 100 ms delay per asset request. This local measurement is not a general performance claim. The selected build has 41 files and 172 KiB of Brotli output versus 106 files and 205 KiB before grouping. A real WebKit token-sign-in probe, kept open beyond the preload warning window, reported no console problems. Full final-tree browser and hosted checks remain required.

The consolidated dependency/release candidate preserves the heads of PRs 38, 39, 41, 42, 45, 71 and 73 as ancestors. All 161 TypeScript SDK tests, generated-client freshness, type checks, lint and packaged ESM/CommonJS verification pass. The version metadata for the planned 0.3.0-beta.2 release agrees across Rust, Web UI and SDKs; this is a candidate, not an already-published release.


### Axe 4.14 accessible-name findings

The full WebKit run caught label-content-name-mismatch in the account button after upgrading axe. A separate mobile regression also demonstrated that an initial fix lost the hidden account identity (expected `Account: user:dana`, received `Account:`). The current implementation keeps a complete text alternative at all viewport sizes. The same axe rule found that task-graph button names listed title/state in a different order than visible content; their names now follow the contents and append the dependency names. Existing graph assertions are strengthened to include the visible state and dependency count. All 18 targeted accessibility, task-graph and account-menu checks now pass across Chromium, Firefox, WebKit, tablet and phone, with retries disabled. The initial 44-case scan had exactly the two task-graph failures; the other 42 cases passed. Full final-tree WebKit and hosted validation are still pending.


### Release dispatch contract

An executed workflow-mode regression found that `publish=true` with a bare version kept that bare value as `ref`, which the real release validator correctly rejected. The publish path now validates the selected tag name after checking it matches the requested version. The regression failed against the old shell block, then all 45 release-tooling tests and 26 request tests passed. Cases cover bare/prefixed publication, mismatched tags, branch refusal, independent rehearsal versions and pushed tags. The earlier string-only workflow assertion was replaced by these execution checks; no tag or release was created by this test.


### Complete release metadata

The licence freshness check caught 23 workspace-package references still naming beta.1; the standard generator updates them to beta.2 and a fresh check passes, with no third-party dependency changes. `uv lock --check --offline` also reproduced a stale project version (`0.3.0b1`); `uv lock --offline` changes only that entry to `0.3.0b2`, and the check passes. Python CI now uses locked synchronization and package jobs check the lock before building, so a future drift fails instead of silently rewriting it. All 28 workflow tests and 15 strict workflow checks pass.


Deployment validation caught three default image tags still on beta.1. Compose and both Kubernetes defaults now match the beta.2 workspace version. The deployment guide distinguishes candidate defaults from already-published installation targets; no live deployment was changed. Strict manifest, Prometheus and Compose validation passes after the correction. CLI reference/artwork, the executable README demo, package metadata, no-stubs scan and reviewed-skip validation also pass.

## 2026-10-28 — resumed baseline and editor ownership

Baseline main: `549555c82219c6465e59b4d16a3d5a9d3bea47f9`; working tree clean. All open GitHub pages returned five issues (#50, #51, #52, #53, #55) and zero PRs. Release [40569106697](https://github.com/Paffin/AgentGit/actions/runs/40569106697) completed successfully; `v0.3.0-beta.2` is published with ten uploaded assets. README links now target that published version.

The standard installer remained blocked: its strict provenance step could not retrieve the attestation bundle from Azure storage (HTTP Forbidden), so it correctly installed nothing. The old local GitHub CLI was replaced by a checksum-verified local CLI2.103.3. The same bundle was then obtained inline from the official GitHub attestation API and verified independently against the release workflow, source commit and tag. The archive matched SHA256 `de728142309507d4871163f7e778a99e182b240ab70a7cf2df825f37c84898ea` and was manually installed into an isolated prefix only after verification. The published binaries report commit549555c82219; the real onboarding smoke passed login/clone/push/integrate/pull/verify, protected-path review and backup/restore/verify. This qualifies the manual Linux archive path, not the blocked installer path or clean macOS (issue #52). No checksum-only verification or production deployment was used.

Editor source commit: `9e3d3f9`. Eight new helper/component cases cover pre-cancellation, cancellation during workspace creation/modification, initiating-session propagation, duplicate submits, unmount, account change and rejected credentials. Transport tests exercise retries and old401 responses for the four editor endpoints. The full web unit suite passed **696 tests /73files** in4.48s locally. Type checks and production build passed. Six selected `proposeChange` mutation sites were killed; this is not a repository-wide mutation score. New helper CC8; no TypeScript coverage/CRAP claim is made.

`draft-recovery.spec.ts` uses a fresh real server: only the infrastructure bootstrap credential is supplied. The browser creates the repository and initial file, lands it, edits it, reloads, signs out/in, recovers the draft, submits the recovered edit, lands it and reads it again after reload. The test delays delivery of a **real** workspace-write response and checks that title, description and submit controls are disabled; it does not replace the server result. The old UI failed the new disabled-field assertion. An initial interception-cleanup race was fixed in the harness before the confirmed red run. The updated app passed Chromium desktop and390px phone profiles **2/2,23.1s,no retries**. Console checks were retained. This viewport emulation is not a physical-device test or a real-agent lifecycle.

Artifacts are retained outside source under `agentgit-quality/web-test-speed/goal-editor-{red-verified,final}*`; they include the JSON reports and recovered/pending screenshots. Older interrupted full runs are preserved as incomplete and are not counted as green qualification. The Browser runtime tool was unavailable; the repository's pinned Playwright runtime was used.

The source audit also mapped workspace read/list: `Cap::WorkspaceRead` is inherited by repository readers and those routes do not perform owner filtering. Owner/admin checks guard modification/deletion. Documentation now describes workspace **isolation**, not confidentiality; account-backed confidential drafts remain blocked on a separate boundary design.

## 2026-10-28 — multi-file browser editing

A dedicated Code editor now stages existing text/link-target edits, new files, renames and removals in one draft and one change request. It preserves modes and the original base, refuses duplicate paths and unintended replacement, and restores a scoped tab draft. The reviewer sees before/after staged contents; the proposal contains the server diff. The UI states its 100-file/4MiB limit.

The real-browser scenario starts a fresh server, signs in through the UI, creates a repository, creates two files in one proposal, reloads and recovers the batch, lands it, then renames/edits one file and deletes the other in a second single proposal. It lands and rereads the result after reload. Chromium desktop and phone viewport passed2/2 in22.3s with no retries or console allowances. Three targeted model cases cover batch operation order/modes, duplicate/unsafe paths and draft decoding; type and targeted lint checks passed. This is a concrete first P1 code-editing slice, not full platform readiness. Physical devices, binary editing, remote draft recovery and large-batch measurements remain open.


## 2026-10-28 — file-by-file review workbench

Baseline main `d3ee13716df7ccb8dc876685322603b324fcbb1c`. The change-request Files changed tab now filters loaded paths, records personal viewed-file marks and hides viewed files. Progress is keyed by account/repository/proposal and the exact diff base/head captured from the completed request, so head changes and account switches cannot reuse stale progress. This does not implement durable discussion threads or account-backed review state.

Two new component scenarios failed before implementation because the filter and viewed controls did not exist. The targeted final checks passed 26 cases across diff rendering, proposal data and the new workbench. Production build and touched-file ESLint passed. A fresh real-server browser scenario created the repository and uploaded both files exclusively through UI, then filtered, marked, hid and reloaded progress: Chromium and phone viewport both passed without retries in 8.9 seconds. Axe found no serious/critical violations, and the viewport overflow assertion passed. Both screenshots were saved under `/workspace/agentgit-quality/review-workbench-results`; the phone screenshot was visually inspected. Physical devices and human screen-reader use remain unverified. No repository-wide test run or CI changes were needed for this slice.


## 2026-10-28 — recoverable review feedback

The review dialog now stores the comment, decision and inspected head in tab storage scoped by account/repository/proposal. Remounting restores the original inspected head, so an author update still requires explicit re-review. A successful server response clears the saved draft; errors keep it. Submission pins its original credential and aborts pending follow-ups when the account/session changes. An expired credential leaves the draft available and offers a sign-in link back to the proposal. This is tab-local recovery, not cross-device or account-backed storage.

The two recovery/isolation component cases failed before implementation. Final focused review-dialog and real HTTP transport checks passed 39 cases, including pinned credentials across retries, old 401 isolation, account change during capability lookup/submission, stale heads and duplicate-send prevention. Production build and touched-file lint passed. The real fresh-server workbench browser scenario was extended through composing a comment, closing, reloading, restoring the decision/text, submitting, reading the stored review and checking a fresh empty draft. Desktop Chromium and phone viewport both passed, no retries, in 9.9 seconds. No full-suite rerun was needed.

## 2026-10-28 — permanent task pages and prerequisite lifecycle

Baseline `8538df3ce6004c19fa1d8572d33faada9cb86299`. Task cards and agent-session rows now link to a permanent task page. Readers see owner, state, scope, prerequisites and dependants. Writers add prerequisites through the existing `TaskWrite` API; `ag-agent/src/tasks.rs` checks cycles in the metadata transaction and appends an audit event. Finish uses the existing owner/operator check and audit path, with a confirmation explaining effects on dependent tasks and active sessions. Pending mutations retain their original credential/context. Related proposals/sessions are explicitly matches from the newest 100 repository results, not complete history.

A focused role test first reproduced that a delegator was not treated as a writer in the browser, unlike the server capability table. The UI now mirrors that inheritance without adding review or integrate rights. 31 focused role and HTTP transport cases passed, including original-token retries and old-session 401 isolation for dependency/finish requests. Production build and touched-file lint passed.

The UI-only scenario starts a fresh server, signs in, creates a repository and two tasks, adds the dependency, navigates between task links, marks the prerequisite done, observes the other task become ready, keeps a cancellation unchanged, confirms cancellation, reloads persisted state and returns to the Tasks tab across reload. Chromium and phone viewport passed 2/2 in 12.5s. Axe found no serious/critical violations on the task detail. Early attempts had test-addressing errors (required-field label text and capturing/reloading before SPA navigation settled); these were corrected using accessible textbox names and explicit destination assertions, not application workarounds or skipped assertions. An initial suspicion of an unnamed dialog was disproved by DOM and accessibility-tree diagnostics; Dialog was not changed. Artifacts: `/workspace/agentgit-quality/task-details-complete-results`.

No real agent run was created, and owner-negative/cycle-rejection browser paths are not claimed here. Editing task descriptions, removing dependencies and a user-facing issue tracker remain open work.


After merging PR76's reviewed state into this candidate, all four real-browser task/review scenarios passed together (Chromium and phone, 17.2s) and 56 targeted role/transport/review checks passed. The final task screenshot after reload has no transient notifications and was visually reviewed. Both original review features and the new task workflow are retained.


## 2026-10-28 — bounded server-side content search

Baseline `f3b7c52f1b1a5d863b81b22dcef6d463944f01f1`. Added `tree.search` as a shared HTTP/gRPC operation using existing TreeRead authorization and immutable tree/blob reads. No commands are executed and symbolic links are not followed. Queries are literal, cursor bindings include repository/tree/query/case/prefix, per-request file/byte limits bound the scan, and binary/invalid-UTF8/>1MiB exclusions are explicit. OpenAPI and both generated SDKs expose the operation. The browser preserves search inputs, continues at the returned immutable revision and opens/highlights the matching line, including virtualized content.

Local verification: 3 real HTTP tests passed (0.42s), including a gRPC call, repository isolation (404 masking), unauthenticated access, invalid queries/cursors, exclusions, match pagination, no-match continuation after the 1000-file budget, and a moved branch with successful continuation at the pinned old revision. Two Rust helper tests cover Unicode excerpts and cursor validation; a long valid quoted path tests cursor capacity. The initial HTTP expectation of 403 was corrected to the server's existing 404 repository-hiding contract. This was a test expectation correction, not weakened authorization.

The real-browser path creates its repository and four uploaded files entirely through UI, lands them, finds 61 matching lines over multiple pages, reports excluded content, opens line 2000 in a virtualized file and confirms it is in the viewport, returns to search, changes case/prefix filters and reloads. Chromium and phone viewport passed 2/2 in14.5s; axe had no serious/critical findings. Earlier browser runs identified a missing Code submenu link (fixed) and a test comparing a virtual row's text without accounting for its separate line-number gutter (changed to assert the code text and viewport independently). Browser artifacts are outside source under `/workspace/agentgit-quality/code-search-virtual-results`.

Crapper reduced the initial scanner from CC39 to functions no higher than CC9; no numerical coverage/CRAP claim is made. Dryer found no candidates at similarity0.8. Two selected Mutator sites (cursor-binding equality and file-count boundary) were killed; other mutation sites were not run. Final touched-code Clippy passed with warnings denied; an earlier attempt ran out of working disk during a Docker VFS copy and was interrupted, then rerun after cache cleanup. Web type/build,15 existing code-rendering checks, SDK type/generation checks passed. Physical devices, indexed-search performance, regex and excluded-file search are not qualified.


After integrating the task-pages main commit `49156f9f9eb9129cc9ce8850802a43f34f543cd9`, the combined search/task browser scenarios passed 4/4 in18.8s on the newly built real server. Search includes a verified visible jump to line2000; the OpenAPI document matches the running server's generated schema. Task, review and search changes are all retained.

Final integration validation also passed 48 focused code-rendering, role and transport checks. Rust formatting and touched server/test Clippy passed with warnings denied. All original goal rows remain present; `tree.search` is an added API row, not a replacement for broader search goals.

## Repository issue slice — 2026-10-28, working branch

Implemented user issues separately from orchestrator tasks: repository-local numbering,
search and state filters, descriptions, immutable discussion/activity, author/admin edits,
close/reopen, optimistic revisions and optional same-repository task links. API mutations
write activity, audit and event outbox in the same metadata transaction. SQLite and
PostgreSQL schema 4 migrations are present; live PostgreSQL qualification is not claimed.

The real-browser scenario in `apps/web/e2e/issues.spec.ts` passed against the actual server:
UI-created repository and issue, draft restoration after reload, comment, close, reload,
reopen and search. Local screenshot: `/workspace/agentgit-quality/issues-browser/`.
The focused HTTP lifecycle exercises readers, author, unrelated writer and outsider,
idempotent creates/comments, stale revision rejection and state/activity readback.
Production Web UI build and TypeScript SDK type checking passed. This is not completion
of `doc/goal.md`: provider discussion import, tracker labels/milestones/attachments,
organizations and executable CI/CD remain outstanding.

## Agent-native handoff follow-up — 2026-10-28

An issue author or administrator can now create the associated agent task in the browser,
with an explicit expected result and file patterns. `task.create` accepts a source issue and
its inspected version. Task creation, linking and audit/activity records are atomic;
a stale/closed/already-linked issue rolls back the task, and a retried request reuses the
original task. Unrelated writers cannot attach tasks to somebody else's issue.

Real HTTP verification covered stale-write rollback, author permissions, one task after
an idempotent retry and the stored link. The fresh-server browser workflow additionally
created the task through the handoff dialog, opened its permanent task page and returned
to the linked issue (Chromium, 9.6 s). PostgreSQL 17 independently preserved issue and
comment data through closing/reopening the metadata connection and rejected a stale CAS.
TypeScript/Python client tours exercised the issue API. These checks do not establish
that a real external coding agent ran or that it produced a reviewed/integrated change.

The manual agent-onboarding follow-up passed the existing real-server/real-CLI Chromium workflow (8.4 s): the browser created a task and credential, its displayed shell commands submitted a task-linked change request, and the browser detected that request. The exact-task/principal matcher separately rejects another task and similarly named agents. This is shell workflow qualification, not a real coding-model pilot.

## Operator task-launch gate — execution-control branch, 2026-10-28

The engine now reads a persisted pause inside the same metadata transaction as task
claim and run creation. Delegators/admins change it through HTTP/gRPC with an expected
revision; readers can inspect it. Audit and outbox event share the write transaction.
Schema 5 makes older binaries refuse the repository instead of ignoring this decision.

Local qualification: the engine case passed on SQLite and PostgreSQL 17, including
reopening metadata, refusing a new task run, keeping existing heartbeat/completion
functional and rejecting a stale operator decision. The real HTTP case passed role
and revision checks. The real Go orchestrator with a command executor kept a task open
during the pause, then ran it after resume. Chromium and phone workflows both changed
the setting through the Web UI, reloaded it and confirmed a reader could not change it.
The Web build, TypeScript SDK types and Python SDK types passed. No real external
coding-model execution or browser executor installation is claimed.

## Permanent run inspection — run-details branch, 2026-10-28

`agent.get_run` looks up a run directly by its repository and ID, including after
completion. The UI links to that stable page from the run table, live board and task.
It shows the recorded summary and timing, current reservations, and an exact-session
filter for change requests; it does not call a completed session a validated change.

The focused HTTP case passed historical lookup, read permissions and unrelated-user
isolation. Chromium and phone cases passed table-to-detail navigation, live completion,
reload and filtering out another session of the same agent. Web build and both SDK type
checks passed. These fixtures do not establish an external coding-model pilot or a full
stdout log service. The repository's existing role-boundary invariant also found the
new issue.manage permission missing from NEVER_DELEGATED; fixed and separately verified
in the parent issue PR before merge.

## Queued agent assignment — task-assignment branch, 2026-10-28

Task creation and issue handoff can reserve an enabled registered agent in the
same transaction. The task creator/operator can change a queued reservation with
an expected-owner check or restore automatic routing. The existing atomic claim
rejects another agent; active work cannot be reassigned.

Engine and HTTP cases passed competing claims, stale edits, non-creator rejection,
running-task rejection, unassignment and rollback of an invalid issue-linked
creation. The real Go command executor ran the explicitly selected agent instead
of the default executor. Chromium and phone workflows both passed creation,
reassignment, reload and launch enforcement (2 cases, 7.6 s). Web build/types and
TypeScript/Python operation tours passed (12/13 cases). The operation recorder
now matches static routes before variable routes, avoiding misattributing
`tasks/scheduling` to `tasks/{id}`. Migration verification compares with the schema
registry rather than the obsolete hard-coded version 3.

Evidence artifacts are in `/workspace/agentgit-quality/assignment-*`. No live
external coding-model pilot or browser executor installation is claimed.

Crapper complexity inspection split assignment validation and agent resolution into
separate units; no coverage-derived CRAP score is claimed. Dryer found no duplicate
candidates in the focused units. Mutator discovery was inspected; no mutation kill
rate is claimed for this slice.

## Executor connectivity — executor-presence branch, 2026-10-28

The Go scheduler prepares and verifies configured executor identities before their
first run, then sends expiring operator reports with name and available/total slots.
The server uses its own receive/expiry times, rejects ordinary writers and invalid
capacity, and exposes the latest report through the agent list. A report is not a
command health check, successful model invocation or aggregate fleet capacity.

Focused engine and HTTP scenarios passed persistence, expiry timestamps, role checks
and invalid capacity. Real-server Go scenarios passed executor reporting before the
first task and dispatch to the explicitly assigned command executor. This run used
AG_SERVER_BIN explicitly; no skipped test is counted as runtime evidence. The prior
standalone assignment fixture was corrected to register its chosen key before use.

TypeScript/Python operation tours passed (12/13 cases), and the Web build and type
checks passed. Chromium and phone workflows exercised the registered-only versus
connected states and reload. The registered-agent tab now opens directly to its
agents, without the unrelated task-creation form above it. Crapper complexity,
focused duplicate analysis and mutation-site discovery were inspected; no coverage
percentage or mutation kill rate is claimed.

## Browser executor setup — executor-setup branch, 2026-10-28

The administrator prepares exact-repository service/agent roles and a 24-hour
scoped credential. The generated task-only JSON contains no bearer secret; a
separate private token file is read by the native `-token-file` option. Disabled
review/advisor components no longer require irrelevant credentials during check.
The wizard waits for a new report by a unique per-setup service identity and supports revoke.

Chromium and phone qualification downloaded the real files, checked the credential
scope, invoked the real native `check` and `run` commands, observed connection,
created an assigned task, waited for that command executor to complete and revoked
the token (11.9 s desktop, 10.9 s phone). The native branch of this browser test
requires AG_ORCH_BIN; CI without it still covers downloads, scope and revocation.
No external coding model was invoked. The token file is deleted from test scratch
space and is not retained as an artifact. Credential-file tests also reject unsafe
Unix permissions and invalid content. Web type/build checks passed.

Existing merge blockers were repaired in the parent stack: the PostgreSQL v1
fixture now removes tables from newer migrations, generated protocol fuzz types
match each branch, Python formatting is current, the orphan-run fixture respects
pre-run registration, and live event streams revalidate run-bound credentials
before buffered delivery. Focused PostgreSQL, stream and real orphan-run checks
passed; the test assertions were retained.

Connection confirmation is scoped to a unique service identity for each setup,
so an older executor using the same agent name cannot turn the new setup green.
Revocation also removes that setup service's role binding, while preserving the
agent role that another installation may use.

## Opt-in saved executor output — run-output branch, 2026-10-28

The native executor keeps output local by default. Explicit capture saves up to
32 KiB per stream at completion, scrubbing the session token, signing-key hex
and known environment values. Server-side permission and size checks, monotonic
snapshots, idempotent retries and deletion tombstones passed the focused HTTP
scenario. Real command execution confirmed redaction and unchanged default-off
behavior. The Web UI renders text, supports download and confirms deletion.

TypeScript/Python operation tours passed (12/13); focused Rust Clippy checks passed.
Chromium and phone passed both output inspection/deletion and the downloaded native
executor profile with capture enabled (4 cases, 12.1 s). The native check/run
path was enabled with AG_ORCH_BIN. Unknown or encoded secrets may remain; this is
not a live log stream. Deletion does not erase existing backups or journals.

## Task-scoped history in the beta.4 candidate

The task detail page now retrieves linked proposals and runs with server-side
filters and continuation tokens, rather than filtering the latest repository-wide
100 records. Storage qualification includes 110 rows and tied timestamps; the
HTTP scenario verifies full page walks, filters, malformed tokens and repository
read isolation. The focused browser scenario loads 51 linked proposals and 51
linked runs alongside unrelated activity in Chromium desktop and phone viewport.
The fixture uses API-created history; it does not qualify a new autonomous run or
a physical phone. Typed SDK generation includes run pagination. See ADR 0023.

Retry recovery qualification: the old 200-run agent window lost a spent attempt
behind 205 later unrelated runs and left the task in progress (reproduced before
the fix). Complete task-scoped recovery now cancels that exhausted task without
launching another process. The real server/command regression and existing restart
budget scenario pass together in 1.3 seconds. Client contract checks cover legacy
truncation, repeated cursors/records, unrelated records and later-page failure.

## Published beta.4 delivery, 2026-10-28

Release `v0.3.0-beta.4` is published from
`e67ba0925df12c61211bcb92620b67ad6fdcbb71` with four native platform
archives, four Rust SBOMs, a Homebrew formula and checksums. The downloaded
Linux x86_64 archive hashes to
`db95a1bf5244a30ceff0a4030390604362bf2e5536942074ed55b9c819057a7e`,
matching the exact release-workflow artifact used for local qualification.
GitHub CLI 2.103.3 verified its signed provenance against the repository, exact
release workflow and source digest, requiring a GitHub-hosted runner. The signed
bundle was read from the GitHub API because this environment cannot fetch its
Azure redirect directly.

That same archive passed login, clone/push, protected review/integration,
backup/restore and upgrade from the published beta.3 archive. The upgrade retained
user records, moved metadata to schema 5 and refused opening it with the old
binary. The native orchestrator in the archive reports beta.4. These checks do not
qualify physical devices, an external model or an independent human pilot.

The release workflow's native builds and archive smokes passed on all four
platforms. The final macOS installer job initially failed to acquire a hosted
runner; rerunning that job succeeded and the full release workflow completed.
Publication and Linux provenance verification are complete. Newer main
features, including task-context reuse and repository-wide history paging, are
not part of this immutable tag.

## Proposal discussion after beta.4

The `proposal.comment` HTTP/gRPC operation gives writers, reviewers and integrators
discussion rights without approval rights. Focused HTTP qualification checks
idempotent replay, rejection of changed-key payloads, stale-head rejection, size
bounds, read-only denial and discussion after closure. No schema migration.

The Chromium flow verifies author draft recovery after reload, explicit handling
of a changed head, correct stored author/head, read-only viewing and posting after
closure. TypeScript and Python operation tours cover the new generated API (112
operations). The browser review summary now ignores comments when finding the
latest approval/request-changes decision, matching the server's policy logic.
Nested threads, line anchors and resolution remain open work.
