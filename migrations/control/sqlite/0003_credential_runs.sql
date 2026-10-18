-- Schema version 3: a credential can be bound to one run of one agent (ADR-0010). The binding is part of the
-- credential's scope (`scope_json`, which authentication reads); these two columns repeat it so that the credentials
-- of a run can be found - and revoked when the run ends - with an index, and so that a build that predates the
-- binding refuses this database (newer schema) instead of reading a run-bound credential as an ordinary one.
-- Empty for every credential minted before this version. Additive.

ALTER TABLE credentials ADD COLUMN run_repo TEXT NOT NULL DEFAULT '';
ALTER TABLE credentials ADD COLUMN run_id TEXT NOT NULL DEFAULT '';
CREATE INDEX credentials_by_run ON credentials (run_repo, run_id) WHERE run_id <> '';
