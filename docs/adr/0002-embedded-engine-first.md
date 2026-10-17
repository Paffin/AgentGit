# ADR-0002: Embedded engine first; SQLite as the first metadata backend

- Status: accepted
- Date: 2026-10-17
- Amends: [ADR-0001](0001-final-architecture.md) §10 (stack) and the "online-first MVP" decision

## Context

ADR-0001 describes a server-centred system: Rust SCM server with gRPC (AGWP), PostgreSQL for metadata,
S3-compatible object storage, an asynchronous Go agent platform, an isolated execution plane and a Vue Web UI.
It also says the MVP must be *narrow* and that the correctness core must not depend on any of those layers.

Building everything at once would produce a large surface with shallow guarantees. The valuable and risky
part of the design — the part everything else relies on — is the deterministic core and the publication and
integration protocols: canonical formats, verified publication, atomic compare-and-swap, policy evaluated in
the committing transaction, safe GC, crash recovery.

## Decision

Ship the **engine as a library and a CLI first**, with the same guarantees the server will need, and add
network layers later.

1. **Embedded, local-first.** `ag` links the engine (`ag-integrate`) and talks to a repository directory
   (`.ag/`). Several processes can use one repository concurrently; SQLite serializes writers across
   processes.
2. **SQLite (WAL, `synchronous=FULL`) is the first metadata backend** instead of PostgreSQL. It provides what
   the design needs from the metadata store: strongly consistent, transactional writes, with `BEGIN IMMEDIATE`
   giving the same "read facts, decide, write" atomicity as a `SELECT … FOR UPDATE`/serializable PostgreSQL
   transaction on the stream row.
3. **The filesystem object store is the first object backend**; `ObjectStore` is a trait and each backend
   states its durability contract.
4. **No premature abstraction of the metadata layer.** All SQL lives in `ag-storage::meta` behind methods on
   a transaction handle. When a second backend (PostgreSQL) arrives, the trait is extracted from that
   concrete surface rather than guessed now.
5. **No network protocol in 0.1.** AGWP stays a design; [`docs/protocol`](../protocol/README.md) maps each
   of its operations onto the engine API so the server is a thin layer.

## What does not change

Every guarantee in ADR-0001 §2 and §5: immutable canonical history, objects verified before the transaction
that makes them reachable, CAS on `(head, version)` with mandatory recomputation, idempotent resumable
intents, signed attestations bound to the exact tree, fail-closed GC, tamper-evident audit. They are
implemented in the engine, independent of backend and transport, and tested with fault injection.

## Consequences

- ✅ A usable, testable product today; the hard guarantees are proven before network concerns are added.
- ✅ Zero infrastructure to try or run it; one binary, one directory.
- ⚠️ **Local mode trusts the local user.** Identities are labels; policy is enforced by the engine but anyone
  who can write `.ag/` can bypass it. Real enforcement across users and agents needs the server with
  authentication. This is documented prominently ([security](../architecture/security.md)).
- ⚠️ Single node; SQLite is one writer at a time. Fine for the target scale of one team and its agents; it is
  not a hosting platform.
- ⚠️ Anything that requires a long-running process (event subscriptions, a live dashboard, remote execution
  planes) waits for the server.

## Migration path

1. Server crate exposing the engine over HTTP/JSON and gRPC (AGWP), with authentication and quotas.
2. PostgreSQL backend: implement the `ag-storage::meta` surface; keep the SQLite backend for local mode.
3. S3-compatible object store behind `ObjectStore`.
4. Execution plane service that submits attestations over the network.
5. Web UI as a client of the server.

The on-disk formats (`ChangesetV1`, tree manifest v1, attestation/provenance signatures, audit chain) are
independent of all of this and are expected to carry over unchanged.
