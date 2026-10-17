# PostgreSQL and S3 storage

By default a repository is a SQLite file and a directory of objects under `.ag/`: one machine, zero
infrastructure. For a shared or hosted setup the same engine can keep its **metadata in PostgreSQL** and its
**objects in an S3-compatible bucket** (Amazon S3, MinIO, Ceph RGW, …). The guarantees are the same — the
suites that prove them run unchanged on every backend (see [Testing](#testing-each-backend)) — only
the infrastructure differs. The design decisions are recorded in [ADR-0004](../adr/0004-postgres-and-s3-backends.md).

| | default | shared setup |
|---|---|---|
| metadata | `.ag/meta.db` (SQLite, WAL) | one **schema** of a PostgreSQL database per repository |
| objects | `.ag/store/objects` (files) | one **key prefix** of a bucket per repository |
| writers | one at a time (database write lock) | one at a time per repository (advisory lock), many repositories in parallel |
| processes | one host | any number of hosts |
| `ag backup` | yes | metadata in PostgreSQL: **refused** (use `pg_dump`); objects in S3: yes |

The config keys, defaults and environment variables are collected in [the reference](#reference) below.

## Quick start

Create a database and a role, then describe the storage in a JSON file and pass it to `ag init`:

```bash
createdb agentgit                                        # or use an existing database
export AG_PG_URL='postgres://ag:secret@db.internal/agentgit?sslmode=verify-full&sslrootcert=/etc/ssl/db-ca.pem'
export AWS_ACCESS_KEY_ID=…  AWS_SECRET_ACCESS_KEY=…     # the standard variables

cat > storage.json <<'EOF'
{
  "meta":    { "kind": "postgres", "url_env": "AG_PG_URL", "schema": "ag_acme_app" },
  "objects": { "kind": "s3", "endpoint": "https://s3.eu-west-1.amazonaws.com", "region": "eu-west-1",
               "bucket": "acme-agentgit", "prefix": "acme/app" }
}
EOF
ag init --namespace acme/app --storage storage.json
```

`ag init` writes `.ag/config.json` (including this section) and connects, creates the schema, migrates it and
creates the first stream. Every other `ag` command — and every process that opens the directory — uses the
configuration it finds there. **The directory is only the entry point**: it holds `config.json`, the local keys
(`.ag/keys`, or `$AG_KEY_DIR`) and the working files. Copy `.ag/config.json` to another machine (or set the same
variables there) and that machine works on the same repository.

Secrets never touch the disk: the configuration contains the **names** of the environment variables that hold
the URL (with its password) and the S3 credentials. A literal `url` is accepted only if it has no password
(`ag init` refuses it otherwise), for example `host=/var/run/postgresql dbname=ag` with OS authentication.

## PostgreSQL

**Privileges.** The role needs `CREATE` on the database (to create the schema; or create the schema yourself
and grant `USAGE, CREATE` on it) and full rights on the schema's objects. `ag` only ever touches its own schema:
the session's `search_path` is that schema alone.

**One schema per repository.** `schema` is the unit of isolation: lowercase letters, digits and `_`, at most
63 bytes, not starting with `pg_`. Two repositories must not share a schema; `ag init` refuses to initialize
on top of a schema that already holds one. Many repositories can share a database (and a server).

**Migrations** run when a repository is opened, inside one transaction guarded by an advisory lock, so any
number of processes may start at once (`opening_twice_at_once_migrates_exactly_once`). The version is stored in
the `meta` table (key `schema`). A schema written by a *newer* `ag` is refused, never touched
(`a_version_1_database_migrates_forward_and_keeps_its_data` covers the upgrade path from schema 1). There is no
downgrade.

**TLS.** Set `sslmode` (and `sslrootcert`) in the URL:

| `sslmode` | behaviour |
|---|---|
| `disable` | plaintext |
| `prefer` (the default) | TLS if the server offers it, **without verifying** the certificate |
| `require` | TLS, **without verifying** the certificate (as in libpq: protects against eavesdropping, not against impersonation) |
| `verify-full` (and `verify-ca`) | TLS, verifying the certificate chain **and** the host name against the built-in web roots, or against the PEM file in `sslrootcert` |

Use `verify-full` for anything that crosses a network you do not control. `verify-ca` is treated as
`verify-full`. Client certificates are not supported. Tested against a server with a private CA
(`tls_connections_work_and_certificates_are_verified_when_asked`).

**Connections.** Each process keeps a pool (default 8, `pool_size`) of connections plus, when objects are in S3,
one dedicated connection for the [object lock](#the-object-lock). Budget `processes × (pool_size + 1)` against
`max_connections`. Defaults you may want to know about: a write transaction waits up to **30 s** for the
repository's write lock, then fails with a retriable error; a transaction that sits idle for **10 minutes** is
ended by the server (it bounds the damage of a hung client: closing the connection releases the lock).
Connection *poolers* in transaction mode (PgBouncer) are not supported: `ag` relies on session state
(`search_path`, prepared statements, session advisory locks). Session-mode pooling should work; it has not been
tested.

**Transactions.** A write is `READ COMMITTED` and takes `pg_advisory_xact_lock(key)` first, where `key` is derived
from the schema name; so the writers of one repository are serialized, those of different repositories are not,
and a crashed writer cannot leave the lock behind. A read is `REPEATABLE READ READ ONLY`: a snapshot, never
blocked. Details and tests: [Storage](../architecture/storage.md#transaction-semantics-per-backend).

**Errors you can act on.** `transient metadata failure (safe to retry)` — nothing was committed, run it again.
`lost the connection … while committing (the outcome is unknown)` — the commit may or may not have happened:
look the operation up (`ag op show <operation-id>`) instead of repeating it
([troubleshooting](troubleshooting.md#postgresql-and-s3)).

## S3

**Bucket.** It must exist (it is not created) and belong to the region you configure. Give the credentials
`s3:ListBucket` on the bucket and `s3:GetObject`, `s3:PutObject`, `s3:DeleteObject` on `arn:aws:s3:::<bucket>/<prefix>/*`
(the age refresh is a server-side copy of an object onto itself, which needs read and write on that object).
Do **not** put a lifecycle expiration rule on the prefix: garbage collection is `ag gc`'s job, and an object
that expires behind its back is missing data. Bucket versioning is a good idea (it turns an accidental delete
into a recoverable event) but is not required.

**Addressing.** `path_style` defaults to *yes* (`https://host/bucket/key`, what MinIO and most servers
expect) except for `*.amazonaws.com` endpoints (virtual-hosted style, `https://bucket.host/key`).
`endpoint` may carry a path prefix for gateways.

**Credentials** are static: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and optionally `AWS_SESSION_TOKEN`, or
the variables you name in `access_key_id_env`, `secret_access_key_env`, `session_token_env`. Instance profiles,
web-identity roles and `~/.aws/credentials` are **not** read; export short-lived credentials into the
environment if you use them (and restart long-running processes when they expire).

**Keys and size.** Objects are stored at `<prefix>/objects/<2 hex>/<62 hex>` — the same fan-out as the
filesystem store — holding the same envelope. There is no multipart upload: each object is one `PUT`, held in
memory while it is transferred, so the largest object is bounded by the repository's blob limit (256 MiB by
default; S3's own limit for one `PUT` is 5 GiB).

**What a `put` does.** A conditional create (`If-None-Match: *`); if the object exists, it is read back and
verified, then its age is refreshed with a server-side `CopyObject` onto itself (a damaged copy is overwritten
instead). Servers that ignore the conditional header just rewrite identical bytes. `get` hashes every object it
reads. Transient failures (connection errors, timeouts, 408/409/429/5xx) are retried with backoff; every other
non-success answer is an error, and a 404 is "absent" only where absence is a valid answer
(`only_a_404_means_absent_and_only_where_absence_is_a_valid_answer`,
`client_errors_are_never_retried_and_never_mistaken_for_absence`).

**TLS** uses the built-in web roots; for a private CA set `ca_bundle` (or run the endpoint over `http://` inside a
trusted network). Redirects are never followed.

**Durability** is what the service promises, and `ag` says so (`s3-compatible` in `ag init`'s output): a 2xx means
the service stored the object; whether that survives a disk, a node or a site is the provider's contract —
replicated or erasure-coded for Amazon S3 and multi-node MinIO, **not** for a single-node MinIO. See
[Durability](durability.md).

## The object lock

Garbage collection deletes an object only if it is unreachable and older than the grace period, and re-checks
that at the moment of deletion. On a filesystem the check and the delete sit under one `flock`; over HTTP there
is no atomicity between two requests, so `ag` coordinates writers and the collector with a lock
(`ObjectLock`): `put` holds it *shared* — any number at once — and the collector holds it *exclusive* across one
object's check-then-delete. The lock must span **everyone who can write to or collect from the prefix**:

| `lock` | spans | use when |
|---|---|---|
| `auto` (default) | `postgres_advisory` with PostgreSQL metadata, otherwise `file` | almost always |
| `postgres_advisory` | every process that reaches the database, on any host; a crashed holder is released by PostgreSQL itself | PostgreSQL metadata (shared setups) |
| `file` | processes on this host (`.ag/objects.lock`) | SQLite metadata + S3 on one machine |
| `in_process` | one process | tests, embedding |

SQLite metadata with S3 objects is a **single-host** setup (SQLite is a local file). Do not point several hosts
at one bucket prefix with `file` locks. The race is reproduced and closed in
`s3_gc_cannot_delete_an_object_a_writer_just_reput`: without the lock the collector deletes an object a writer's
`put` just returned for; with it the put is never undone.

## Reference

`storage` section of `.ag/config.json` (additive; the file's `format` stays `1`; absent means SQLite + files).
Unknown keys are errors, so a typo does not silently select a default.

| key | meaning | default |
|---|---|---|
| `meta.kind` | `sqlite` or `postgres` | `sqlite` |
| `meta.url_env` | name of the variable holding the PostgreSQL URL | `AG_PG_URL` |
| `meta.url` | literal URL, only if it has no password | — |
| `meta.schema` | schema of this repository (required for `postgres`) | — |
| `meta.pool_size` | pooled connections per process | 8 |
| `objects.kind` | `fs` or `s3` | `fs` |
| `objects.endpoint`, `objects.bucket` | service URL and bucket (required for `s3`) | — |
| `objects.region` | signing region | `us-east-1` |
| `objects.prefix` | key prefix | the repository namespace |
| `objects.path_style` | path-style addressing | yes, except for `amazonaws.com` |
| `objects.access_key_id_env`, `objects.secret_access_key_env`, `objects.session_token_env` | names of the credential variables | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN` |
| `objects.ca_bundle` | PEM file with the CA(s) of a private endpoint | built-in web roots |
| `objects.lock` | `auto`, `postgres_advisory`, `file`, `in_process` | `auto` |

Pass the section to `ag init` with `--storage <file>`; the file holds just the object shown in the quick start.

Environment variables read by `ag`: the ones named above (`AG_PG_URL`, `AWS_*`), and `AG_KEY_DIR` as before.
Used only by the test scripts: `AG_TEST_POSTGRES_URL`, `AG_TEST_POSTGRES_CA`, `AG_TEST_S3_ENDPOINT`,
`AG_TEST_S3_BUCKET`, `AG_TEST_S3_REGION`, `AG_TEST_S3_ACCESS_KEY_ID`, `AG_TEST_S3_SECRET_ACCESS_KEY`,
`AG_REQUIRE_BACKENDS`.

## Operating it

- **Backup and restore** are per backend: [Durability](durability.md#backup).
- **Garbage collection** needs the same environment as any other command; with S3 it lists the prefix and issues
  one `HEAD` and (if still due) one `DELETE` per candidate. See [GC](gc-and-retention.md).
- **`ag verify`** hashes every object it can list and cross-checks the metadata. Run it after any restore. With
  S3 it downloads every object once: schedule it accordingly.
- **Decommissioning** a repository is dropping its schema (`DROP SCHEMA … CASCADE`) and deleting its prefix; `ag`
  has no command for it on purpose.
- **Monitoring.** Connections carry `application_name = agentgit` (`pg_stat_activity`); the object lock's
  session is `agentgit-object-lock`. `pg_locks` shows the advisory locks (`locktype = 'advisory'`).
- **Moving an existing repository** from SQLite/files to PostgreSQL/S3 is not supported yet (there is no
  migration tool). `ag git export` / `ag git import` carries history but not proposals, reviews, policy,
  attestations or the audit log.

### Sizing and scaling

Not benchmarked; these follow from the design. Writes to **one** repository are serialized — each write
transaction is short (a few statements), but throughput per repository is bounded by round trips to the
database, not by the number of processes. Different repositories do not contend. Reads are snapshots and scale
with connections. Every object operation on S3 is a network round trip: a new object costs one `PUT`; an
object that already exists costs a `PUT` (412), a `GET` and a `CopyObject`; `ag verify` and `ag fsck` read every
object. Keep the database and the bucket in the same region as the `ag` processes.

### Honest limits

- Verified against **PostgreSQL 16** and **moto** (a Python S3 implementation that checks request signatures).
  Not yet run against Amazon S3, MinIO or other PostgreSQL versions; the S3 behaviours relied upon (conditional
  create, `CopyObject` onto itself with `REPLACE`, `ListObjectsV2`) are standard, but your service's edge
  cases are yours to confirm. The scripts below make that a one-command exercise against a server of your
  choice.
- If the object lock's PostgreSQL connection dies while a guard is held, protection is lost for that guard (the
  next acquisition reconnects). The collector holds its guard for one object's check-then-delete, a few
  milliseconds.
- Object ages are the server's `LastModified` (one-second resolution on Amazon S3); grace periods are hours.
- `BIGSERIAL` ids (`reflog.seq`, `audit.seq`, …) can have gaps after a rolled-back transaction; order and
  uniqueness are unaffected.
- PostgreSQL rejects NUL characters in text; a title or description containing one is refused with a clear error.
- An `ag` older than this feature does not understand the `storage` section. To keep it from silently working on
  an empty local repository, `ag init` occupies `.ag/meta.db` (PostgreSQL metadata) or `.ag/store` (S3 objects)
  with a placeholder, so the old binary fails to open the directory. Upgrade every machine before pointing it at a
  shared repository.

## Testing each backend

The same suites run on every backend; environment variables choose which. Without any, everything runs on SQLite
and local files and needs nothing else.

```bash
cargo test --workspace                                         # SQLite + files (default)
scripts/pg-test.sh cargo test --workspace                      # metadata on a throwaway PostgreSQL cluster
scripts/s3-test.sh cargo test --workspace                      # objects on a throwaway S3 server (moto)
AG_REQUIRE_BACKENDS=1 scripts/pg-test.sh scripts/s3-test.sh cargo test --workspace   # both, and fail if either is missing
```

`scripts/pg-test.sh` needs the PostgreSQL server binaries (`initdb`, `pg_ctl`, `postgres`; as root it runs them as
the `postgres` user) and starts a private cluster with a private CA so the TLS tests run too.
`scripts/s3-test.sh` needs `python3` with `venv`/`pip` (it installs moto into `~/.cache/agentgit/s3-venv` the first
time) and switches signature checking on, so the server independently verifies every request our signer produces.
Each test gets its own schema and key prefix and removes it afterwards. With `AG_REQUIRE_BACKENDS=1`, a test that
needs a backend whose variable is unset **fails** rather than quietly using the default.
