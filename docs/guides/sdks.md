# Client SDKs

AgentGit has two client SDKs for the [AGWP v1](../protocol/agwp-v1.md) HTTP/JSON API, in
[`sdk/typescript`](../../sdk/typescript/README.md) (`@agentgit/client`) and
[`sdk/python`](../../sdk/python/README.md) (`agentgit-client`, imported as `agentgit`). They cover **every operation**
of [`openapi.json`](../protocol/openapi.json), with the same names and the same rules, and add what a protocol
description cannot: retries that cannot apply a request twice, deadlines, pagination, an event feed that resumes, and the
helpers for content ids and signatures.

Both are **alpha (0.1)** and **not published** to npm or PyPI yet; install them from a checkout. Both are tested against a
real `ag-server` binary (never a mock of it), and only what those tests show is claimed here.

| | TypeScript | Python |
|---|---|---|
| Package | `@agentgit/client` | `agentgit-client` (`import agentgit`) |
| Runtime | Node 20+ (tests: 20.20 and 22.22); written with `fetch` only, not run in a browser here | Python 3.10+ (tests: 3.10 to 3.13) |
| Dependencies | none (Ed25519 and SHA-256 from two MIT libraries are bundled) | `httpx`, `cryptography` |
| Clients | `AgentGitClient` | `AgentGitClient` and `AsyncAgentGitClient` |
| Types | generated from `openapi.json` by `openapi-typescript` | dataclasses generated from `openapi.json` by `scripts/generate.py` |
| Static checks | `tsc --strict`, ESLint, Prettier | `mypy --strict`, `ruff` |
| Tests | `vitest` | `pytest` |

## The same call in both

Operations are grouped into namespaces named after the operation ids: `proposal.create` is
`client.proposals.create`, `history.merge_base` is `client.history.mergeBase` in TypeScript and
`client.history.merge_base` in Python. The namespaces are `session`, `repos`, `workspaces`, `proposals`, `integration`,
`queue`, `streams`, `history`, `tree`, `objects`, `operations`, `agents`, `tasks`, `leases`, `provenance`, `events`,
`jobs`, `admin`, `credentials` and `roles`. TypeScript takes one flat request object shaped like the protocol message;
Python takes the same fields as keyword arguments.

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

<!-- example: sdk/python/examples/quickstart.py#quickstart -->

```python
import os
import time

from agentgit import AgentGitClient, put_file

client = AgentGitClient(os.environ["AGENTGIT_URL"], os.environ["AGENTGIT_TOKEN"])
client.negotiate(client_name="quickstart")  # checks that the server speaks agwp.v1

repo = f"demo/hello-{int(time.time() * 1000)}"
client.repos.create(name=repo)

# Change files on the server, in a virtual workspace: no checkout needed.
client.workspaces.create(repo=repo, name="hello", stream="main")
change = client.workspaces.modify(
    repo=repo,
    name="hello",
    message="add a greeting",
    changes=[put_file("README.md", "# hello\n"), put_file("src/main.rs", "fn main() {}\n")],
)

# Propose it, and land it on the stream "main".
proposal = client.proposals.create(repo=repo, stream="main", head=change.head, title="Add a greeting")
result = client.integration.integrate(repo=repo, proposal=proposal.id)
print(result.outcome.status)  # integrated

# Read it back.
readme = client.tree.file(repo=repo, path="README.md", rev="main")
print(readme.decode())  # "# hello"
for changeset in client.paginate("history.log", repo=repo, rev="main"):
    print(changeset.id[:12], changeset.message)
```

## An agent run

An agent registers an Ed25519 public key, starts a run, works in a virtual workspace as that run, and signs a provenance
record; the server checks the signature against the registered key. With `require_provenance_for_agents` in the stream
policy, the change cannot land without it, and a human still has to approve and integrate.

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

<!-- example: sdk/python/examples/agent.py#agent -->

