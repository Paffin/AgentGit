-- Schema version 3: indexes for the keyset paging of proposals (`Tx::proposals_page`), which lists newest first
-- (`ORDER BY created_at DESC, id DESC`) and filters by state and/or stream. Without them every page sorts the
-- whole table; with them a page reads only its own rows. Additive: no data changes, a build that only knows
-- schema 2 refuses the migrated database (docs/architecture/compatibility.md).

CREATE INDEX proposals_by_created ON proposals (created_at DESC, id DESC);
CREATE INDEX proposals_by_state_created ON proposals (state, created_at DESC, id DESC);
CREATE INDEX proposals_by_stream_created ON proposals (stream, created_at DESC, id DESC);
