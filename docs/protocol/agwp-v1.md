# AGWP v1 — the AgentGit Wire Protocol

AGWP is how clients (the CLI, SDKs, agents, execution-plane workers, the Web UI) talk to `ag-server`.
It is defined **once**, in [`proto/agwp/v1/agwp.proto`](../../proto/agwp/v1/agwp.proto), and served over
two transports that share one implementation:

| Transport | For | Notes |
|---|---|---|
| **gRPC** (HTTP/2, TLS optional) | CLI, SDKs, agents, workers | streaming uploads/downloads/events; deadlines; structured error details |
| **HTTP/JSON + SSE** | browsers, scripts, any language | the same messages as JSON; described by [`openapi.json`](openapi.json) |

Both are served on **one port**. `GET /openapi.json` returns the live OpenAPI 3.1 document; the copy in this
directory is checked by a test, so it cannot drift from the server.

## Connecting

1. `GET /v1/capabilities` (gRPC: `ProtocolService.Negotiate`) needs no credentials. It returns the protocol
   version (`agwp.v1`), the features the server supports, its limits and accepted authentication methods.
   A client that only speaks another version gets `failed_precondition`.
2. Every other call carries a bearer credential: `Authorization: Bearer <token>` (gRPC metadata:
   `authorization`). See [Authentication](#authentication).
3. `GET /v1/whoami` returns who the server thinks you are, your roles and the scope of your credential.

## Conventions

* **JSON mapping.** Field names are the proto field names (`snake_case`), with one family of exceptions: a
  field that carries a JSON document inside a string in the proto (`Event.payload_json`,
  `Blocker.detail_json`, `Job.result_json`, `AuditEntry.details_json`, `CompleteJobRequest.result_json`) is
  exposed as real JSON under the name without the suffix (`payload`, `detail`, `result`, `details`). `int64`/`uint64` are JSON numbers
  (timestamps are Unix milliseconds in `*_ms`, sequence numbers and versions stay far below 2^53). `bytes`
  are standard base64. Enumerations are strings with a documented value set; clients must tolerate unknown
  values. Missing fields take their zero value; unknown fields are ignored (so old clients and new servers
  interoperate).
* **Where parameters go.** `GET`/`DELETE`: path placeholders and the query string. Other methods: a JSON
  object body (path placeholders are merged in and win). *Free-form names — streams, revisions, paths,
  pin names — travel in the query string or body, never inside the URL path*, so URL encoding cannot change
  which resource a request addresses. Repositories are `owner/name`: `/v1/repos/{owner}/{name}/…`.
  List-valued fields of a `GET` are repeated keys (`?features=a&features=b`). Fields named `file_type` hold
  `regular` or `symlink` (writes also accept `file` as an alias of `regular`); the `kind` of a directory
  entry or of a resolved path is `file`, `symlink`, `dir` (or `missing`). Tree and directory listings carry
  each file's size in bytes.
* **Idempotency.** Operations that *create* something accept `idempotency_key` (HTTP: also the
  `Idempotency-Key` header): `repo.create`, `objects.publish`, `proposal.create`, `integration.integrate`,
  `task.create`, `run.start` and `lease.acquire`. Repeating a request with the same key and content returns the
  recorded response (what the first call answered — not a fresh view of the object); the same key with
  different content is refused with `idempotency_key_reused`. For `task.create`, `run.start` and `lease.acquire`
  the record is written in the same transaction as the object, so two requests racing on one key, or a crash in
  between, never produce two objects; for these three, records are kept for `repos.idempotency_ttl_secs` (default 7 days; 0 = for the life of the repository), after which the key is free again and a periodic sweep (`repos.idempotency_sweep_interval_secs`, default hourly) deletes the record; the engine's own keyed operations (publish, propose, integrate) are kept as operations. Every other operation either is naturally
  idempotent (setting a policy, granting a role, pinning, finishing a task) or cannot replay its result — a
  minted credential's secret is shown once, so `credentials.mint` ignores the key; a repeated
  `stream.create` answers `already_exists`. After a lost response to a call without a key, ask for the outcome
  (`GET …/operations?idempotency_key=…` where a key was used) instead of blindly retrying.
* **Pagination.** `page_size` (0 = default 100, max 1000) and an opaque `page_token`; responses carry
  `next_page_token` (empty = last page). Tokens are only valid for the listing that produced them. Proposal listings are keyset pages (the token carries the sort key `created_at, id` of the last row; `state`, `stream` and `author` filter in SQL), so a walk neither skips nor repeats a proposal when others are opened or closed meanwhile. A few
  listings are capped instead of paged and say so with `truncated: true`: credentials (newest 1000), role
  bindings (5000) and the lease history (`limit`, default 100, at most 1000; active leases are never cut).
  Narrow the request (`subject`, `repo`) to see the rest. Authorization itself never reads a capped list.
* **Deadlines.** gRPC deadlines (`grpc-timeout`) or, over HTTP, `Request-Timeout: <milliseconds>`. When a
  deadline expires the operation *may still complete*: the error is `deadline_exceeded`; ask for the outcome.
  The server applies `limits.default_deadline_ms` when the client sets none.
* **Backpressure.** `429 rate_limited`/`quota_exceeded` and `503 unavailable` carry `retry_after_ms`
  (HTTP: `Retry-After`, whole seconds rounded up, at least 1, never shorter than `retry_after_ms`; and `Retry-After-Ms` with the exact value; prefer `retry_after_ms`). Every authenticated operation
  takes one token from the caller's bucket (`limits.requests_per_second`: per credential, or per identity for
  OIDC sign-ins, whose tokens are short-lived), including the ones not tied to a repository. A repository may
  carry a quota of its own: `max_total_object_bytes` is charged to every write that adds an object (uploads,
  workspace edits, merges — whichever endpoint writes it, atomically across concurrent requests and replicas;
  the tracked total is an estimate that garbage collection recounts), `max_object_bytes` bounds one object,
  `max_streams` and `max_open_proposals` bound counts (counted inside the transaction that inserts the row; write
  transactions of one repository never overlap, so racing creates cannot pass the limit), and `requests_per_second`/`burst` add a per-repository rate limit.
  New repositories start with the `[limits] repo_*` / `max_*` defaults of the server configuration.
