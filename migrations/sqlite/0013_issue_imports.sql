CREATE TABLE issue_imports (
    provider TEXT NOT NULL,
    source_repo TEXT NOT NULL,
    source_id TEXT NOT NULL,
    digest TEXT NOT NULL,
    issue_number BIGINT NOT NULL REFERENCES issues(number),
    source_json TEXT NOT NULL,
    imported_by TEXT NOT NULL,
    imported_at BIGINT NOT NULL,
    PRIMARY KEY (provider, source_repo, source_id)
);
