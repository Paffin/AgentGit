CREATE TABLE review_threads (
    id TEXT PRIMARY KEY,
    proposal TEXT NOT NULL REFERENCES proposals(id),
    source_base TEXT,
    source_head TEXT NOT NULL,
    path TEXT NOT NULL,
    side TEXT NOT NULL CHECK (side IN ('old', 'new')),
    line INTEGER NOT NULL CHECK (line > 0),
    source_text TEXT NOT NULL,
    body TEXT NOT NULL,
    author TEXT NOT NULL,
    resolved INTEGER NOT NULL DEFAULT 0 CHECK (resolved IN (0, 1)),
    version BIGINT NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);
CREATE INDEX review_threads_proposal ON review_threads(proposal, created_at DESC, id DESC);
CREATE TABLE review_thread_comments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    thread_id TEXT NOT NULL REFERENCES review_threads(id),
    body TEXT NOT NULL,
    author TEXT NOT NULL,
    created_at BIGINT NOT NULL
);
CREATE INDEX review_thread_comments_thread ON review_thread_comments(thread_id, id);