* **`retryable`.** `retryable: true` means that doing the *logical operation* again may succeed — after
  waiting (`rate_limited`, `unavailable`), after re-reading state and redoing the change (`stream_moved`,
  `aborted`), or, for `deadline_exceeded`, after finding out whether the first attempt took effect. It does
  **not** mean that sending the same bytes again is safe: do that only for reads, for naturally idempotent
  operations, and for creations that carried an idempotency key. The SDKs implement exactly these rules.
* **Correlation.** Send `X-Request-Id` (gRPC metadata `x-request-id`); the server echoes it in
  `X-Request-Id` and in every error. A `traceparent` continues the caller's trace (exported when
  `telemetry.otlp_endpoint` is set) and its trace id is on every log line of the request (`trace_id`) whether
  or not the trace is sampled; the peer address (`remote`) is on them too, failed authentications included.
  The server does not forward `traceparent` to anything it calls.

## Errors

HTTP: status code + `{"error": ErrorInfo}`. gRPC: status + `ErrorInfo` in `grpc-status-details-bin`
(and the code in `ag-error-code` metadata).

```json
{"error": {"code": "stream_moved", "message": "…", "retryable": true, "retry_after_ms": 0,
           "request_id": "req_…", "details": {"stream": "main"}}}
```

| `code` | HTTP | gRPC | meaning / what to do |
|---|---:|---|---|
| `unauthenticated` | 401 | UNAUTHENTICATED | missing, invalid, expired or revoked credential |
| `permission_denied` | 403 | PERMISSION_DENIED | authenticated but not allowed (role or credential scope) |
| `not_found` | 404 | NOT_FOUND | also returned for repositories you may not know exist |
| `already_exists` | 409 | ALREADY_EXISTS | |
| `invalid_argument` | 400 | INVALID_ARGUMENT | malformed request; fix it |
| `failed_precondition` | 412 | FAILED_PRECONDITION | state does not allow it (e.g. proposal already integrated) |
| `stream_moved` | 409 | ABORTED | compare-and-swap lost; re-read and retry |
| `idempotency_key_reused` | 422 | FAILED_PRECONDITION | same key, different request |
| `object_missing` / `object_corrupt` | 422 | FAILED_PRECONDITION / DATA_LOSS | upload the object / bytes do not hash to the id |
| `too_large` | 413 | OUT_OF_RANGE | |
| `rate_limited` / `quota_exceeded` | 429 | RESOURCE_EXHAUSTED | wait `retry_after_ms` / free space or raise the quota |
| `unavailable` | 503 | UNAVAILABLE | transient; retry with backoff |
| `deadline_exceeded` | 504 | DEADLINE_EXCEEDED | outcome unknown; ask before retrying |
| `aborted` | 409 | ABORTED | concurrent modification; retry |
| `lease_conflict` | 409 | ABORTED | an exclusive lease is held by someone else |
| `stale_candidate` | 409 | ABORTED | the candidate tree changed since the job was created |
| `unimplemented` / `internal` | 501 / 500 | UNIMPLEMENTED / INTERNAL | |

