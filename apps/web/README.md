# AgentGit Web UI

The browser interface to an AgentGit server. It is built around **live agent work**: which agents are
running and whether they are still alive, who holds which lease, what is waiting for what, and, for every
proposal, four _independent_ verdicts (Mergeable, Validated, Approved, Integrated) with the blockers in
plain language. It is not a clone of a code-hosting site.

It is a static single-page app (Vue 3, TypeScript, Vite) that talks to the server only through the public
HTTP/JSON API ([`docs/protocol/openapi.json`](../../docs/protocol/openapi.json)). There is no backend of its
own: `ag-server` serves the built files itself.

![The live board](../../docs/assets/ui-board.png)

A user tour with more pictures is in [`docs/guides/web-ui.md`](../../docs/guides/web-ui.md). This file is for
people working on the UI.

## Quick start

```bash
cd apps/web
npm ci

# 1. Develop against a running server (default http://127.0.0.1:8080).
AG_SERVER_URL=http://127.0.0.1:8080 npm run dev     # proxies /v1, /openapi.json and /healthz

# 2. Build the files ag-server serves.
npm run build                                      # -> apps/web/dist
#    ag-server.toml:   [server]
#                      web_dir = "/path/to/AgentGit/apps/web/dist"

# 3. Or click through a throw-away server with realistic data (see "The demo world" below).
npm run demo:server                                # builds, starts a real ag-server, prints tokens
```

Requirements: Node 22.18 or newer. For `demo:server` and the end-to-end tests also an `ag-server` binary:
`AG_SERVER_BIN=/path/to/ag-server`, or `target/debug/ag-server` after `cargo build -p agentgit-server`.

### Scripts

| Script                | What it does                                                                                  |
| --------------------- | --------------------------------------------------------------------------------------------- |
| `npm run dev`         | Vite dev server with hot reload; proxies the API to `AG_SERVER_URL`.                          |
| `npm run build`       | Type-checks, then writes `dist/`.                                                             |
| `npm run typecheck`   | `vue-tsc` for the app, the unit tests and the end-to-end suite (all strict).                  |
| `npm run lint`        | ESLint (flat config, Vue + TypeScript) and `prettier --check`.                                |
| `npm test`            | Vitest: stores, API client, helpers, every UI component.                                      |
| `npm run test:e2e`    | Builds, then Playwright against a **real** `ag-server` (see below).                           |
| `npm run demo:server` | Real server + demo world + the built UI, kept running for manual use.                         |
| `npm run gen:api`     | Regenerates `src/api/schema.gen.ts` from `docs/protocol/openapi.json`.                        |
| `npm run check:api`   | Fails if the checked-in `schema.gen.ts` differs from a fresh generation (this is a CI check). |

## Architecture

```
src/
  api/          typed fetch client, SSE, error type, generated types (schema.gen.ts)
  lib/          pure helpers: formatting, four-state derivation, blockers, diff parsing, DAG and graph layout,
                policy analysis, blame grouping, roles, events -> sentences
  stores/       Pinia: auth (session), live (one event feed per repo), board (derived live state), prefs, toasts
  composables/  useResource (load + keep + cancel), useNow, useFocusTrap, useLiveReload, useProposalData ...
  ui/           the design system: Button, Dialog, Tabs, DataTable, DiffViewer, Timeline, ... (no framework)
  components/   domain components built from ui/: ProposalCard, StateStrip, BlockerList, board/, admin/ ...
  views/        one file per screen (routes); shell/ is the header
  styles/       tokens.css (design tokens), base.css (reset, focus, utilities)
e2e/            Playwright suite, the real-server harness and the demo world
tests/          cross-cutting unit tests (source hygiene: no v-html, no eval, no console, no external origins)
```

### The API layer (`src/api`)

- `client.ts` is a small `fetch` wrapper: `Authorization: Bearer`, a fresh `X-Request-Id` per request, an
  `Idempotency-Key` on every mutation (the **same** key is reused across retries of one logical action),
  `Request-Timeout` so the server knows our deadline, and an `ApiError` carrying `code`, `retryable` and
  `request_id`.
- Retries are deliberate: only errors the server marks retryable (or network failures of requests that are safe
  to repeat: reads, keyed mutations, read-only POSTs), honouring `Retry-After`, with a cap. `429` is waited out while the wait stays within `maxRetryWaitMs` (30 s) and `maxRetries` (3); beyond that the error surfaces.
- `sse.ts` implements Server-Sent Events over `fetch` streaming (so the token can travel in a header, never in
  a URL): a parser for the wire format, `Last-Event-ID`/`after_seq` resume, heartbeat comments ignored, a stall
  watchdog, de-duplication, exponential backoff with jitter.
- `schema.gen.ts` is generated (`openapi-typescript`) and checked in. `types.ts` gives friendly names to the
  generated types and corrects the places where the OpenAPI document says "free-form object".
- The server clock is read from the `Date` header, so "lease expires in 4m" is right even if the laptop clock is not.

### Live updates (`stores/live.ts`, `stores/board.ts`)

