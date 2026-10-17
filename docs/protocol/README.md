# Protocol (AGWP v1) — status and mapping

**Status: designed, not implemented.** ADR-0001 §9 specifies AGWP v1 (gRPC over HTTP/2 + TLS, an HTTPS API
for browsers, SSE/WebSocket for events). Version 0.1 ships the *engine* as a Rust library plus the `ag`
CLI; there is no network server yet. See [ADR-0002](../adr/0002-embedded-engine-first.md) for why, and the
[roadmap](../../ROADMAP.md) for what comes next.

The protocol will be a thin layer over operations that already exist and are tested. This table is the
contract those operations will keep.

| AGWP group | Operations | Today (library · CLI) |
|---|---|---|
| **Repository** | Create, Describe, Configure | `Repository::init/open/config` · `ag init`, `ag stream show`, `ag policy` |
| **Objects** | Upload, Get, Verify | `put_blob`, `put_changeset`, `stage_object(op, obj)`, `blob`, `changeset`; `ObjectStore::verify` · `ag verify` |
| **History** | GetChangeset, Ancestors, MergeBase | `changeset`, `log`, `ag_dag::{ancestors, merge_bases}` · `ag show`, `ag log`, `ag preview` |
| **Tree** | ResolvePath, ListDirectory | `tree_of`, `Tree::get/list_dir` · `ag cat REV:PATH`, `ag ls` |
| **Workspace** | Create, Modify, Snapshot | `workspace_create`, `snapshot`, `sync_workspace` · `ag workspace create`, `ag snapshot`, `ag sync` |
| **Proposal** | Create, Update, Review | `propose`, `update_proposal`, `review` · `ag propose`, `ag proposal …` |
| **Integration** | Prepare, Validate, Integrate, GetResult | `integrate` (prepare + integrate, resumable), `merge_preview` (validate input), `submit_attestation`, `operation`, `operation_by_key` · `ag integrate`, `ag preview`, `ag validate run`, `ag op show` |
| **Stream** | ReadHead, CompareAndSwap | `meta.read(\|tx\| tx.stream(..))`; the CAS is internal to `integrate` by design — clients never move a head directly · `ag stream show/reflog` |
| **Agent** | Register, Heartbeat, Complete | `ag_agent::registry::*` · `ag agent register/start/heartbeat/finish` |
| **Events** | Subscribe, Replay | outbox (`changeset.published`, `stream.updated`) and the audit chain · `ag trace`, `ag audit --list`; subscription 🔜 |

## What already follows the protocol's rules

- **Every mutating operation has an `operation_id` and an idempotency key**; replays return the recorded
  result; keys bound to a request hash; lost responses are answered by `operation(…)` rather than retried.
- **Structured outcomes**, not strings: `Outcome` (`integrated | blocked | conflict | denied | invalid`) and
  stable `blockers[].code` values — see the [policy reference](../guides/policy.md#outcome-codes).
- **Bounded inputs**: every decoder takes limits; sizes are checked before allocation.
- **Versioned formats**: format version inside every hashed object; schema version in the database; a build
  refuses a database written by a newer version.

## Still to design for the network

Capability negotiation, message versioning on the wire, pagination, backpressure and deadlines, quotas,
authentication (OIDC for users, short-lived scoped credentials for agents) and authorization (RBAC/ABAC
per repository, stream and operation). These belong with the server work.