```python
bot = AgentGitClient(url, minted.token)

# The agent registers the public half of its own key, then starts a run.
key = generate_agent_key()
bot.agents.register(repo=repo, name="docs-bot", public_key=key.public_key_hex)
run = bot.agents.start_run(repo=repo, agent="docs-bot")
print(run.identity)  # agent:docs-bot#run_...

# Everything the agent does from here on is attributed to this run.
work = bot.with_run(run.run_id)
work.workspaces.create(repo=repo, name="docs", stream="main")
change = work.workspaces.modify(
    repo=repo,
    name="docs",
    message="document the build",
    changes=[put_file("docs/build.md", "# Building\n\nRun `make`.\n")],
)
proposal = work.proposals.create(repo=repo, stream="main", head=change.head, title="Document the build")

# Signed provenance: who made this change, with what, and why. The server verifies the signature
# against the key registered for the agent.
record = {
    "changeset": change.head,
    "agent": run.identity,
    "run_id": run.run_id,
    "model": "example-model",
    "rationale": "the build steps were undocumented",
    "created_at_ms": int(time.time() * 1000),
}
entry = work.provenance.submit(repo=repo, record=record, signature=sign_provenance(record, key.private_key))
print(entry.verified)  # True

work.agents.complete(repo=repo, run_id=run.run_id, status="completed", summary="documented the build")
```

## Errors, retries, idempotency and deadlines

Both SDKs follow the same rules, and `OPERATIONS[id].retry` (the same expression in both) tells the class of every
operation. The classes are defined in [`sdk/api-policy.json`](../../sdk/api-policy.json), the one file that holds what
the OpenAPI document does not say; both generators read it.

* An error is an `AgentGitError` with `code`, `status`, `retryable`, `retryAfterMs` / `retry_after_ms`, `requestId` /
  `request_id`, `details`, `operation`, `attempts` and `outcomeUnknown` / `outcome_unknown`. Python raises a subclass per
  stable code (`NotFoundError`, `StreamMovedError`, ...); TypeScript has `error.is('not_found')`.
* **Only errors the server marks retryable are retried**, with exponential backoff and full jitter, never sooner than the
  server's `retry_after_ms` / `Retry-After`. Transport failures (a dropped connection, a local timeout) count as
  retryable, with `status` 0.
* **Whether a request may be repeated depends on the operation.** `read` and `natural` operations are repeated freely.
  `keyed` operations (`repo.create`, `objects.publish`, `proposal.create`, `integration.integrate`, `task.create`,
  `agent.start_run`, `lease.acquire`) carry an `Idempotency-Key` that the SDK generates and reuses on every retry, so a
  lost response does not apply them twice (the server answers a repeat with the recorded response).
  `unsafe` operations (creating a workspace or a stream, minting a credential, ...) are not retried after a lost response,
  because the server cannot deduplicate them; the exception is `rate_limited`, which the server answers before running
  anything. Pass an explicit retry setting to override.
* `retryable: true` from the server means that doing the *logical operation* again may succeed (after waiting, after
  re-reading state, or after finding out whether the first attempt took effect), not that sending the same bytes again is
  safe. That is why the class of the operation decides, and why `stream_moved` and `deadline_exceeded`, which the server
  marks retryable, are not blindly repeated (below).
* `stream_moved` is never retried: the identical compare-and-swap cannot win.
* A deadline is sent as `Request-Timeout` and enforced locally. `outcomeUnknown` / `outcome_unknown` is set when the
  request may have been applied; `operations.get` with the idempotency key tells the outcome.

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

<!-- example: sdk/python/examples/errors.py#errors -->

```python
from agentgit import (
    AgentGitClient,
    AgentGitError,
    CallOptions,
    DeadlineExceededError,
    IdempotencyKeyReusedError,
    NotFoundError,
    RetryPolicy,
)

client = AgentGitClient(
    base_url,
    token,
    request_timeout_ms=10_000,  # per attempt; sent to the server as Request-Timeout
    retry=RetryPolicy(max_attempts=4),  # exponential backoff with jitter; honours Retry-After
)

try:
    client.proposals.get(repo="demo/does-not-exist", id="prop_1")
except NotFoundError as error:  # every stable error code has its own subclass of AgentGitError
    print(error.code)  # "not_found": a stable, machine-readable code
    print(error.status)  # 404
    print(error.retryable)  # False: the SDK did not retry it, and you should not
    print(error.request_id)  # the id to quote when asking an operator for the server log

# Mutations that the server can deduplicate carry an Idempotency-Key that is generated for you and
# reused by every retry, so a lost response never creates the repository twice. Pass your own key
# to make a retry of the whole program safe too.
client.repos.create(name=repo, options=CallOptions(idempotency_key=f"create-{repo}"))
client.repos.create(name=repo, options=CallOptions(idempotency_key=f"create-{repo}"))  # the recorded result

# Reusing a key for a different request is an error, not a retry.
try:
    client.repos.create(name=f"{repo}-other", options=CallOptions(idempotency_key=f"create-{repo}"))
except IdempotencyKeyReusedError as error:
    print(error.code)  # "idempotency_key_reused"

# A deadline can expire while the server is still working. The error says so: ask for the outcome
# before repeating anything that is not idempotent.
try:
    client.streams.head(
        repo=repo,
        stream="main",
        wait_version_gt=1000,
        wait_ms=20_000,
        options=CallOptions(timeout_ms=300, retry=False),
    )
except DeadlineExceededError as error:
    print(isinstance(error, AgentGitError), error.outcome_unknown)  # True True
```

