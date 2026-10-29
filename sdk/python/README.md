# agentgit-client

Python client for [AgentGit](https://github.com/Paffin/AgentGit)'s wire protocol (AGWP v1, HTTP/JSON + SSE):
repositories, virtual workspaces, proposals, integration, agents with runs, leases and **signed provenance**, the
execution plane, and the event feed. Import name: `agentgit`.

* **Sync and async with one surface.** `AgentGitClient` (on `httpx.Client`) and `AsyncAgentGitClient` (on
  `httpx.AsyncClient`) expose every operation of
  [`docs/protocol/openapi.json`](../../docs/protocol/openapi.json), grouped into namespaces named after the operation
  ids (`proposal.create` is `client.proposals.create(...)`, `history.merge_base` is `client.history.merge_base(...)`).
* **Typed.** Every message is a dataclass in `agentgit.models`, generated from that document; every method takes
  keyword arguments and returns a model. `py.typed` is shipped; the package type-checks under `mypy --strict`.
* **Two runtime dependencies:** `httpx` and `cryptography`.
* **Python 3.10 or newer.** CI runs the test suite on 3.10, 3.11, 3.12 and 3.13 (the full lint and type checks on the default interpreter only); newer versions are not run.
* **Early beta (0.3).** Not published to PyPI yet: install from a checkout (`pip install ./sdk/python`, or `uv sync` in
  `sdk/python` for development). It is exercised by the tests in this directory against a real `ag-server`.

```bash
pip install agentgit-client   # once published; today: pip install ./sdk/python
```

## Quick start

Needs a running `ag-server` (`AG_BOOTSTRAP_TOKEN=... ag-server serve`) and a credential. With the bootstrap token the
same script plays every role.

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

Parameters are the protocol's field names (`repo` is `"owner/name"`); a Python keyword gets a trailing underscore
(`client.tree.diff(repo=..., from_=..., to=...)`). Fields you leave out take the protocol's zero value. Nested
messages can be dataclasses (`models.FileChange`) or plain dicts.

The same with asyncio:

<!-- example: sdk/python/examples/async_quickstart.py#async -->

```python
import asyncio
import os
import time

from agentgit import AsyncAgentGitClient, put_file


async def main() -> str:
    async with AsyncAgentGitClient(os.environ["AGENTGIT_URL"], os.environ["AGENTGIT_TOKEN"]) as client:
        repo = f"demo/async-{int(time.time() * 1000)}"
        await client.repos.create(name=repo)
        await client.workspaces.create(repo=repo, name="hello", stream="main")
        change = await client.workspaces.modify(repo=repo, name="hello", changes=[put_file("a.txt", "a\n")])
        proposal = await client.proposals.create(repo=repo, stream="main", head=change.head, title="A")
        result = await client.integration.integrate(repo=repo, proposal=proposal.id)

        # Listings are async iterators; so is the event feed.
        async for changeset in client.paginate("history.log", repo=repo, rev="main"):
            print(changeset.id[:12], changeset.message)
        return result.outcome.status


status = asyncio.run(main())
print(status)  # integrated
```

## The API at a glance

| Namespace | Methods |
|---|---|
| `session` | `negotiate`, `auth_config`, `whoami` (and `client.negotiate()`, which also checks the protocol version) |
| `repos` | `create`, `describe`, `list`, `configure` |
| `workspaces` | `create`, `get`, `list`, `modify`, `sync`, `delete` |
| `proposals` | `create`, `get`, `list`, `update`, `set_reviewers`, `review`, `close` |
| `integration` | `prepare`, `validate`, `integrate`, `attest`, `attestations` |
| `queue` | `enqueue`, `list`, `run` |
| `streams` | `list`, `create`, `head`, `cas`, `reflog` |
| `history` | `resolve`, `log`, `changeset`, `ancestors`, `merge_base`, `blame` |
| `tree` | `get`, `list`, `resolve`, `diff`, `file` (returns `bytes`) |
| `objects` | `missing`, `put` (bytes), `get` (bytes), `verify`, `publish`, plus `put_blob` and `upload` |
| `operations` | `get` (the recorded outcome of an operation, by operation id or idempotency key) |
| `agents` | `register`, `list`, `start_run`, `list_runs`, `heartbeat`, `complete`, `predict` |
| `tasks`, `leases` | `create`, `get`, `list`, `depend`, `finish` / `acquire`, `list`, `extend`, `release` |
| `provenance` | `submit`, `list` |
| `events` | `replay`, `subscribe` (same as `client.subscribe_events`) |
| `jobs` | `claim`, `heartbeat`, `complete`, `fail`, `get`, `list`, `candidate` |
| `admin` | `policy_get`, `policy_set`, `policy_history`, `pin`, `pins`, `unpin`, `audit`, `audit_verify`, `gc`, `fsck` |
| `credentials`, `roles` | `mint`, `list`, `revoke` / `grant`, `list`, `revoke` |

Beyond the generated methods: `client.call(operation_id, **params)`, `client.paginate(operation_id, **params)` and
`client.pages(...)` for the paginated listings, `client.subscribe_events(...)`, `client.with_run(run_id)` and
`client.with_token(token)` (both share the connection pool). `agentgit.OPERATIONS` lists every route with its HTTP
method, path and retry class.

Client arguments: `base_url`, `token` (a string, or a function returning a fresh one for each request), `run_id`,
`request_timeout_ms` (default 30 s per attempt), `retry` (`RetryPolicy`, or `False`), `headers`, `http_client` (your
own `httpx.Client`/`httpx.AsyncClient`: proxies, TLS, a test transport). Per call, pass
`options=CallOptions(timeout_ms=..., idempotency_key=..., run_id=..., headers=..., retry=..., request_id=...)`. Use the
clients as context managers (`with` / `async with`) or call `close()` / `aclose()`.

## An agent run

An agent has its own Ed25519 key, works inside a *run* (the server then attributes every request to
`agent:<name>#<run>`), proposes changes, and signs a provenance record that the server verifies against the key
registered for the agent. A policy can require that record before a change may land; humans approve and integrate.

<!-- example: sdk/python/examples/agent.py#agent-setup -->

```python
# An operator creates the repository, requires signed provenance from agents, and gives the agent a
# short-lived credential that is limited to this repository.
repo = f"demo/agent-{int(time.time() * 1000)}"
admin.repos.create(name=repo)
admin.admin.policy_set(
    repo=repo,
    stream="main",
    document=json.dumps({"require_provenance_for_agents": True, "approvals": {"min_for_agents": 1}}),
)
admin.roles.grant(subject="agent:docs-bot", repo=repo, role="writer")
admin.roles.grant(subject="user:reviewer", repo=repo, role="reviewer")
admin.roles.grant(subject="user:maintainer", repo=repo, role="integrator")
minted = admin.credentials.mint(subject="agent:docs-bot", ttl_secs=900, scope={"repos": [repo]})
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

<!-- example: sdk/python/examples/agent.py#agent-review -->

```python
# Humans decide. The policy above holds the change until someone approves it, and only an
# integrator can land it.
reviewer = human("user:reviewer")
maintainer = human("user:maintainer")
held = maintainer.integration.integrate(repo=repo, proposal=proposal.id)
print(held.outcome.status, [b.code for b in held.outcome.blockers])  # blocked ['insufficient_approvals']

reviewer.proposals.review(repo=repo, id=proposal.id, decision="approve", body="looks right")
landed = maintainer.integration.integrate(repo=repo, proposal=proposal.id)
print(landed.outcome.status)  # integrated
```

An `integrate` that cannot proceed is **not an error**: it answers `200` with `outcome.status` `blocked`, `conflict`,
`denied` or `invalid` and a list of blockers with stable `code`s. Errors are for requests that failed.

## Requesting reviewers

On servers advertising `proposals.review_requests`, the proposal author can replace up to 20 requested reviewers. The helper reads the current list and sends it as the precondition. A conflict requires reviewing the newer list; do not blindly overwrite it. Passing an empty list clears the requests.

Requests route attention: they do not grant access, count as an approval, or change integration policy. The list remains after a new proposal revision; decisions still belong to the exact head reviewed. Before using `requested_reviewer` on a listing or pagination, negotiate and require `proposals.review_requests`: an older server may ignore an unknown query field and return a broader list. The filtering helper below performs that check.

<!-- example: sdk/python/examples/review_requests.py#review-requests -->

```python
from collections.abc import Iterator

from agentgit import AgentGitClient, models


def request_review(
    client: AgentGitClient, repo: str, id: str, reviewers: list[str]
) -> models.RequestedReviewers:
    """Replace the inspected routing list; let the caller resolve a concurrent edit."""
    current = client.proposals.get(repo=repo, id=id)
    return client.proposals.set_reviewers(
        repo=repo,
        id=id,
        reviewers=reviewers,
        expected_reviewers=current.requested_reviewers,
    )


def reviews_requested_from(client: AgentGitClient, repo: str, reviewer: str) -> Iterator[models.Proposal]:
    """Check support before filtering: old servers may ignore unknown query fields."""
    hello = client.negotiate()
    if "proposals.review_requests" not in hello.features:
        raise ValueError("This server does not support requested-reviewer filtering.")
    yield from client.paginate("proposal.list", repo=repo, requested_reviewer=reviewer)
```

## Errors, retries, idempotency and deadlines

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

`AgentGitError` has one subclass per stable error code (`NotFoundError`, `StreamMovedError`, `RateLimitedError`, ...;
unknown codes raise the base class) and carries `code`, `status`, `retryable`, `retry_after_ms`, `request_id`, `details`,
`operation`, `attempts` and `outcome_unknown`. Transport failures (a refused or dropped connection, a local timeout)
are `UnavailableError` / `DeadlineExceededError` with `status == 0`.

**What is retried.** Only errors the server marks `retryable` (or transport failures), with exponential backoff and full
jitter, never shorter than a `retry_after_ms` / `Retry-After` the server sent, and never longer than
`RetryPolicy.max_retry_after_ms` (then the error is raised). Whether a request may be repeated at all depends on the
operation, recorded per operation in `OPERATIONS[id].retry`:

| Class | Meaning | Operations (examples) | Retried on |
|---|---|---|---|
| `read` | only reads | every `GET`, `history.ancestors`, `tree.diff` | any retryable error |
| `natural` | repeating has the same effect | `objects.put`, `lease.release`, `stream.cas`, `proposal.close` | any retryable error |
| `keyed` | the server records the outcome under the `Idempotency-Key` | `repo.create`, `objects.publish`, `proposal.create`, `integration.integrate`, `task.create`, `agent.start_run`, `lease.acquire` | any retryable error; the **same key** is sent again |
| `unsafe` | a repeat may be applied twice | `workspace.create`, `stream.create`, `credentials.mint`, `provenance.submit`, ... | only `rate_limited` (the server rejected it before running it), or with `RetryPolicy(retry_unsafe=True)` |

`workspace.modify` becomes `natural` when the request carries `expected_head` (a second application is refused).
`stream_moved` is never retried automatically: the identical compare-and-swap cannot win; read the stream again.
`outcome_unknown` is set when the request may have been applied (a deadline, a lost connection): ask
`client.operations.get(repo=..., idempotency_key=...)` for the outcome of a keyed operation instead of repeating it.

`tests/test_retries.py` shows these rules against a real server with faults injected between the client and the server
(a custom `httpx` transport), including that a response lost after `repo.create`, `proposal.create`,
`integration.integrate`, `task.create`, `agent.start_run` or `lease.acquire` ran does not create or integrate anything
twice, and that a lost `credentials.mint` (whose secret the server can show only once, so it ignores the key) is
reported, not repeated.

`retryable=True` in an error means that doing the *logical operation* again may succeed (after waiting, after re-reading
state, or after finding out whether the first attempt took effect). It does not mean that sending the same bytes again
is safe; that is what the classes above decide. The deadline is the per-attempt
`request_timeout_ms`: it is sent as `Request-Timeout` and also enforced locally (`httpx` timeouts plus a total check
while the body is read).

## Events

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

`subscribe_events` streams with `httpx`, so the `Authorization` header works. When the connection drops, the server
closes the stream, or nothing (not even a heartbeat) arrives for `idle_timeout_ms`, it reconnects with `after_seq` set to
the last delivered `seq`; it never yields a `seq` twice and never yields heartbeats. Without `after_seq` the current end
of the log is read first, so events appended while the client is reconnecting are not skipped. A non-retryable error (an
expired credential, a repository you may not read) is raised from the iterator. Leaving the loop (`break`, or
`close()` on the generator; `contextlib.aclosing` for the async client) closes the connection.

## Content ids, signatures and changesets

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

These helpers are checked against the independent reference implementation in
[`tests/conformance/`](../../tests/conformance/) (blob ids, `ChangesetV1` payloads and ids, tree ids, attestation signing
bytes), against provenance vectors produced by plain `json` + OpenSSL
([`sdk/testdata`](../testdata/)), and the signatures against a real server, which accepts a valid record and rejects a
tampered one. A complete client-side upload:

<!-- example: sdk/python/examples/objects.py#objects -->

```python
from agentgit import PutOp, build_changeset

info = client.repos.describe(repo=repo)  # the namespace is part of every changeset hash
me = client.session.whoami()
parent = client.streams.head(repo=repo, stream="main").head  # "" while the stream is empty

built = build_changeset(
    namespace=info.namespace,
    parents=[parent] if parent else [],
    author=me.identity,
    timestamp_ms=int(time.time() * 1000),
    message="add docs",
    ops=[
        PutOp("docs/guide.md", content="# guide\n"),
        PutOp("docs/copy.md", content="# guide\n"),  # same bytes: stored once
    ],
)

# Blobs first, then the changeset that names them. Objects the server already has are skipped.
upload = client.objects.upload(repo=repo, objects=built.objects())
print(len(upload.stored), len(upload.already_present))  # 2 0

# The server derives the same id from the bytes, so the id can be proposed directly.
proposal = client.proposals.create(repo=repo, stream="main", head=built.id, title="Add docs")
landed = client.integration.integrate(repo=repo, proposal=proposal.id)
print(landed.outcome.status)  # integrated
```

## Development

```bash
cd sdk/python
uv sync                              # creates .venv from uv.lock
uv run python scripts/generate.py            # rewrite src/agentgit/_generated/ from docs/protocol/openapi.json
uv run python scripts/generate.py --check    # CI: fails if the checked-in files are stale
uv run ruff check . && uv run ruff format --check .
uv run mypy                          # strict
uv run pytest                        # every module starts its own ag-server on a random loopback port
python3 ../tools/sync_doc_samples.py --check   # the code blocks of the docs match the examples
```

The tests start the server binary found in `$AG_SERVER_BIN`, `/home/user/bin/ag-server`, or
`target/{debug,release}/ag-server` of the repository (build it with `cargo build -p agentgit-server`). Each test module gets
its own process and temporary data directory, and the process is killed when the tests end, whether they pass or not.

`src/agentgit/_generated/` holds `models.py` (a dataclass per message), `operations.py` (the route table) and
`namespaces.py` (the synchronous and asynchronous namespace classes). `sdk/api-policy.json` carries what the OpenAPI
document does not: the retry class of each operation (its `bodies` section, for responses the document describes wrongly, is empty today) and the message fields the server always
fills in (typed as present; `tests/test_server_contract.py` fails if the server ever answers `null` for one).

## Things the server does today that show through

Checked by `tests/test_server_gaps.py` against `ag-server`, which fails if any of this changes. The SDK has no workaround
for the behaviour the server has since fixed (repeated query keys, `after_seq` beyond the end, real sizes in listings,
the error envelope on every route, `regular` file types); these are what remains:

* `stream.create` and `credentials.mint` ignore the `Idempotency-Key`, by design: a second `stream.create` answers
  `already_exists`, and a minted credential's secret can be shown only once, so a second `credentials.mint` is a second
  credential. Both are in the `unsafe` class.
* `Retry-After` is whole seconds and at least 1, while `retry_after_ms` in the body can be shorter. The client waits for
  `retry_after_ms` (plus jitter), so a 429 with `retry_after_ms=40` is not stretched to a second.

## License

Apache-2.0. Copyright 2026 The AgentGit Authors.

### Migrate a Git repository

With an authenticated administrator client, upload a full Git bundle, inspect the report,
then publish it. Set `AGENTGIT_REPO` to a new owner/name and `AGENTGIT_BUNDLE` to the file.
Leave `AGENTGIT_PUBLISH` unset to review; set it to `1` to publish without accepting content omissions.
Keep the returned import ID if you need to resume the review later.

<!-- example: sdk/python/examples/migration.py#migration -->
```python
import os
from pathlib import Path

from agentgit import AgentGitClient, CallOptions

client = AgentGitClient(os.environ["AGENTGIT_URL"], os.environ["AGENTGIT_TOKEN"])
repo = os.environ["AGENTGIT_REPO"]  # a new owner/name
plan = client.migrations.prepare(
    name=repo,
    data=Path(os.environ.get("AGENTGIT_BUNDLE", "repository.bundle")).read_bytes(),
    options=CallOptions(timeout_ms=300_000),
)
print(plan.id, plan.report)
# Review the report before opting into publication. Content omissions remain blocked.
if os.environ.get("AGENTGIT_PUBLISH") == "1":
    client.migrations.commit(id=plan.id, accept_lossy=False)
```

`migrations.status(id=plan.id)` resumes a report. The same methods exist on
`AsyncAgentGitClient`. Binary uploads keep one idempotency key across automatic retries.
See [migration limits and what transfers](../../docs/guides/migrate-from-git.md).


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


Private review drafts use `client.proposals.get_draft`, `save_draft` and `clear_draft`
(the same names are available on the async client). Negotiate `proposals.review_drafts`
first. Save the complete draft with the `expected_version` returned by the last read;
clear it with that guard. A conflict preserves the newer draft. The authenticated actor
owns the notes; there is no owner argument. Drafts never post reviews or count as approval.
Clearing removes content but retains a version, preventing stale saves from recreating it.
Identical retries are safe; reread and decide before changing a conflicting draft.
