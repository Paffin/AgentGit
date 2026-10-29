CREATE TABLE file_drafts (
    owner TEXT NOT NULL,
    stream TEXT NOT NULL,
    path TEXT NOT NULL,
    version BIGINT NOT NULL CHECK (version > 0),
    base TEXT,
    original_blob_id TEXT,
    executable BIGINT NOT NULL,
    file_type TEXT NOT NULL,
    content BLOB NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (owner, stream, path),
    CHECK ((base IS NULL AND original_blob_id IS NULL AND file_type = '' AND executable = 0) OR
           (base IS NOT NULL AND original_blob_id IS NOT NULL AND file_type IN ('regular', 'symlink')))
);