The tests that demonstrate this are `test/retries.test.ts` and `tests/test_retries.py`: they put a fault-injecting `fetch` /
`httpx` transport between the SDK and a real server, drop the response of a request the server has already processed, and
check that the repository, proposal or integration exists exactly once.

## Events

Both SDKs read `GET /v1/repos/{owner}/{name}/events` (Server-Sent Events) with a streaming HTTP client, so an
`Authorization` header works. The iterator resumes after the last delivered `seq` when the connection drops, ignores
heartbeats and never yields an event twice.

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

<!-- example: sdk/python/examples/events.py#events -->

```python
import threading

seen: list[str] = []


# Something happens in the repository while we listen ...
def work() -> None:
    client.workspaces.create(repo=repo, name="w", stream="main")
    change = client.workspaces.modify(repo=repo, name="w", changes=[put_file("a.txt", "a\n")])
    proposal = client.proposals.create(repo=repo, stream="main", head=change.head, title="A")
    client.integration.integrate(repo=repo, proposal=proposal.id)


worker = threading.Thread(target=work)
worker.start()

# after_seq=0 replays the whole history first; leave it out to start from "now".
# topics are prefixes: "proposal" matches proposal.create, "stream" matches stream.integrate.
feed = client.subscribe_events(repo=repo, after_seq=0, topics=["proposal", "stream"])

# ... and each event arrives exactly once, in order, with a strictly increasing seq.
for event in feed:
    print(event.seq, event.topic, event.payload["target"])
    seen.append(event.topic)
    if event.topic == "stream.integrate":
        break  # closes the connection
worker.join()
```

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

<!-- example: sdk/python/examples/signing.py#signing -->

```python
from agentgit import PutOp, blob_id, build_changeset, generate_agent_key, sign_provenance, verify_provenance

# The content id of a file: SHA-256 over "agentgit.object\0" || kind || version || length || bytes.
print(blob_id("hello\n"))  # 6fdfa63ccd7aa33065dcfab8c0f7b1b605226c87833b50de3345f921122a4545

# Agents sign provenance records with their own Ed25519 key. The signature covers the record as
# compact JSON with sorted keys, under the domain "provenance.v1" - exactly what the server verifies.
key = generate_agent_key()
record = {
    "changeset": "ab" * 32,
    "agent": "agent:docs-bot#run_0123456789abcdef0123",
    "run_id": "run_0123456789abcdef0123",
    "model": "example-model",
    "created_at_ms": 1_760_659_200_000,
}
signature = sign_provenance(record, key.private_key)
print(verify_provenance(record, signature, key.public_key))  # True
print(verify_provenance({**record, "model": "other"}, signature, key.public_key))  # False

# A client that has no use for virtual workspaces can encode ChangesetV1 itself. The id is the same
# one the server derives from the uploaded bytes.
changeset = build_changeset(
    namespace="demo/hello",
    author="user:alice",
    timestamp_ms=1_760_659_200_000,
    message="add a greeting",
    ops=[PutOp("README.md", content="# hello\n")],
)
print(changeset.id, len(changeset.blobs))  # <64 hex> 1
# Upload with client.objects.upload(repo=repo, objects=changeset.objects()),
# then client.proposals.create(repo=repo, stream="main", head=changeset.id, title="...").
```

