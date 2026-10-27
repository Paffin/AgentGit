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
