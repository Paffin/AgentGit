# Protocol (AGWP v1)

**Status: implemented** in `ag-server` (crate `ag-server`; wire types in `ag-protocol`).

| Document | What it is |
|---|---|
| [`agwp-v1.md`](agwp-v1.md) | the reference: connecting, conventions (JSON mapping, idempotency, pagination, deadlines, backpressure), errors, authentication, authorization, publication, signing formats, events |
| [`openapi.json`](openapi.json) | the generated OpenAPI 3.1 document of the HTTP/JSON API. A test fails if it differs from what the server serves (`/openapi.json`); regenerate with `AG_UPDATE_GOLDEN=1 cargo test -p agentgit-server --test observability` |
| [`proto/agwp/v1/agwp.proto`](../../proto/agwp/v1/agwp.proto) | the single source of truth for every message and gRPC service |

One implementation serves two transports on one port: **gRPC** (CLI, SDKs, agents, workers) and **HTTP/JSON +
SSE** (browsers, scripts). The protocol is a thin layer over the engine's operations — a client can never
move a stream head directly; changes travel as proposals and land only through `integrate`, under policy.

## What follows the engine's rules

- **Idempotency is operation-specific**: operations that support `idempotency_key` return the recorded result on replay and bind keys to the original request. Some operations, including review submission, accept but ignore the key; consult the message's field documentation before retrying a lost response.
- **Structured outcomes**, not strings: `integrate` answers `integrated | blocked | conflict | denied |
  invalid` with stable blocker codes — see the [policy reference](../guides/policy.md#outcome-codes).
- **Bounded inputs**: sizes are checked before allocation; the server advertises its limits in
  `GET /v1/capabilities`.
- **Versioned wire format**: capability negotiation first; unknown JSON fields are ignored, unknown enum
  values must be tolerated by clients.

## Mapping to the engine

| AGWP group | Engine (library) | CLI |
|---|---|---|
| **Repository** | `Repository::init/open/config` | `ag init`, `ag stream show`, `ag policy` |
| **Objects** | `put_blob`, `put_changeset`, `stage_object`, `ObjectStore::verify` | `ag verify` |
| **History / Tree** | `changeset`, `log`, `tree_of`, `Tree::get/list_dir`, `ag_dag::*` | `ag show`, `ag log`, `ag cat`, `ag ls` |
| **Workspace** | `workspace_create`, `snapshot`, `sync_workspace` (virtual workspaces on the server) | `ag workspace …`, `ag snapshot`, `ag sync` |
| **Proposal** | `propose`, `update_proposal`, `review` | `ag propose`, `ag proposal …`, `ag remote review` |
| **Integration** | `integrate` (prepare + integrate, resumable), `merge_preview`, `submit_attestation` | `ag integrate`, `ag remote integrate/prepare` |
| **Stream** | compare-and-swap is internal to `integrate` by design | `ag stream show/reflog` |
| **Agent** | `ag_agent::registry::*`, leases, tasks, provenance, conflict prediction | `ag agent …`, `ag lease …`, `ag task …` |
| **Events** | the hash-chained audit log (SSE and replay); the outbox is internal to the engine | `ag trace`, `ag audit`, `ag remote events --follow` |
| **Execution** | validation jobs for trusted workers: claim, heartbeat, complete (with a signed attestation), fail | — |
| **Admin** | policy, roles, credentials, GC, fsck, pins | `ag policy`, `ag gc`, `ag remote policy` |

Clients: the `ag` CLI (`ag clone/fetch/pull/push/remote`) and the Rust crate `ag-client`.