Provenance signatures are Ed25519 over `"agentgit.sig.v1\0" || len(domain) || domain || payload`, with the payload the
record as compact JSON with keys sorted by code point at every level (see [Signing formats](../protocol/agwp-v1.md#signing-formats)).
The helpers are checked three ways: against the independent reference implementation in
[`tests/conformance`](../../tests/conformance/) (blob ids, changeset and tree ids, attestation signing bytes), against
provenance vectors that [`sdk/testdata/gen_signing_vectors.py`](../../sdk/testdata/gen_signing_vectors.py) produces with
only `json` and OpenSSL (both SDKs reproduce the signatures bit for bit), and by a live server that accepts a valid
record and refuses a tampered one.

## Content search at one revision

Both generated HTTP SDKs expose `tree.search`. Start with a branch or changeset in `rev` and a literal `query`; optional `path_prefix` and `case_sensitive` refine it. For each continuation, reuse the first response's resolved `rev`, the same search inputs and the returned `next_page_token`. A generic pagination helper should be started with an already-resolved immutable revision. A changed branch or query is rejected instead of mixing snapshots.

A page can have no matches and still contain a continuation token because scanning is bounded. Read the `skipped_binary` and `skipped_large` counters before treating the result as exhaustive. See the [protocol contract](../protocol/agwp-v1.md#literal-content-search) for the exact limits.

## Several repositories in one event feed

On servers advertising `events.multi_repo`, replay and subscribe can take an explicit
set of up to thirty repository names. Reuse the returned opaque cursor with the same
repository and topic set. Empty cursor starts at the current positions; `from_start`
requests historical replay. A checkpoint envelope has no event and should still have
its cursor saved. Events have independent per-repository sequence numbers, so do not
sort or deduplicate them using one global sequence. Both SDKs reuse their existing
subscription transport and reconnect from the latest cursor.

## Regenerating the types

The messages come from `docs/protocol/openapi.json`, which `ag-server` serves at `/openapi.json` and a Rust test keeps in
sync with the router. After it changes:

```bash
cd sdk/typescript && npm ci && npm run generate        # src/generated/{schema,models,operations,api}.ts
cd sdk/python && uv sync && uv run python scripts/generate.py   # src/agentgit/_generated/{models,operations,namespaces}.py
```

CI can run `npm run generate:check` and `python scripts/generate.py --check`: they fail when the checked-in files are
stale. The Python tests also fail when a schema of the OpenAPI document has no dataclass, and both test suites fail when an
operation has no client method. A new operation also needs a namespace entry or a retry class in
`sdk/api-policy.json` only if the defaults are wrong (a mutation without a class is treated as `unsafe`).

## Running the tests

```bash
cd sdk/typescript && npm ci && npm run check     # generated types, tsc, ESLint + Prettier, tests, package check
cd sdk/python && uv sync && uv run pytest && uv run mypy && uv run ruff check . && uv run ruff format --check .
python3 sdk/tools/sync_doc_samples.py --check    # the code blocks of this guide and of both READMEs match the examples
```

The tests need a built `ag-server`; they use `$AG_SERVER_BIN`, then `/home/user/bin/ag-server`, then
`target/debug/ag-server` or `target/release/ag-server` (`cargo build -p agentgit-server`). Every test file or module starts its
own server on `127.0.0.1` with a random port and a temporary data directory, and kills it at the end, also when a test
fails. Nothing leaves the loopback interface.

A binary older than the routes the tests call fails them with `no such route`: build `ag-server` from the same revision as the
OpenAPI document.

Every code block above is a copy of a region of a program in `sdk/*/examples/`; those programs run in the test suites
(`test/examples.test.ts`, `tests/test_examples.py`), and `sdk/tools/sync_doc_samples.py` rewrites the blocks when an example
changes.

## What the SDKs work around

Very little. The server contract the SDKs rely on is pinned by `test/server-gaps.test.ts` / `tests/test_server_gaps.py`
(a failure there means the server changed), and most of what an earlier `ag-server` did differently from its protocol
documents has been fixed on the server: seven creating operations honour `Idempotency-Key`, `GET /v1/capabilities` takes
repeated keys, `events.replay` beyond the end returns nothing and the real `latest_seq`, listings carry real sizes, every
route answers in the error envelope with the caller's request id, and file types are `regular` / `symlink`. What remains:

* `stream.create` and `credentials.mint` ignore `Idempotency-Key`, by design (a repeated `stream.create` answers
  `already_exists`; a minted secret can be shown only once), so both are `unsafe` and a lost response is reported with
  `outcomeUnknown`, not repeated;
* `Retry-After` is whole seconds and at least 1, while `retry_after_ms` can be shorter: the SDKs wait for
  `retry_after_ms`;
* the rate limit now applies to every authenticated operation, not only to those of one repository, so any call can
  answer `429 rate_limited`; the retry rules handle it for every class, since the server rejects before running anything.

## Not covered

gRPC (the SDKs speak the HTTP/JSON gateway only), browsers (the TypeScript code uses only standard `fetch` APIs but has not
been run in one), and TLS and OIDC against a real provider. The SDKs have no
telemetry and make no requests other than the ones you ask for.
