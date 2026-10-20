# ADR-0011: Proposal paging indexes (metadata schema 3), dated idempotency records, a lease for background work

- Status: accepted
- Date: 2026-10-20
- Relates to: ADR-0004 (PostgreSQL and SQLite behind one schema), ADR-0007 (several replicas, additive migrations),
  ADR-0010 (the run reaper), [compatibility](../architecture/compatibility.md),
  [operations: deployment](../operations/deployment.md)

## Context

Three follow-ups of the server-hardening wave left small persistent-state decisions open:

1. `GET /v1/repos/{repo}/proposals` pages by key (`created_at`, `id`) and filters in SQL, but the tables had no index
   that serves `ORDER BY created_at DESC, id DESC`: every page sorted the whole proposals table. Fine for thousands of
   proposals, not for hundreds of thousands.
2. Idempotency records written before they carried a creation time (builds before `repos.idempotency_ttl_secs`) were
   kept forever, because a record without a time cannot be judged.
3. The periodic run reaper (`agents.reap_interval_secs`) ran on every replica. A reap is idempotent, so that was
   correct, but N replicas did N times the work, and each pass opens every repository.

## Decision

### Metadata schema 3: three indexes

`migrations/{sqlite,postgres}/0003_proposal_paging.sql` creates, on `proposals`:

* `proposals_by_created (created_at DESC, id DESC)` - the unfiltered listing;
* `proposals_by_state_created (state, created_at DESC, id DESC)` - `?state=open`;
* `proposals_by_stream_created (stream, created_at DESC, id DESC)` - `?stream=main`.

Author-filtered pages use the first index and filter rows (an author filter is rare and its selectivity high). The
migration changes no rows and no format; hashes, wire formats and the API are unchanged. It is applied in the
transaction that raises the schema version (as every migration), so a crash leaves schema 2.

Consequence, stated plainly: **a build that only knows schema 2 refuses a repository migrated to 3**
(`UnsupportedSchema`), as for every schema bump. A rolling upgrade across replicas starts the new build first
(ADR-0007). Downgrading needs a restore or `DROP INDEX` of the three indexes plus `schema` = 2 by hand; nothing else
changed.

Test evidence: `every_proposal_page_is_read_in_index_order_without_sorting` asks the database for the plan of exactly
the statement `proposals_page` runs (`proposals_page_clause`) for each filter combination, with and without a
cursor, on SQLite (`EXPLAIN QUERY PLAN`) and PostgreSQL (`EXPLAIN` with sequential scans and sorts forbidden, since a
table of a few rows rightly prefers a scan); it fails without the migration. The upgrade is covered by the
schema-1 -> 3 unit tests (SQLite and PostgreSQL) and by a compatibility fixture written by this build
(`tests/compat/fixtures/repo-0.2.0-rc.2`) next to the schema-2 fixture of the first alpha, which is migrated to 3 when
opened.

### Dated idempotency records

The first idempotency sweep of a repository gives every record that has no time the time of that sweep, once
(`meta` key `backfill:idem-times` marks it done), in one SQL statement. Such a record then lives one more
`idempotency_ttl_secs` and is swept like any other. The true age of those records is unknowable; "one lifetime after
the upgrade" is the honest bound. Not a schema change.

Limit: a record without a time written *after* the backfill (an old replica during a rolling upgrade) is not dated; it
stays until the next manual cleanup. Run the sweep after the rollout finishes, or accept the few records.

### A lease for background work

`Control::try_lease(name, holder, ttl)` keeps a lease as a row of the control database's existing `kv` table
(`lease/<name>` = `<holder>|<expires_at_ms>`). Taking it is one conditional statement - `INSERT … DO NOTHING` when
absent, `UPDATE … WHERE value = <what was read>` when expired or already the caller's - so of several replicas racing
for an expired lease exactly one wins, on both backends, with no new table (a new control table would have made the
control schema 4 and broken rolling upgrades for a convenience). The holder renews on every pass; a lease expires
after two reap intervals if the holder dies, and is released at once on a clean stop. The run reaper takes the
`run-reaper` lease before each pass and skips the pass without it.

The lease is a courtesy, not a lock: it compares wall clocks of the replicas (skew of a fraction of the interval is
harmless), and the work it guards stays idempotent, so two replicas briefly reaping together change nothing twice.
Idempotency sweeping and the health scan are per-replica by design (each replica needs its own `/readyz` figures) and
do not use it.

## Consequences

* ✅ A page of proposals reads its own rows; no sort of the table.
* ✅ Old idempotency records stop being immortal.
* ✅ One reaper per installation, without a new table or a new service.
* ⚠️ Metadata schema 3 is not readable by older builds (see above).
* ⚠️ Index cost: three more entries per proposal write. Proposals are created and closed at human/agent-review
  speed; not measured as a load.
* ⚠️ Not verified: the plan on a large PostgreSQL table (the plan test forbids scans to learn which index could be
  used; the planner's choice at scale is its own); several real replicas contending for the lease (the contention is
  tested at the database level on SQLite and PostgreSQL with threads, and the gating through the server's reap pass on
  one server).