One event feed per open repository. On entering a repository the store reads `latest_seq` and replays the recent
tail (so the activity feed is not empty), then follows `GET /events?after_seq=…`. Every event topic marks the
board sections it affects as dirty (`lib/events.ts: sectionsFor`); dirty sections are refetched together, coalesced
over ~180 ms, with bounded concurrency. After a reconnect the feed reports a `resync` and everything is
refetched, because events may have been missed.

Two things leave **no event**: agent heartbeats and lease expiry. The board therefore also polls runs and leases
every few seconds, and says so on the page. "Quiet" (a run silent for longer than a threshold you choose) is
computed in the browser from `last_heartbeat_ms`.

The connection state is always visible (Live, Connecting, Reconnecting, Signed out, No access, Paused).

### The four states (`lib/fourstate.ts`)

The server returns one `prepare` result per proposal plus the reviews; the UI derives four separate verdicts
from it and never merges them into a single "mergeable" tick:

| State      | Comes from                                                                                        |
| ---------- | ------------------------------------------------------------------------------------------------- |
| Mergeable  | `strategy`, `conflicts`, `problem` of the candidate tree                                          |
| Validated  | `validations[]` (per required check: passed, failed, pending, missing)                            |
| Approved   | `insufficient_approvals` / `changes_requested` blockers and the reviews bound to the current head |
| Integrated | the proposal's own state                                                                          |

An approval is _bound to the head the reviewer looked at_; when the head moves it is shown as stale and not
counted. Blockers (`lib/blockers.ts`) are turned into sentences with a remedy for each code the server defines.

### Security model

Everything the server sends is untrusted text: titles, descriptions, review bodies, run summaries, file names and
contents, diffs, agent-written provenance.

- Text is only ever bound as text. `v-html`, `innerHTML`, `insertAdjacentHTML`, `eval`, `new Function`,
  `document.write` and inline event handlers are banned by ESLint **and** by `tests/security.spec.ts`; the
  end-to-end suite seeds a repository full of hostile markup (`acme/xss-lab`) and checks that none of it runs.
- The production build works under the server's CSP
  `default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; connect-src 'self'` (plus
  `frame-ancestors 'none'`): no CDN, no external fonts, no inline scripts, no `eval`. The end-to-end suite listens
  for `securitypolicyviolation` and for any request to another origin.
- The token lives in `sessionStorage` (this tab only), in memory, and in an `Authorization` header. Never in a URL,
  `localStorage`, a log or the DOM after minting (the one-time reveal of a new token is the only place it is
  shown, in full so it can be copied, with a button to hide it, and the page forgets it when the dialog closes). Nothing in `src/` calls `console.*`.
- The `next` parameter after sign-in only accepts same-origin paths.
- The UI hides or disables what your roles cannot do (`lib/roles.ts`, parsed from `whoami`), and says which role
  is needed. This is guidance only: the server is the authority and refuses forbidden requests regardless.

## Design tokens

`src/styles/tokens.css` is the only place colours, spacing, radii, type sizes, shadows and durations are defined.
Components use `var(--…)` and never literal colours.

- **Themes.** Light by default; dark when `prefers-color-scheme: dark`; a toggle in the header cycles
  system → light → dark and sets `data-theme` on `<html>` (stored in `localStorage` as a preference only). The dark
  tokens are declared twice, once under `@media (prefers-color-scheme: dark)` guarded by
  `:root:not([data-theme='light'])`, once under `:root[data-theme='dark']`.
- **Groups.** Surfaces (`--bg`, `--surface*`), lines (`--border*`), text (`--text`, `--text-2`, `--text-3`), the
  accent, semantic colours with a soft background and a line (`--ok`, `--warn`, `--bad`, `--info`), identity kinds
  (`--agent`, `--service`), code and diff colours, type (`--font-*`, `--fs-*`), space (`--s-1`…, 4 px base),
  radii, shadows, motion (`--dur-*`, `--ease`).
- **Contrast.** Every text/background pair used by a component is at least 4.5:1 (3:1 for large text and UI
  boundaries) in both themes. The accessibility end-to-end test runs axe-core on every main screen in both themes
  and fails on serious or critical findings; that includes colour contrast.
- **Meaning never rests on colour alone.** States have shapes (check, clock, alert, cross, dash); diff lines have
  `+`/`−` and screen-reader text; identity kinds have distinct glyphs (person, robot, gear) as well as colours.
- **Motion** is short and is switched off under `prefers-reduced-motion`.

## Adding things

### A component

1. Put it in `src/ui/` if it knows nothing about AgentGit (a button, a table); in `src/components/` if it does.
2. `<script setup lang="ts">` first, then `<template>`, then `<style scoped>` (ESLint enforces the order).
   Props are typed; events are declared with `defineEmits`. Bind text with `{{ }}`, never HTML.
3. Style with tokens only. Make it work with the keyboard (real `button`/`a`/form elements first; ARIA only where
   needed), give every control an accessible name, and give anything that loads an empty, a loading and an error state.
4. Add a spec next to it (`Foo.spec.ts`, Vitest + Vue Test Utils): its contract, its keyboard behaviour, and that a
   hostile string renders as text.

