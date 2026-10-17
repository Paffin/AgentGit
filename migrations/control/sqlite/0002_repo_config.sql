-- Schema version 2: the registry remembers each repository's `.ag/config.json`, so that any replica can
-- open a repository that was created through another one (ADR-0007). Empty for repositories registered
-- before this version; such a repository is served by the replica that holds its files.
-- Additive: a replica still running the version-1 build keeps working against this schema.

ALTER TABLE repos ADD COLUMN config_json TEXT NOT NULL DEFAULT '';
