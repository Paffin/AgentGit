# Handoff: web_tail (Web UI track tail)

Supersedes the "NOT done" list of `dev/handoff/web.md`.

## Done

1. **Flaky axe `scrollable-region-focusable` (root cause).** `useScrollRegion` decided "does this box scroll" from a
   ResizeObserver on the box alone. Content that outgrows the box without the box resizing and without the owner
   re-rendering (a child component updating itself, a font loading) left an overflowing box that was not yet a
   focusable region; an axe run in that window failed. Now it also observes the children's sizes, the DOM below it
   (MutationObserver: same microtask checkpoint) and the fonts. `e2e/scrollregion.spec.ts` reproduces the window
   deterministically and **fails on the old code** (shown locally), passes on the new.
2. **Firefox and WebKit are in `web.yml`'s matrix** (with chromium, tablet, phone), plus the Keycloak job. Findings fixed
   on the way, all test-side timing/engine behaviour, none product defects found: WebKit reports a computed colour / an
   outline of 0px until its next animation frame after a change (theme toggle test, "focus is always visible"): the tests
   now wait for the engine instead of reading once. The earlier Firefox failures (timeline tab, layout shift) no longer
   fail; `focus is always visible` failed once in Firefox (element 12) and passed on the next run: the same wait is
   now in place, it was not reproduced separately. WebKit (only) reports every request to the app's own server still in
   flight when a document is left (navigation or page close) as an uncaught exception ("<url> due to access control
   checks"); `e2e/fixtures.ts` ignores exactly that message for WebKit and same-host URLs. A real CSP/CORS violation is
   also a console error and still fails tests. Not investigated further whether the app could avoid the rejection.
3. **Onboarding** `ConnectAgentPanel` on a repository with no changesets/proposals; commands in `src/lib/onboarding.ts`;
   `e2e/onboarding.spec.ts` takes them from the page and runs them with real `curl` and `ag` (clone, whoami, snapshot,
   push) and sees the proposal on the board. Unsafe identities are refused (unit + e2e).
4. **Version banner** (`ui-version.json` from the build, `server_version`/`protocol_version` from `/v1/capabilities`);
   Rust test keeps `apps/web/package.json` and the lock equal to the workspace version; `e2e/version.spec.ts` uses a
   second real server serving a copy of the build with another version.
5. **`/unauthorized`**: mid-session 401/expiry lands there with the server's message and request id.
6. **Real OIDC provider**: Keycloak 26.0.7 (tag + digest pinned) in the CI job "real identity provider (Keycloak)":
   `e2e/keycloak.spec.ts` (login redirect round trip, groups to roles, wrong password, silent re-authentication, sign-out at
   the provider, token expiry after the provider revoked the session via its admin API) and
   `crates/ag-server/tests/oidc_keycloak.rs` (server validation with tokens Keycloak issued: discovery, accept, group
   roles, tampered signature/payload/header, foreign audience incl. Keycloak's access token, expiry with leeway 0;
   `#[ignore]`d outside that job). The fake provider (`e2e/support/idp.ts`, `security.rs`'s built-in one) stays, documented
   as a **test double**, for failures a real provider will not produce on request.
7. `docs/guides/web-ui.md`: new sections and a "Verified where" table.

## Evidence

- Web UI at `9598a7a` (all 7 jobs: checks, chromium, firefox, webkit, tablet, phone, Keycloak):
  https://github.com/Paffin/AgentGit/actions/runs/39136540471 . Earlier iterations on branch `web-tail-ci` (red to green):
  39129846258, 39130490754, 39131891385.
- CI, Windows, Security, Docs, CodeQL at `cb60df1` (contains all of this): CI https://github.com/Paffin/AgentGit/actions/runs/39136844190 ,
  Windows .../39136844173 , Security .../39136844273 , CodeQL .../39136844255 , Docs .../39136844285.
- Keycloak server tests: 3 passed against Keycloak 26.0.7 in the job above; browser spec 6 tests.
- Branch `web-tail-ci` on the remote is a leftover of the iteration (delete it).

## NOT verified

- Screen reader (NVDA/VoiceOver/TalkBack) and keyboard-only use on real devices; real phones. Tablet/phone are Chromium
  with touch emulation.
- Identity providers other than Keycloak (Entra, Okta, Auth0). Keycloak is in dev mode (H2), HTTP on localhost.
- The Keycloak job was written without being able to run Docker here: it was iterated in CI only.
- ID-token lifetime equals the client's access-token lifespan: observed (the expiry tests pass) but not read from docs.

## Next

- Pin Playwright browsers' versions only through `@playwright/test` (already pinned); bump together.
- A Keycloak run per release candidate with a real TLS issuer would cover the `https` CSP/CORS path.
