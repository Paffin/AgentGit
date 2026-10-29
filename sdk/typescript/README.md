# @agentgit/client

TypeScript client for [AgentGit](https://github.com/Paffin/AgentGit)'s wire protocol (AGWP v1, HTTP/JSON + SSE):
repositories, virtual workspaces, proposals, integration, agents with runs, leases and **signed provenance**, the
execution plane, and the event feed.

- **One method per operation.** Every operation of
  [`docs/protocol/openapi.json`](../../docs/protocol/openapi.json), grouped into namespaces named after the operation
  ids (`proposal.create` is `client.proposals.create`). Their request and response types are **generated** from that
  document.
- **No runtime dependencies.** It uses the standard platform APIs available in Node 20+: `fetch`, `Headers`, `Response`, `AbortController`, `ReadableStream`, `TextEncoder`/`TextDecoder`, `URLSearchParams`, `crypto.getRandomValues` and timers. The
  Ed25519/SHA-256 code ([`@noble`](https://github.com/paulmillr/noble-ed25519), MIT) is bundled into `dist/`; see
  [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). ESM, CommonJS and `.d.ts` are shipped; Node 20 or newer (CI runs the tests on Node 20 and 22).
- **Retries that cannot apply a request twice** (see [Errors and retries](#errors-retries-idempotency-and-deadlines)).
- **Early beta (0.3).** Not published to npm yet: build it from a checkout (`npm ci && npm run build` in `sdk/typescript`).
  The package is exercised by the tests in this directory against a real `ag-server`; it has not been run in a browser.

```bash
npm install @agentgit/client   # once published; today: npm ci && npm run build, then npm pack
```

## Quick start

Needs a running `ag-server` (`AG_BOOTSTRAP_TOKEN=... ag-server serve`) and a credential. With the bootstrap token the
same script plays every role.

<!-- example: sdk/typescript/examples/quickstart.ts#quickstart -->

```ts
import { AgentGitClient, putFile, utf8Decode } from '@agentgit/client';

const client = new AgentGitClient({ baseUrl: process.env['AGENTGIT_URL']!, token: process.env['AGENTGIT_TOKEN']! });
await client.negotiate({ clientName: 'quickstart' }); // checks that the server speaks agwp.v1

const repo = `demo/hello-${Date.now()}`;
await client.repos.create({ name: repo });

// Change files on the server, in a virtual workspace: no checkout needed.
await client.workspaces.create({ repo, name: 'hello', stream: 'main' });
const change = await client.workspaces.modify({
  repo,
  name: 'hello',
  message: 'add a greeting',
  changes: [putFile('README.md', '# hello\n'), putFile('src/main.rs', 'fn main() {}\n')],
});

// Propose it, and land it on the stream "main".
const proposal = await client.proposals.create({ repo, stream: 'main', head: change.head, title: 'Add a greeting' });
const result = await client.integration.integrate({ repo, proposal: proposal.id });
console.log(result.outcome.status); // "integrated"

// Read it back.
const readme = await client.tree.file({ repo, path: 'README.md', rev: 'main' });
console.log(utf8Decode(readme)); // "# hello"
for await (const changeset of client.paginate('history.log', { repo, rev: 'main' })) {
  console.log(changeset.id.slice(0, 12), changeset.message);
}
```

Requests are flat objects shaped like the protocol messages (`repo` is `"owner/name"`; path, query and body fields sit
side by side). Missing fields take the protocol's zero value. Whatever goes into the URL path (ids, workspace names, file
paths) is percent-encoded segment by segment; stream names, revisions and the like travel in the query string or the
body, as the protocol prescribes.

## The API at a glance

| Namespace              | Methods                                                                                                 |
| ---------------------- | ------------------------------------------------------------------------------------------------------- |
| `session`              | `negotiate`, `authConfig`, `whoami` (and `client.negotiate()`, which also checks the protocol version)  |
| `repos`                | `create`, `describe`, `list`, `configure`                                                               |
| `workspaces`           | `create`, `get`, `list`, `modify`, `sync`, `delete`                                                     |
| `proposals`            | `create`, `get`, `list`, `update`, `setReviewers`, `review`, `close`                                    |
| `integration`          | `prepare`, `validate`, `integrate`, `attest`, `attestations`                                            |
| `queue`                | `enqueue`, `list`, `run`                                                                                |
| `streams`              | `list`, `create`, `head`, `cas`, `reflog`                                                               |
| `history`              | `resolve`, `log`, `changeset`, `ancestors`, `mergeBase`, `blame`                                        |
| `tree`                 | `get`, `list`, `resolve`, `diff`, `file` (bytes)                                                        |
| `objects`              | `missing`, `put` (bytes), `get` (bytes), `verify`, `publish`, plus `putBlob` and `upload`               |
| `operations`           | `get` (the recorded outcome of an operation, by operation id or idempotency key)                        |
| `agents`               | `register`, `list`, `startRun`, `listRuns`, `heartbeat`, `complete`, `predict`                          |
| `tasks`, `leases`      | `create`, `get`, `list`, `depend`, `finish` / `acquire`, `list`, `extend`, `release`                    |
| `provenance`           | `submit`, `list`                                                                                        |
| `events`               | `replay`, `subscribe` (same as `client.subscribeEvents`)                                                |
| `jobs`                 | `claim`, `heartbeat`, `complete`, `fail`, `get`, `list`, `candidate`                                    |
| `admin`                | `policyGet`, `policySet`, `policyHistory`, `pin`, `pins`, `unpin`, `audit`, `auditVerify`, `gc`, `fsck` |
| `credentials`, `roles` | `mint`, `list`, `revoke` / `grant`, `list`, `revoke`                                                    |

Beyond the generated methods: `client.call(operationId, request)`, `client.paginate(operationId, request)` and
`client.pages(...)` for the paginated listings, `client.subscribeEvents(...)`, `client.withRun(runId)` and
`client.withToken(token)`. The exported `OPERATIONS` table lists every route, with its HTTP method, path and retry
class. `Req<'proposal.create'>` and `Res<'proposal.create'>` give the request and response type of any operation, and
every message type is exported by name (`Proposal`, `IntegrationOutcome`, ...).

Client options: `baseUrl`, `token` (a string, or a function returning a fresh one for each request), `fetch`, `runId`,
`requestTimeoutMs` (default 30 s per attempt), `retry`, `headers`. Per call: `signal`, `timeoutMs`, `idempotencyKey`,
`runId`, `headers`, `retry`, `requestId`.

## An agent run

An agent has its own Ed25519 key, works inside a _run_ (the server then attributes every request to
`agent:<name>#<run>`), proposes changes, and signs a provenance record that the server verifies against the key
registered for the agent. A policy can require that record before a change may land; humans approve and integrate.

<!-- example: sdk/typescript/examples/agent.ts#agent-setup -->

```ts
// An operator creates the repository, requires signed provenance from agents, and gives the agent a
// short-lived credential that is limited to this repository.
const repo = `demo/agent-${Date.now()}`;
await admin.repos.create({ name: repo });
await admin.admin.policySet({
  repo,
  stream: 'main',
  document: JSON.stringify({ require_provenance_for_agents: true, approvals: { min_for_agents: 1 } }),
});
await admin.roles.grant({ subject: 'agent:docs-bot', repo, role: 'writer' });
await admin.roles.grant({ subject: 'user:reviewer', repo, role: 'reviewer' });
await admin.roles.grant({ subject: 'user:maintainer', repo, role: 'integrator' });
const minted = await admin.credentials.mint({
  subject: 'agent:docs-bot',
  ttl_secs: 900,
  scope: { repos: [repo] },
});
```

<!-- example: sdk/typescript/examples/agent.ts#agent -->

```ts
import { generateAgentKey, putFile, signProvenance } from '@agentgit/client';

const bot = new AgentGitClient({ baseUrl: url, token: minted.token });

// The agent registers the public half of its own key, then starts a run.
const key = generateAgentKey();
await bot.agents.register({ repo, name: 'docs-bot', public_key: key.publicKeyHex });
const run = await bot.agents.startRun({ repo, agent: 'docs-bot' });
console.log(run.identity); // agent:docs-bot#run_...

// Everything the agent does from here on is attributed to this run.
const work = bot.withRun(run.run_id);
await work.workspaces.create({ repo, name: 'docs', stream: 'main' });
const change = await work.workspaces.modify({
  repo,
  name: 'docs',
  message: 'document the build',
  changes: [putFile('docs/build.md', '# Building\n\nRun `make`.\n')],
});
const proposal = await work.proposals.create({ repo, stream: 'main', head: change.head, title: 'Document the build' });

// Signed provenance: who made this change, with what, and why. The server verifies the signature
// against the key registered for the agent.
const record = {
  changeset: change.head,
  agent: run.identity,
  run_id: run.run_id,
  model: 'example-model',
  rationale: 'the build steps were undocumented',
  created_at_ms: Date.now(),
};
const entry = await work.provenance.submit({ repo, record, signature: signProvenance(record, key.privateKey) });
console.log(entry.verified); // true

await work.agents.complete({ repo, run_id: run.run_id, status: 'completed', summary: 'documented the build' });
```

<!-- example: sdk/typescript/examples/agent.ts#agent-review -->

```ts
// Humans decide. The policy above holds the change until someone approves it, and only an
// integrator can land it.
const reviewer = await humanClient('user:reviewer');
const maintainer = await humanClient('user:maintainer');
const held = await maintainer.integration.integrate({ repo, proposal: proposal.id });
console.log(
  held.outcome.status,
  held.outcome.blockers.map((b) => b.code),
); // blocked [ 'insufficient_approvals' ]

await reviewer.proposals.review({ repo, id: proposal.id, decision: 'approve', body: 'looks right' });
const landed = await maintainer.integration.integrate({ repo, proposal: proposal.id });
console.log(landed.outcome.status); // integrated
```

An `integrate` that cannot proceed is **not an error**: it answers `200` with `outcome.status` `blocked`, `conflict`,
`denied` or `invalid` and a list of blockers with stable `code`s. Errors are for requests that failed.

## Requesting reviewers

On servers advertising `proposals.review_requests`, the proposal author can replace up to 20 requested reviewers. The helper reads the current list and sends it as the precondition. A conflict requires reviewing the newer list; do not blindly overwrite it. Passing an empty list clears the requests.

Requests route attention: they do not grant access, count as an approval, or change integration policy. The list remains after a new proposal revision; decisions still belong to the exact head reviewed. Before using `requested_reviewer` on a listing or pagination, negotiate and require `proposals.review_requests`: an older server may ignore an unknown query field and return a broader list. The filtering helper below performs that check.

<!-- example: sdk/typescript/examples/review-requests.ts#review-requests -->

```ts
import type { AgentGitClient } from '@agentgit/client';

/** Replace the inspected routing list. A concurrent edit needs a fresh user decision. */
export async function requestReview(client: AgentGitClient, repo: string, id: string, reviewers: string[]) {
  const current = await client.proposals.get({ repo, id });
  return client.proposals.setReviewers({
    repo,
    id,
    reviewers,
    expected_reviewers: current.requested_reviewers ?? [],
  });
}
/** Gate the filter: older servers can ignore unknown query fields and return a broader list. */
export async function* reviewsRequestedFrom(client: AgentGitClient, repo: string, reviewer: string) {
  const hello = await client.negotiate();
  if (!hello.features.includes('proposals.review_requests'))
    throw new Error('This server does not support requested-reviewer filtering.');
  yield* client.paginate('proposal.list', { repo, requested_reviewer: reviewer });
}
```

## Errors, retries, idempotency and deadlines

<!-- example: sdk/typescript/examples/errors.ts#errors -->

```ts
import { AgentGitError } from '@agentgit/client';

const client = new AgentGitClient({
  baseUrl,
  token,
  requestTimeoutMs: 10_000, // per attempt; sent to the server as Request-Timeout
  retry: { maxAttempts: 4 }, // exponential backoff with jitter; honours Retry-After
});

try {
  await client.proposals.get({ repo: 'demo/does-not-exist', id: 'prop_1' });
} catch (error) {
  if (error instanceof AgentGitError) {
    console.log(error.code); //       "not_found": a stable, machine-readable code
    console.log(error.status); //     404
    console.log(error.retryable); //  false: the SDK did not retry it, and you should not
    console.log(error.requestId); //  the id to quote when asking an operator for the server log
  }
}

// Mutations that the server can deduplicate carry an Idempotency-Key that is generated for you and
// reused by every retry, so a lost response never creates the repository twice. Pass your own key
// to make a retry of the whole program safe too.
await client.repos.create({ name: repo }, { idempotencyKey: `create-${repo}` });
await client.repos.create({ name: repo }, { idempotencyKey: `create-${repo}` }); // returns the recorded result

// Reusing a key for a different request is an error, not a retry.
try {
  await client.repos.create({ name: `${repo}-other` }, { idempotencyKey: `create-${repo}` });
} catch (error) {
  console.log(error instanceof AgentGitError && error.code); // "idempotency_key_reused"
}

// A deadline can expire while the server is still working. The error says so: ask for the outcome
// before repeating anything that is not idempotent.
try {
  await client.streams.head(
    { repo, stream: 'main', wait_version_gt: 1000, wait_ms: 20_000 },
    { timeoutMs: 300, retry: false },
  );
} catch (error) {
  if (error instanceof AgentGitError && error.is('deadline_exceeded')) {
    console.log(error.outcomeUnknown); // true
  }
}
```

`AgentGitError` carries `code`, `status`, `retryable`, `retryAfterMs`, `requestId`, `details`, `operation`, `attempts` and
`outcomeUnknown`. Transport failures (a refused or dropped connection, a local timeout) are `AgentGitError`s too, with
`status` 0.

**What is retried.** Only errors the server marks `retryable` (or transport failures), with exponential backoff and full
jitter, never shorter than a `retry_after_ms` / `Retry-After` the server sent, and never longer than `maxRetryAfterMs`
(then the error is thrown). Whether a request may be repeated at all depends on the operation, recorded per operation in
`OPERATIONS[id].retry`:

| Class     | Meaning                                                    | Operations (examples)                                                                                                           | Retried on                                                                                                      |
| --------- | ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `read`    | only reads                                                 | every `GET`, `history.ancestors`, `tree.diff`                                                                                   | any retryable error                                                                                             |
| `natural` | repeating has the same effect                              | `objects.put`, `lease.release`, `stream.cas`, `proposal.close`                                                                  | any retryable error                                                                                             |
| `keyed`   | the server records the outcome under the `Idempotency-Key` | `repo.create`, `objects.publish`, `proposal.create`, `integration.integrate`, `task.create`, `agent.start_run`, `lease.acquire` | any retryable error; the **same key** is sent again                                                             |
| `unsafe`  | a repeat may be applied twice                              | `workspace.create`, `stream.create`, `credentials.mint`, `provenance.submit`, ...                                               | only `rate_limited` (the server rejected it before running it), or when you pass `retry: { retryUnsafe: true }` |

`workspace.modify` becomes `natural` when the request carries `expected_head` (a second application is refused).
`stream_moved` is never retried automatically: the identical compare-and-swap cannot win; read the stream again.
`outcomeUnknown` is set when the request may have been applied (a deadline, a lost connection): ask
`client.operations.get({ repo, idempotency_key })` for the outcome of a keyed operation instead of repeating it.

The tests in `test/retries.test.ts` show these rules against a real server with faults injected between the client and
the server, including that a response lost after `repo.create`, `proposal.create`, `integration.integrate`,
`task.create`, `agent.start_run` or `lease.acquire` ran does not create or integrate anything twice, and that a lost
`credentials.mint` (whose secret the server can show only once, so it ignores the key) is reported, not repeated.

`retryable: true` in an error means that doing the _logical operation_ again may succeed (after waiting, after
re-reading state, or after finding out whether the first attempt took effect). It does not mean that sending the same
bytes again is safe; that is what the classes above decide.

## Events

<!-- example: sdk/typescript/examples/events.ts#events -->

```ts
const controller = new AbortController();
const seen: string[] = [];

// afterSeq: 0 replays the whole history first; leave it out to start from "now".
// topics are prefixes: "proposal" matches proposal.create, "stream" matches stream.integrate.
const feed = client.subscribeEvents({
  repo,
  afterSeq: 0,
  topics: ['proposal', 'stream'],
  signal: controller.signal,
});

// Something happens in the repository while we listen ...
const work = (async () => {
  await client.workspaces.create({ repo, name: 'w', stream: 'main' });
  const change = await client.workspaces.modify({ repo, name: 'w', changes: [putFile('a.txt', 'a\n')] });
  const proposal = await client.proposals.create({ repo, stream: 'main', head: change.head, title: 'A' });
  await client.integration.integrate({ repo, proposal: proposal.id });
})();

// ... and each event arrives exactly once, in order, with a strictly increasing seq.
for await (const event of feed) {
  console.log(event.seq, event.topic, event.payload['target']);
  seen.push(event.topic);
  if (event.topic === 'stream.integrate') controller.abort(); // ends the loop quietly
}
await work;
```

`subscribeEvents` streams with `fetch`, so the `Authorization` header works (a browser `EventSource` cannot send one).
When the connection drops, the server closes the stream, or nothing (not even a heartbeat) arrives for `idleTimeoutMs`,
it reconnects with `after_seq` set to the last delivered `seq`; it never yields a `seq` twice and never yields
heartbeats. Without `afterSeq` the current end of the log is read first, so events appended while the client is
reconnecting are not skipped. A non-retryable error (an expired credential, a repository you may not read) is thrown from
the iterator. Breaking out of the loop closes the connection.

## Content ids, signatures and changesets

<!-- example: sdk/typescript/examples/signing.ts#signing -->

```ts
import { blobId, buildChangeset, generateAgentKey, signProvenance, verifyProvenance } from '@agentgit/client';

// The content id of a file: SHA-256 over "agentgit.object\0" || kind || version || length || bytes.
console.log(blobId('hello\n')); // 6fdfa63ccd7aa33065dcfab8c0f7b1b605226c87833b50de3345f921122a4545

// Agents sign provenance records with their own Ed25519 key. The signature covers the record as
// compact JSON with sorted keys, under the domain "provenance.v1" - exactly what the server verifies.
const key = generateAgentKey();
const record = {
  changeset: 'ab'.repeat(32),
  agent: 'agent:docs-bot#run_0123456789abcdef0123',
  run_id: 'run_0123456789abcdef0123',
  model: 'example-model',
  created_at_ms: 1_760_659_200_000,
};
const signature = signProvenance(record, key.privateKey);
console.log(verifyProvenance(record, signature, key.publicKey)); // true
console.log(verifyProvenance({ ...record, model: 'other' }, signature, key.publicKey)); // false

// A client that has no use for virtual workspaces can encode ChangesetV1 itself. The id is the same
// one the server derives from the uploaded bytes.
const changeset = buildChangeset({
  namespace: 'demo/hello',
  author: 'user:alice',
  timestampMs: 1_760_659_200_000,
  message: 'add a greeting',
  ops: [{ op: 'put', path: 'README.md', content: '# hello\n' }],
});
console.log(changeset.id, changeset.blobs.length); // <64 hex> 1
// Upload with client.objects.upload({ repo, objects: [...changeset.blobs, changeset.object] }),
// then client.proposals.create({ repo, stream: 'main', head: changeset.id, title: '...' }).
```

These helpers are checked against the independent reference implementation in
[`tests/conformance/`](../../tests/conformance/) (blob ids, `ChangesetV1` payloads and ids, tree ids, attestation signing
bytes), against provenance vectors produced by plain `json` + OpenSSL
([`sdk/testdata`](../testdata/)), and the signatures against a real server, which accepts a valid record and rejects a
tampered one. A complete client-side upload:

<!-- example: sdk/typescript/examples/objects.ts#objects -->

```ts
import { buildChangeset } from '@agentgit/client';

const info = await client.repos.describe({ repo }); // the namespace is part of every changeset hash
const me = await client.session.whoami();
const parent = (await client.streams.head({ repo, stream: 'main' })).head; // "" while the stream is empty

const built = buildChangeset({
  namespace: info.namespace,
  parents: parent ? [parent] : [],
  author: me.identity,
  timestampMs: Date.now(),
  message: 'add docs',
  ops: [
    { op: 'put', path: 'docs/guide.md', content: '# guide\n' },
    { op: 'put', path: 'docs/copy.md', content: '# guide\n' }, // same bytes: stored once
  ],
});

// Blobs first, then the changeset that names them. Objects the server already has are skipped.
const upload = await client.objects.upload({ repo, objects: [...built.blobs, built.object] });
console.log(upload.stored.length, upload.alreadyPresent.length); // 2 0

// The server derives the same id from the bytes, so the id can be proposed directly.
const proposal = await client.proposals.create({ repo, stream: 'main', head: built.id, title: 'Add docs' });
const landed = await client.integration.integrate({ repo, proposal: proposal.id });
console.log(landed.outcome.status); // integrated
```

## Development

```bash
npm ci
npm run generate         # rewrite src/generated/ from docs/protocol/openapi.json
npm run generate:check   # CI: fails if the checked-in files are stale
npm run typecheck        # tsc --noEmit (sources, tests, examples; and sources alone without Node types)
npm run lint             # ESLint (flat config) + Prettier
npm test                 # vitest; every file starts its own ag-server on a random loopback port
npm run verify:package   # build, then load ESM and CJS, compile .d.ts for both, check the tarball
npm run check            # all of the above
```

The tests start the server binary found in `$AG_SERVER_BIN`, `/home/user/bin/ag-server`, or
`target/{debug,release}/ag-server` of the repository (build it with `cargo build -p agentgit-server`). Each test file gets
its own process and temporary data directory, and the process is killed when the tests end, whether they pass or not.

`src/generated/` holds `schema.ts` (message types by [`openapi-typescript`](https://github.com/openapi-ts/openapi-typescript)),
`models.ts` (one exported alias per message), `operations.ts` (the route table) and `api.ts` (the typed namespaces).
`sdk/api-policy.json` carries what the OpenAPI document does not: the retry class of each operation (its `bodies` section, for responses the document describes wrongly, is empty today) and the message fields the server always fills in (typed as present; `test/server-contract.test.ts` fails if the
server ever answers `null` for one).

## Things the server does today that show through

Checked by `test/server-gaps.test.ts` against `ag-server`, which fails if any of this changes. The SDK has no
workaround for the behaviour the server has since fixed (repeated query keys, `after_seq` beyond the end, real sizes in
listings, the error envelope on every route, `regular` file types); these are what remains:

- `stream.create` and `credentials.mint` ignore the `Idempotency-Key`, by design: a second `stream.create` answers
  `already_exists`, and a minted credential's secret can be shown only once, so a second `credentials.mint` is a second
  credential. Both are in the `unsafe` class.
- `Retry-After` is whole seconds and at least 1, while `retry_after_ms` in the body can be shorter. The client waits for
  `retry_after_ms` (plus jitter), so a 429 with `retry_after_ms: 40` is not stretched to a second.

## License

Apache-2.0. Copyright 2026 The AgentGit Authors.

### Migrate a Git repository

With an authenticated administrator client, upload a full Git bundle, inspect the report,
then publish it. Set `AGENTGIT_REPO` to a new owner/name and `AGENTGIT_BUNDLE` to the file.
Leave `AGENTGIT_PUBLISH` unset to review; set it to `1` to publish without accepting content omissions.
Keep the returned import ID if you need to resume the review later.

<!-- example: sdk/typescript/examples/migration.ts#migration -->

```ts
import { readFile } from 'node:fs/promises';
import { AgentGitClient } from '@agentgit/client';

const client = new AgentGitClient({ baseUrl: process.env['AGENTGIT_URL']!, token: process.env['AGENTGIT_TOKEN']! });
const repo = process.env['AGENTGIT_REPO']!; // a new owner/name
const plan = await client.migrations.prepare(
  { name: repo, data: new Uint8Array(await readFile(process.env['AGENTGIT_BUNDLE'] ?? 'repository.bundle')) },
  { timeoutMs: 300_000 },
);
console.log(plan.id, plan.report);
// Review the report before opting into publication. Content omissions remain blocked.
if (process.env['AGENTGIT_PUBLISH'] === '1') {
  await client.migrations.commit({ id: plan.id, accept_lossy: false });
}
```

`client.migrations.status({ id: plan.id })` resumes a report. Binary uploads keep one
idempotency key across automatic retries. See
[migration limits and what transfers](../../docs/guides/migrate-from-git.md).

### Resolve against an inspected branch revision

Negotiate `workspaces.sync_precondition` before relying on `expected_stream_head` in
`workspaces.sync`. A conflicting sync returns `stream_head`, the exact branch revision
used to prepare its conflicts. Read that immutable revision and repeat sync with the
same `expected_stream_head` plus the chosen resolutions. If the branch moved, the server
returns a precondition failure instead of applying stale choices. Read and decide again;
do not silently retry with a newer head.

A resolution can retain an existing `blob_id`, `file_type` and `executable` value without
fetching or re-encoding its bytes. `op: delete` removes the path. Older servers omit
`stream_head`; they must be upgraded before using the pinning guarantee.

Workspace sync is retried automatically only when `expected_stream_head` is nonempty.
An unpinned sync can apply resolutions against a moving branch, so a lost response
leaves its outcome unknown and is not retried by default. Explicit unsafe-retry opt-in
remains available for callers that deliberately accept that risk.

Private review drafts use `client.proposals.getDraft`, `saveDraft` and `clearDraft`.
Negotiate `proposals.review_drafts` first. Save the complete draft with the `expected_version`
returned by the last read; clear it with the same guard. A conflict preserves the newer draft.
Drafts belong to the authenticated actor, not an owner parameter; they never post a review,
count as approval or notify reviewers. Clearing removes the content and retains a version
so stale saves cannot recreate it. Retrying an identical save/clear is safe; reread before
changing a conflicting draft rather than automatically overwriting it.

Servers advertising `workspaces.conflict_origins` may include `conflict.origin` with
`base`, `ours`, and `theirs` source paths when a rename moved the file. During workspace
sync, read `origin.ours` at the workspace head and `origin.theirs` at `stream_head`;
resolve at `conflict.path`. Without origin metadata, both sides use `conflict.path`.
For proposal preparation and integration, ours is the target stream and theirs is the proposal.

With `workspaces.structural_preview`, fresh workspace sync conflicts may include
`resolution_scope.paths` for a file/directory conflict. The scope is bounded; only
`complete: true` identifies a complete set. An absent scope or `complete: false`
must not authorize deleting unlisted paths or treating the listed paths as exhaustive.
