# The Web UI

The Web UI is a browser interface to an AgentGit server. It is made for watching and steering **agents at
work**: who is running, who holds which part of the repository, what is waiting for what, and, for every
proposal, whether it can land and if not, why, in plain language.

It is a static app. `ag-server` serves it; it talks to the server only through the public
[HTTP/JSON API](../protocol/README.md), with the same credentials and the same permission checks as any other
client. Nothing in the UI can do what the API would refuse.

![The live board](../assets/ui-board.png)

## Turning it on

Build the UI once and point the server at the result:

```bash
cd apps/web
npm ci
npm run build                 # writes apps/web/dist
```

```toml
# ag-server.toml
[server]
listen  = "127.0.0.1:8080"
web_dir = "/srv/agentgit/web"   # a copy of apps/web/dist
```

Start the server and open `http://127.0.0.1:8080/`. Requests under `/v1/` and `/agwp.` always reach the API;
every other path serves the app (it handles its own routes), so a bookmarked URL works after a reload.

The server sends the UI with a strict `Content-Security-Policy`
(`default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'`),
`X-Content-Type-Options: nosniff` and `Referrer-Policy: no-referrer`. The UI is built to work under exactly that:
it loads nothing from any other origin.

To try it without setting anything up, `npm run demo:server` in `apps/web` starts a throw-away server with a
realistic team and four agents, and prints a token for each of them (see the
[developer README](../../apps/web/README.md)).

## Signing in

Paste a token. Tokens are the same ones the CLI and SDKs use: a scoped credential (`agt_…`) minted by an
administrator (`POST /v1/credentials`, or from the **Admin → Credentials** page), an OIDC ID or access token if
your organisation configured single sign-on, or the bootstrap token for first-time setup. After signing in you see
who the server thinks you are, what roles you hold, and when the credential expires.

The token stays in this tab's session storage (closing the tab signs you out), is sent only in an
`Authorization` header, and never appears in a URL. If the credential expires or is revoked while you work, the
session ends with an explanation instead of a page full of errors.

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

## Keyboard and accessibility

Everything works without a mouse: a skip link, one tab stop per tab list with arrow keys, dialogs that trap focus and give
it back, a visible focus ring everywhere, and screen-reader announcements for route changes, toasts and the live
indicator. State is never colour alone: states have icons and words, diffs have `+`/`−`. The UI follows your system's
colour scheme and reduced-motion setting. The test suite runs an automated accessibility audit (axe-core, WCAG 2.x A
and AA) on every main screen in both themes; automated checks do not replace trying it with your assistive technology,
and reports of problems are welcome.

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
- Redirect-based single sign-on. OIDC works by pasting an ID or access token.
- Email or chat notifications. The board is live while it is open.