### A screen

1. Create `src/views/FooView.vue`. Get the repository from `useRepoName()`, load data with `useResource` (it keeps
   old data while reloading, ignores stale responses and cancels on unmount).
2. Add the route in `src/router/index.ts` (repository screens are children of `RepoLayout`; set `meta.title`).
   Add a tab in `RepoLayout.vue` if it should be reachable from the repository header, gated by the role it needs.
3. If it should update live, call `useLiveReload(topicPrefixes, reload)` or read from the `board` store; map any new event
   topic in `lib/events.ts` (`describeEvent`, `sectionsFor`) and cover it in `lib/*.spec.ts`.
4. Add an end-to-end spec in `e2e/` (what a person does, against the real server) and include the screen in
   `e2e/accessibility.spec.ts`.

### An API operation

Add a typed wrapper to `src/api/endpoints.ts` (the generated types give you the shapes). If the OpenAPI document
changed, run `npm run gen:api` and fix the compile errors: they are the list of things to update.

## Tests

- **Unit** (`npm test`, Vitest + happy-dom): the API client (retry rules, idempotency-key reuse, timeout, error
  mapping), the SSE parser and reconnect logic, stores, every helper in `lib/`, and every component in `ui/` and
  `components/`, including hostile input. `tests/security.spec.ts` scans the source for forbidden constructs.
- **End-to-end** (`npm run test:e2e`, Playwright, Chromium): runs the built UI against a **real `ag-server`**. Nothing
  is mocked; the only simulated actors are the ones that would be outside the server in real life (see below). The
  suite covers: signing in with a bad and a good token; the live board updating when a proposal, run, lease, task or
  queue entry is created through the API; the four states flipping as policy, validation, approvals and
  integration change (including a real approval through the UI by one user and integration by another, with the first
  user's page updating live); conflicts; the diff viewer (including binary files); blame; admin (minting a token that is
  shown once, the policy editor with server-side validation, roles, GC dry run first, fsck, audit chain
  verification, pins); a read-only user; keyboard navigation and focus traps in dialogs; both themes; hostile text
  and the CSP; loading, error, rate-limited and unreachable-server states; layout shift; and axe-core on every main
  screen in light and dark. Every test also fails on any console error, warning or uncaught exception.
  It writes the screenshots in `docs/assets/ui-*.png`.

Running it:

```bash
npm run test:e2e                                  # all
npx playwright test e2e/proposal.spec.ts          # one file (build first: npm run build)
npx playwright test --project=main --no-deps      # everything except the screenshots
AG_E2E_CHROMIUM=/path/to/chrome npm run test:e2e  # use a specific browser
```

Browser: `AG_E2E_CHROMIUM`, else `/opt/pw-browsers/chromium` if present, else Playwright's own
(`npx playwright install chromium`). Server: `AG_SERVER_BIN`, else `/home/user/bin/ag-server`, else
`target/{debug,release}/ag-server` in the repository.

### The demo world (`e2e/support/`)

`e2e/global-setup.ts` starts `ag-server serve` on a free port with a temporary data directory and `web_dir` pointing at
`dist/`, then builds the world **only through the HTTP API** (`seed.ts`): a team (an admin, a writer, a reviewer, an
integrator, a reader, a CI service) and four agents (`refactor-bot`, `docs-writer`, `test-fixer`, `dep-updater`)
working on `acme/webapp` with a task graph, runs, leases with fencing tokens, a two-version policy, a merge queue,
signed provenance, a conflict, a failing check, a stale approval and a protected path. Two more repositories are
an empty one and one full of hostile text.

Two things run beside the server, because in real life they live outside it. Both talk to the server over HTTP
exactly like the real thing:

- **a CI stand-in** claims `validate` jobs from the execution plane and submits an Ed25519-signed attestation for
  exactly the tree it was given. It does **not** run the command in the policy (`npm test`); it reports pass, or
  fail for proposals a test asks it to fail.
- **heartbeats** for three of the four agent runs. The fourth never beats, so it goes quiet on the board.

Tests that change things create their own repository (`freshRepo`), so they do not disturb each other or the demo.

## Dependencies

All versions are pinned in `package.json` and `package-lock.json`. Runtime: `vue`, `vue-router`, `pinia` (all MIT).
Everything else is a development dependency and is not shipped. See `package.json` for the list; licences are MIT
except Playwright and TypeScript (Apache-2.0) and axe-core / `@axe-core/playwright` (MPL-2.0, test-time only).

## Known limits

- Line-level blame needs a server that has the `history.blame` operation; on others the UI says it is unavailable.
- Heartbeats and lease expiry produce no events, so those parts of the board refresh by polling every few seconds.
- The file viewer shows files up to 8 MiB; images up to 1 MiB are previewed; everything else is text or a download.
- Directory listings (200 per page), the history log and the proposal list (50 per page) are paged with a "load more" control; the activity feed and code blocks are virtualised.
- OIDC sign-in is by pasting an ID or access token; there is no redirect flow.
- The end-to-end suite has been run on Linux with Chromium only.