Routes that do not exist answer `404 not_found` and methods a route does not have answer `405` (with an
`Allow` header); both use this envelope and carry the request id like every other error.

**Outcomes are not errors.** `integrate` returns `200` with `outcome.status` ∈ `integrated`, `blocked`
(something can be provided: approvals, validation, a lease), `conflict`, `denied`, `invalid`. Each blocker
has a stable `code` — see the [policy guide](../guides/policy.md#outcome-codes). A `conflict` outcome (and a
sync that cannot merge) lists `Conflict`s: each has a `path`, a stable machine-readable `kind` — one of `content`,
`both_added`, `delete_modify`, `binary`, `type_change`, `too_large`, `file_directory`, `rename_rename`,
`rename_delete`, `directory_rename_ambiguous`, `directory_rename_collision`; new kinds may be added, so a client must tolerate one it does not know — and a `message` for
people (region counts, which side deleted); a client branches on `kind` and shows `message`.

The two `directory_rename_*` kinds come from `ag-merge/3` ([ADR-0013](../adr/0013-directory-renames.md)): one side
moved a directory and the other changed files under its old name in a way the merge cannot follow without guessing.
A merge that *did* follow a move is not a conflict; it is reported as a typed note (`directory_rename`) by
`ag preview` and in the `Directory-Rename:` lines of the merge changeset message. Notes are not part of AGWP v1
messages yet (`Preparation` carries conflicts only), so the Web UI shows the conflicts and not the notes.

## Authentication

Three kinds of bearer credentials; all end up as an **identity** (`user:alice`, `agent:refactor-bot`,
`service:ci`) that the engine records as the actor of every change:

* **Scoped tokens** (`agt_…`), minted with `POST /v1/credentials` (or `ag-server admin mint`): opaque,
  stored only as a hash, always expiring (agents: ≤ 1 h by default), revocable, and **never more powerful
  than their minter** — a credential may carry a *scope* (`repos`, `streams`, `operations` patterns) that
  can only narrow, and a minted credential never outlives the credential that minted it.
* **OIDC** ID/access tokens from configured providers (RS256/ES256/EdDSA; issuer, audience, expiry and
  signature are all checked; groups map to roles).
* The **bootstrap token** (environment variable) for first-time setup.

An agent presents its credential plus `ag-run: <run_id>` (gRPC metadata `ag-run`) to act as
`agent:<name>#<run_id>` — the server checks that the run exists, belongs to that agent and is running.

**Credentials bound to a run** (feature `auth.run_credentials`, [ADR-0010](../adr/0010-run-bound-credentials.md)).
A credential for `agent:<name>` is bound to the *name* by default: it may name any run of the agent in `ag-run`. A
scope with `run_id` (and exactly one literal repository in `repos`, the one that holds the run) binds it to **one run**:

* minting needs the run to exist, be `running` and belong to the subject (`not_found`, `failed_precondition`,
  `invalid_argument`), and the minter to be allowed `agent.run` on the repository;
* the credential acts as that run - with or without `ag-run` - and only as that run (naming another is
  `permission_denied`); it cannot start runs, act on the workspaces, proposals and leases of another run, finish a task
  the run did not claim, or manage other runs' credentials; it can mint credentials only for its own run;
