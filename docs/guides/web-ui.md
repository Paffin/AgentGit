# The Web UI

The Web UI is a browser interface to an AgentGit server. It is made for watching and steering **agents at
work**: who is running, who holds which part of the repository, what is waiting for what, and, for every
proposal, whether it can land and if not, why, in plain language.

It is a static app. `ag-server` serves it; it talks to the server only through the public
[HTTP/JSON API](../protocol/README.md), with the same credentials and the same permission checks as any other
client. Nothing in the UI can do what the API would refuse.

![The live board](../assets/ui-board.png)

## Turning it on

`ag-server` serves the UI itself: the build of `apps/web` is compiled into the binary (cargo feature `ui`, on by
default), so one file is the whole product. Release and CI builds run `scripts/build_ui.sh` (Node 22.18 or newer:
`npm ci`, a production build, pre-compressed brotli/gzip copies) and then `cargo build --release -p ag-server`. The
container image (`deploy/docker/Dockerfile`: a Node stage runs the script, the Rust stage embeds its output) is built
that way, and CI checks that the image's server serves the app. The release workflow builds the UI before its
binaries too (and its archive smoke test checks the app is served); that part has not been rehearsed since it was
added. A server built without the UI build still compiles and runs; a browser then sees a page that says how to get the
UI (and API clients keep their JSON 404). If you built the UI by hand after a first cargo build, `touch
crates/ag-ui-assets/build.rs` (Cargo cannot watch a directory that did not exist yet; `build_ui.sh` does it for you).

```toml
# ag-server.toml
[ui]
enabled = true                  # default; false leaves only the API
# dir = "/srv/agentgit/web"     # serve a copy of apps/web/dist from disk instead of the embedded build
```

