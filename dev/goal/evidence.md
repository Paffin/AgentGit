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