* when the run is no longer `running` (completed, failed, abandoned, gone) every call answers `unauthenticated`,
  open event streams end with an `error` event, and finishing the run revokes the credentials bound to it (the check at
  use time is the guarantee, the revocation is the record).

**Delegation** (feature `auth.delegation`): `POST /v1/credentials` with a subject other than your own needs
`credentials.mint` (administrators: anyone but a system identity) or `credentials.delegate` (the `delegator` role:
`agent:*` subjects only, scoped to the capabilities the caller itself holds on every repository of the scope, never the
administrative ones; an empty `operations` becomes exactly that set, a request for more is `permission_denied`).

## Authorization

Roles are bound to identities, patterns (`agent:*`) or OIDC groups, per repository pattern; a request needs
**both** a role that grants the capability **and** a credential scope that permits it.

| Role | Grants |
|---|---|
| `reader` | read everything in a repository: history, trees, streams, proposals, events, policy, audit pins |
| `writer` | reader + upload/publish, virtual workspaces, open/update/close proposals and add discussion comments, request validation, enqueue, register agents and run as an agent (start, heartbeat and end the runs of **its own** agent identity only: acting for another agent needs `agent.operate`), tasks, leases, provenance |
| `reviewer` | reader + review proposals and add discussion comments |
| `integrator` | reader + integrate proposals, request validation, run the queue, close proposals and add discussion comments |
| `runner` | reader + claim execution jobs and submit attestations (execution-plane workers) |
| `delegator` | writer + mint, list and revoke credentials for `agent:*` subjects (never for anyone else, never with a capability it lacks) + operate other agents' tasks and runs (`agent.operate`: start, heartbeat or end a run for another agent, finish a task whoever owns it, reap stale runs). For a service that starts agents; replaces `admin` for the orchestrator |
| `admin` | everything, including policy, credentials, roles, GC, fsck, stream compare-and-swap |

A caller with no role at all on a repository gets `not_found`. Authorization says *who may call which API*;
the stream **policy** inside the engine independently decides *whether a change may land* (approvals,
validations, protected paths, agent autonomy) — a role never overrides it.

## Objects and publication

Objects are content-addressed; the exact byte formats and ID computation are in
[object-format](../architecture/object-format.md). A client that builds changesets itself:

1. `POST …/objects/missing` with the ids it holds → the ids the server lacks;
2. `PUT …/objects/{id}` (binary body = the object envelope; gRPC: `ObjectService.Upload` stream). The server
   re-hashes the bytes and refuses any that do not hash to `id` (`object_corrupt`); trees cannot be uploaded;
3. `POST …/proposals` with `head` (the server publishes the unpublished closure first) or
   `POST …/publish`.

Clients without a filesystem skip all of this with **virtual workspaces**: `POST …/workspaces`, then
`POST …/workspaces/{name}/changes` with `{path, op: put|delete, content (base64)}` batches; each batch
becomes one changeset. `…/sync` merges the stream head in; on conflicts it reports them and changes
nothing until every conflicting path is resolved by passing `resolutions`.

## Signing formats

Everything signed is Ed25519 over a **domain-separated** message:

```text
message = "agentgit.sig.v1\0" ‖ len(domain):u8 ‖ domain ‖ payload        domains: "provenance.v1", "attestation.v1"
```

* **Provenance** (`POST …/provenance`): `payload` is the record as *compact JSON with keys sorted
  alphabetically at every level* and absent optional fields as `null`: `agent, changeset, checks_run,
  created_at, inputs, model, parameters, prompt_digest, rationale, run_id, schema ("agentgit.provenance.v1"),
  task_id`. (`created_at` is Unix milliseconds.) The signature is checked against the key registered for the
  agent, and only the agent itself may submit it.