`server.web_dir` is the old spelling of `ui.dir` and still works. Open `http://127.0.0.1:8080/`. Requests under
`/v1/` and `/agwp.`, and `/metrics`, `/healthz`, `/readyz` and `/openapi.json`, always reach the server (a path
under those prefixes that matches nothing is the API's JSON 404, never the page); every other path that is not a
file serves the app, so a bookmarked URL works after a reload. Missing assets are 404s.

Hashed assets are sent `Cache-Control: public, max-age=31536000, immutable`; the page and everything else is
`no-cache` with an `ETag` (304 on revalidation); brotli or gzip is chosen by `Accept-Encoding`. Every UI response
carries `Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:;
font-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors
'none'` (no inline script and no eval; inline style attributes are allowed because WebKit refuses the app's styles
under a stricter `style-src`), `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer` and
`X-Frame-Options: DENY`. With redirect sign-in configured, `connect-src` also names exactly the identity providers'
origins. The UI finds the API on its own origin; nothing is configured.

To try it without setting anything up, `npm run demo:server` in `apps/web` starts a throw-away server with a
realistic team and four agents, and prints a token for each of them (see the
[developer README](../../apps/web/README.md)).

## Signing in

**Single sign-on (redirect).** When the server lists an identity provider, the sign-in page offers "Sign in with
<name>": OpenID Connect authorization code with PKCE (S256), `state` and `nonce` checked by the browser, then the
code is exchanged at the provider's token endpoint. The server still verifies every token with the provider's keys
(`GET /v1/auth/config`, public, tells the UI which providers exist; see the [protocol](../protocol/agwp-v1.md)).

```toml
[[auth.oidc]]                                  # makes the server accept the provider's tokens
issuer = "https://idp.example.com/realms/acme"
audience = "agentgit-web"                      # = the UI's client_id when the ID token is presented
group_roles = [{ group = "platform", role = "admin", repo = "acme/*" }]

[[ui.login]]                                   # makes the UI offer the button
issuer = "https://idp.example.com/realms/acme"
client_id = "agentgit-web"                     # a public client; redirect URI: <UI origin>/auth/callback
# scopes = ["openid", "profile"]; token = "id_token" | "access_token"
# authorization_endpoint / token_endpoint / end_session_endpoint: otherwise read from discovery
```

The provider must allow requests from the UI's origin (CORS) on its discovery and token endpoints. If its token
endpoint is on another origin than the issuer, set `token_endpoint` so the Content-Security-Policy can allow it.

*Trade-off, on purpose:* the token is kept **in memory only**, never in `localStorage` or `sessionStorage` (only the
short-lived PKCE/state/nonce survive the redirect, deleted when the callback is handled). So a script injected into
the page cannot read a stored credential, but a reload (or a second tab) asks the provider again, silently
(`prompt=none`: instant while the provider's own session lives, otherwise you are asked to sign in). A refresh token,
when the provider issues one, renews the session silently a minute before expiry; without one you are warned and
the session ends at expiry with an explanation. Signing out also ends the provider's session when it offers a
logout endpoint. Redirect sign-in needs a secure context (HTTPS or localhost) for PKCE.

**Paste a token.** Still available next to it, and the way for scoped credentials: `agt_…` tokens minted by an
administrator (`POST /v1/credentials`, or **Admin → Credentials**), an OIDC ID or access token, or the bootstrap token.
A pasted token stays in this tab's session storage (closing the tab signs you out), is sent only in an
`Authorization` header and never appears in a URL. If a credential expires or is revoked while you work, the session
ends with an explanation instead of a page full of errors.

## The live board

Open a repository and you land on its board. It follows the repository over a live event stream; the indicator at
the top right says whether it is **Live**, **Reconnecting** (it retries with backoff and catches up on what it
missed) or has lost access.

- **Agent runs.** Each active run with its task, how long ago it last sent a heartbeat, and the leases it holds. A run
  that has been silent for longer than a threshold you choose (the selector on the panel) is marked **Quiet** with how
  long it has been silent. That is a hint that an agent may be stuck, not a verdict.
- **Lease map.** Every active lease: its path scope, whether it is exclusive or soft, the **fencing token** an
  integrator must present, and a countdown. Leases whose scopes may overlap are called out together with who holds them.
- **Open proposals.** One card per proposal, with the four independent verdicts and the first blockers (below).
- **Tasks and dependencies.** The task graph: what is in progress, what is blocked and by which tasks, what is done.
- **Integration queue and conflict predictions.** What is waiting to be integrated, and where open proposals, leases
  and the stream itself are about to collide: two proposals editing the same files, a proposal editing files inside
  someone else's exclusive lease, a stream that moved under a proposal.
- **Activity.** The event log as sentences ("`alice` took a soft lease on `src/**`"), newest first. Housekeeping events
  are hidden unless you ask.

Agent heartbeats and lease expiry do not appear in the event log, so those two parts re-check every few seconds
(the page says so).

A dark theme is available from the toggle in the header; by default the UI follows your system.

![The live board in the dark theme](../assets/ui-board-dark.png)

## A proposal

![A proposal](../assets/ui-proposal.png)

The header names who proposed it. For an agent's change it shows the **run**, the **model** and the **task** behind it
(from the agent's signed provenance record).

**Four independent states.** A change is not "ready" or "not ready"; four separate things can each be true or false:

| State          | Question it answers                                                                 |
| -------------- | ----------------------------------------------------------------------------------- |
| **Mergeable**  | Does the change combine with the stream's current head without conflicts?           |
| **Validated**  | Does every check the policy requires have a trusted passing result *for this exact tree*? |
| **Approved**   | Does it have the approvals the policy asks for, *of its current head*?              |
| **Integrated** | Has it landed?                                                                      |

They are shown separately because they change for different reasons: approving a proposal does not make it
mergeable, a moved stream changes the tree that must be validated, and pushing a new commit makes earlier approvals
stale (they are listed as stale, not counted). When something is in the way, **What is in the way** lists each blocker in
plain language with what to do about it, for example *"An exclusive lease on `src/auth/**` is held by
`agent:refactor-bot`. Integrate with the lease holder's current fencing token, or wait until the lease is released or
expires."*

**Tabs.** *Changes* is the diff (files, hunks, line numbers; binary files are named, not shown); *Reviews* shows
each decision with the head it looked at; *Validation* shows each required check, the job that ran it and the signed
attestation (who signed, for which tree, under which policy version, in which environment); *Provenance* shows
what the agent said about itself (rationale, inputs, parameters), always as untrusted text, with whether the signature
verified; *Timeline* is the history of the proposal.

**Actions** are *Review*, *Request validation*, *Integrate*, *Add to queue* and *Close*. Each is enabled only if your
roles allow it and otherwise says which role it needs. *Integrate* shows the strategy (fast-forward or merge), asks for fencing
tokens when a lease is involved, and reports the outcome the server decided: integrated, blocked (nothing changed, with
the blockers), conflict (with the paths) or refused by policy.

## Streams and history

- **Streams** lists streams with their heads and the policy version in force, and the **reflog**, the append-only
  record of every move of a head.
- **History** shows the changesets of a stream as a graph; merge commits show their parents. Opening a changeset shows
  its message, files and the diff it made.
- **Files** browses the tree at any revision, with file sizes, and shows files as text. A **Blame** switch shows, for each
  line, the changeset, author and time that last changed it (renames are followed; lines whose origin is further back
  than the server looked are marked *older than shown*). If a server does not offer blame, the page says so.
- **Compare** shows the diff between any two revisions (`main~3` and `main`, two changeset ids, ...).

![Blame](../assets/ui-blame.png)

## Agents

*Runs* (active, or the history of finished ones with their summaries), *Tasks* (a board by state, with what each is
waiting for), *Leases*, and *Registered agents* with their identities and public keys.

## Administration

Visible only where you hold the `admin` role; anyone else sees what the page needs instead of controls.

![Administration](../assets/ui-admin.png)

- **Policy.** An editor for the stream's policy with checks while you type (JSON errors, unknown fields with a
  suggestion, settings that can never be satisfied), the server's own refusal shown verbatim if it still rejects the
  document, and a **version history with a diff between any two versions**. Saving creates a new version. Attestations made
  under an older version stop counting, on purpose, and the UI says so.
- **Roles** and **Credentials.** Grant and revoke roles. Mint tokens for yourself, or, as an administrator, for an
  agent or service, optionally limited to repositories, streams or operations. A new token is **shown exactly once**: in full, selected and with
  a copy button (and a button to hide it), and gone from the page when you close the dialog. The server keeps only a hash, so it cannot show it again.
- **Settings and quota**, **GC and fsck** (garbage collection always starts as a *dry run*; the real run is only offered
  after one, behind a confirmation), **Audit log** (filterable, with a button that verifies the hash chain) and **Pins**.

## Keyboard, accessibility and small screens

Everything works without a mouse: a skip link, one tab stop per tab list with arrow keys, dialogs that trap focus and give
it back, a visible focus ring everywhere, and screen-reader announcements for route changes, toasts and the live
indicator. State is never colour alone: states have icons and words, diffs have `+`/`−`. The UI follows your system's
colour scheme and `prefers-reduced-motion`. Below 820 px wide (or on a touch screen) every control is at least 44 px and
the header, tab strips and tables adapt: tables that do not fit scroll sideways as a labelled region the keyboard can reach.

What is tested, and what is not (honestly):

- **axe-core** (WCAG 2.x A and AA plus best practices) runs on every main screen in the light and the dark theme, at
  desktop, tablet (768) and phone (390) size, plus the error screens and the main dialogs; serious or critical findings
  fail the build. Findings fixed on the way: a table that scrolls without keyboard access, an account button with no
  name on a phone, a tab-strip that hid its current tab.
- **Layout** is asserted at 768 and 390 px in Chromium (no page-wide horizontal scroll on any route, header and menus fit,
  every repository tab reachable, dialogs fit and keep their action in reach, controls at least 40 px).
- **Browsers:** CI runs the suite in Chromium (plus the tablet and phone layouts, also Chromium). Firefox and WebKit
  projects exist in `playwright.config.ts`; their first CI runs found a WebKit CSP incompatibility (fixed) and a few
  Firefox-only failures that are not yet fixed (`dev/handoff/web.md`), so those two engines are **not** claimed to work
  and are not in the required matrix yet.
- Automated checks find only part of the problems. Keyboard-only flows of the proposal page and the dialogs are tested
  (`keyboard.spec.ts`); a pass with a real screen reader (NVDA, VoiceOver, TalkBack) and on real phones has **not** been
  done. There is no command palette. Known minor issues: the tab strips of a repository have no scroll hint other than the
  cut-off tab; the diff and blame views are read-only on a phone (they scroll sideways inside their own box).

## Security notes

- **Everything the server sends is treated as untrusted text**: proposal titles, descriptions, review comments, task and
  run summaries, file names and contents, diffs, and what agents write about themselves. It is only ever shown as text. The
  code base forbids HTML injection (`v-html`, `innerHTML`, `eval`, inline handlers) by lint rule and by test, and the
  end-to-end suite seeds a repository full of hostile markup and checks that none of it runs.
- **Content-Security-Policy.** The UI needs nothing outside its own origin (no CDN, no web fonts, no inline scripts), so the
  server's strict policy applies in full.
- **Tokens** live in the tab's session storage and in an `Authorization` header only; they are never written to a URL, local
  storage, a log or the page after the one-time reveal.
- **Permissions** shown in the UI come from your roles and are a convenience. The server decides, and refuses
  anything you may not do whatever the page offers.
- **Same origin.** The UI is served by the server it talks to, so no CORS configuration is needed. If you serve it from
  elsewhere, list that origin under `cors_origins` in `[server]` and keep the same CSP.

## Not in the UI

- Writing code. The UI shows and steers; agents and people write through workspaces, the CLI and the SDKs.
- Email or chat notifications. The board is live while it is open.
