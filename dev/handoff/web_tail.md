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
   now in place, it was not reproduced separately. WebKit (only) reports every request still in flight when a page is
   closed as an uncaught exception ("<url> due to access control checks"); the fixture ignores exactly that, WebKit
   only, once the test body is over (`e2e/fixtures.ts`, `settled`). During a test it still fails the test.
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

See the final report for run links (filled in after the last push).

## NOT verified

- Screen reader (NVDA/VoiceOver/TalkBack) and keyboard-only use on real devices; real phones. Tablet/phone are Chromium
  with touch emulation.
- Identity providers other than Keycloak (Entra, Okta, Auth0). Keycloak is in dev mode (H2), HTTP on localhost.
- The Keycloak job was written without being able to run Docker here: it was iterated in CI only.
- ID-token lifetime equals the client's access-token lifespan: observed (the expiry tests pass) but not read from docs.

## Next

- Pin Playwright browsers' versions only through `@playwright/test` (already pinned); bump together.
- A Keycloak run per release candidate with a real TLS issuer would cover the `https` CSP/CORS path.