* **Attestations** (workers only): the canonical binary signing bytes are specified in
  [object-format](../architecture/object-format.md#signatures).

## Events

`GET …/events` is a Server-Sent Events stream; `GET …/events/replay?after_seq=N` returns a JSON page.
Events are the repository's audit log: `seq` is strictly increasing and never reused, `topic` is the audit
action (`proposal.create`, `stream.integrate`, `lease.acquire`, `run.start`, `policy.set`, …). Resume with
`Last-Event-ID` or `after_seq`; filter with `topics=a,b` (prefix match on dotted names). Heartbeats are SSE
comments. A subscriber that was away loses nothing: a topic filter is applied while paging through the log,
so a long run of other entries never hides a matching event, and a replay page with fewer than `limit` events is
the end of the matching events up to `latest_seq`. The stream ends with an `error` event when its credential is
revoked or expires — checked before every batch is delivered, and every few seconds while idle. A query value
that is not a number (`after_seq=abc`, a bad `Last-Event-ID`) is `invalid_argument`, never a default.


### Review head precondition

Servers advertising `proposals.review_head` accept `expected_head` on review requests. A nonempty value is an exact changeset ID. The server compares it with the proposal head in the same metadata write transaction as the review and audit insertion; a mismatch returns `failed_precondition` (HTTP 412) and creates neither. An empty value preserves legacy behavior. Negotiate the capability before relying on the precondition: older servers may ignore an unknown JSON or protobuf field. This does not add idempotency to reviews. See [ADR-0014](../adr/0014-review-head-precondition.md).

### Literal content search

Servers advertising `tree.search` support `TreeService.Search` and HTTP `GET /v1/repos/{owner}/{rname}/search` under `tree.read` authorization. Parameters: `rev`, `query`, optional `case_sensitive`, file/directory `path_prefix`, `page_size` (default 50, maximum 100), and `page_token`.

Responses include the resolved `rev`, `tree_id`, matching `path`/1-based `line`/bounded `text`, `next_page_token`, and per-page `files_scanned`, `scanned_bytes`, `skipped_binary`, `skipped_large`. Continue with the returned immutable `rev` and unchanged search inputs. A continuation token is invalid for a different repository, tree, query, case mode or prefix. A token can be returned even if the page contains no matches, because each request scans at most 1000 files and 16 MiB. Invalid UTF-8/NUL-containing files and files over 1 MiB are skipped. See [ADR 0015](../adr/0015-bounded-revision-code-search.md).

### Task-scoped history

Servers advertising `tasks.history` accept `task_id` on proposal and run list
requests. The filter is applied before pagination. Run lists accept `page_token`
and return `next_page_token`; pass the returned token with the same filters to
continue in descending `(started_at, run_id)` order. `limit` retains its default
100 and maximum 1000. Proposal lists keep `page_size` and their existing token
contract. Empty filters preserve repository-wide listings. Check the feature
before relying on task filtering: older servers may ignore unknown fields.

### Proposal discussion

Servers advertising `proposals.comments` accept `proposal.comment` through
`POST /v1/repos/{owner}/{rname}/proposals/{id}/comments` and gRPC `Comment`.
Writers, reviewers and integrators may post discussion without approval rights.
`body` must contain non-whitespace text, contain no NUL and fit in 16 KiB.
`expected_head` is required; a mismatch returns `failed_precondition` without
writing anything. An idempotency key records the response atomically with the
comment. Comments use review records with `decision=comment`, remain available
after closure and do not supersede approval or request-changes decisions.

### Required line-discussion resolution (development)

`policy.resolved_threads` advertises support for the optional policy field
`require_resolved_threads`. When true, preparation and integration report
`unresolved_threads` with `detail.count` until every line discussion is resolved,
including earlier-head anchors. The count is read inside the integration transaction;
no new endpoint or permission is introduced. General comments and approvals are separate.
The default is false. Upgrade all replicas before enabling the field; older servers reject it.
See [ADR 0032](../adr/0032-required-discussion-resolution.md).

### Proposal head preconditions (development)

Servers advertising `proposals.update_head` accept optional `expected_head` on
`proposal.update` (`PATCH /proposals/{id}`, gRPC `Update`). Send the full inspected
changeset ID. A mismatch returns `failed_precondition` (HTTP 412); comparison and head
replacement share one metadata transaction. Omission preserves legacy request compatibility
but cannot detect changes before the request began. Negotiate support before relying on the
field: an older server may ignore it. This operation does not deduplicate idempotency keys.
See [ADR 0029](../adr/0029-proposal-head-preconditions.md).

### Line review threads (development)

`proposals.threads` advertises metadata schema 6 and these operations:

| Operation | HTTP route relative to `/v1/repos/{owner}/{rname}/proposals/{id}` |
|---|---|
| `proposal.threads` | `GET /threads` |
| `proposal.thread_create` | `POST /threads` |
| `proposal.thread_comments` | `GET /threads/{thread_id}/comments` |
| `proposal.thread_reply` | `POST /threads/{thread_id}/comments` |
| `proposal.thread_resolve` | `PATCH /threads/{thread_id}` |

Reads require proposal read access. Creating/replying requires `proposal.comment`.
Resolution additionally requires the thread author, proposal author, reviewer or
integrator. `expected_version` is mandatory; replies increment it, and a stale
resolution returns `failed_precondition`. Reopen a resolved thread before replying.
All three writes support transaction-scoped idempotency keys and audit events.

Create with `body`, `expected_head` and `anchor` (`base`, `head`, `path`, `side`,
`line`). `side` is `old` or `new`, and `line` is one-based. Head must match the
current proposal; source revisions must be published in the repository. The
server reads `anchor.text` itself and ignores supplied text. Text files are bounded
to 8 MiB and individual source lines and message bodies to 16 KiB. Old-side anchors
require a base revision. Anchors never move with later proposal updates.

The Web UI recognizes a single final fenced `suggestion` block in a thread body as a
replacement proposal for its new-side anchor. An empty block deletes that line. The fence
has at least three backticks and must exceed any backtick run in the content. This is a
readable body convention, not a new server operation or permission. Applying it uses a
private workspace and a head-checked proposal update; it does not resolve the thread or
approve the resulting head. See [ADR 0030](../adr/0030-fenced-review-suggestions.md).

Both list operations use `page_size` (maximum 100) and `page_token`. Threads use
creation-time keyset ordering; replies use increasing IDs. Tokens are scoped to
the repository and owning proposal/thread. Thread resolution does not by itself
approve a proposal or introduce an integration policy requirement.

## Per-agent run admission (development)

`agents.state` advertises `PATCH /v1/repos/{owner}/{rname}/agents/{name}`
(`agent.set_state`, gRPC `AgentService.SetState`). It requires `agent.operate`.
Send `disabled` and the observed `expected_disabled`; a mismatch returns
`failed_precondition`. `Idempotency-Key` (or `idempotency_key`) replays the
original response within the repository's idempotency retention period. The
state comparison, audit record and `agent.state` event commit together.

The existing `disabled` flag controls admission of new runs, checked in their
creation transaction. Existing sessions and their credentials remain valid;
this operation is not an emergency credential-revocation or process-kill API.
See [ADR-0026](../adr/0026-agent-run-admission.md).

## Task sources (development)

`tasks.lineage` adds `source_task` and `source_run` to task creation and task
responses. Both default to empty. A nonempty run requires a source task and must
be a failed or abandoned run of that task in the same repository. Validation and
source recording commit with task creation, audit and its idempotency response.
Source task/run reads require `task.read`/`agent.read` in addition to `task.write`.

The links are immutable through the API. They do not inherit ownership,
prerequisites or retry history. Requests without these fields preserve the old
idempotency fingerprint. Metadata schema 7 requires a backup before upgrade.


### Task instruction refinement

Servers advertising `tasks.edit` expose `PATCH /v1/repos/{owner}/{rname}/tasks/{id}`
(`task.update`, `AgentService.UpdateTask`). Both `content` and `expected` are required objects
with `title`, `description`, and `scope`; omitted string/array members inside them use protocol
defaults. The caller needs `task.write` and must be the creator or have `agent.operate`.
Invalid titles or path patterns return `invalid_argument` so a form can be corrected in place.
Only an open task with no run history may change. A full-content mismatch returns
`failed_precondition`; timestamps alone are not concurrency tokens.

An optional idempotency key records the exact successful response atomically. Concurrent
identical requests replay once, even if the original content is now stale; different content
under that key is refused. Retry with the original body and key after an uncertain result.
Owner, creation metadata, prerequisites and task/run source links are unaffected. No metadata
migration is introduced. [Semantics](../adr/0028-task-instruction-refinement.md).
