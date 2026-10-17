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

* **JSON mapping.** Field names are the proto field names (`snake_case`). `int64`/`uint64` are JSON numbers
  (timestamps are Unix milliseconds in `*_ms`, sequence numbers and versions stay far below 2^53). `bytes`
  are standard base64. Enumerations are strings with a documented value set; clients must tolerate unknown
  values. Missing fields take their zero value; unknown fields are ignored (so old clients and new servers
  interoperate).
* **Where parameters go.** `GET`/`DELETE`: path placeholders and the query string. Other methods: a JSON
  object body (path placeholders are merged in and win). *Free-form names — streams, revisions, paths,
  pin names — travel in the query string or body, never inside the URL path*, so URL encoding cannot change
  which resource a request addresses. Repositories are `owner/name`: `/v1/repos/{owner}/{name}/…`.
* **Idempotency.** Mutating requests accept `idempotency_key` (HTTP: also the `Idempotency-Key` header).
  Repeating a request with the same key and content returns the recorded result; the same key with
  different content is refused with `idempotency_key_reused`. After a lost response, ask for the outcome
  (`GET …/operations?idempotency_key=…`) instead of blindly retrying a non-idempotent call.
* **Pagination.** `page_size` (0 = default 100, max 1000) and an opaque `page_token`; responses carry
  `next_page_token` (empty = last page). Tokens are only valid for the listing that produced them.
* **Deadlines.** gRPC deadlines (`grpc-timeout`) or, over HTTP, `Request-Timeout: <milliseconds>`. When a
  deadline expires the operation *may still complete*: the error is `deadline_exceeded`; ask for the outcome.
  The server applies `limits.default_deadline_ms` when the client sets none.
* **Backpressure.** `429 rate_limited`/`quota_exceeded` and `503 unavailable` carry `retry_after_ms`
  (HTTP: `Retry-After`). Honour it; retry only errors with `retryable: true`.
* **Correlation.** Send `X-Request-Id` (gRPC metadata `x-request-id`); the server echoes it in
  `X-Request-Id` and in every error. `traceparent` is propagated into logs and traces.

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

**Outcomes are not errors.** `integrate` returns `200` with `outcome.status` ∈ `integrated`, `blocked`
(something can be provided: approvals, validation, a lease), `conflict`, `denied`, `invalid`. Each blocker
has a stable `code` — see the [policy guide](../guides/policy.md#outcome-codes).

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

## Authorization

Roles are bound to identities, patterns (`agent:*`) or OIDC groups, per repository pattern; a request needs
**both** a role that grants the capability **and** a credential scope that permits it.

| Role | Grants |
|---|---|
| `reader` | read everything in a repository: history, trees, streams, proposals, events, policy, audit pins |
| `writer` | reader + upload/publish, virtual workspaces, open/update/close proposals, request validation, enqueue, register agents and run as an agent, tasks, leases, provenance |
| `reviewer` | reader + review proposals |
| `integrator` | reader + integrate proposals, run the queue, close proposals |
| `runner` | reader + claim execution jobs and submit attestations (execution-plane workers) |
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
comments. A subscriber that was away loses nothing; the stream ends when its credential expires.
