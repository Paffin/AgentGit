CREATE TABLE review_drafts (
    proposal TEXT NOT NULL REFERENCES proposals(id),
    owner TEXT NOT NULL,
    version BIGINT NOT NULL CHECK (version > 0),
    head TEXT,
    decision TEXT NOT NULL,
    body TEXT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (proposal, owner),
    CHECK ((head IS NULL AND decision = '' AND body = '') OR
           (head IS NOT NULL AND decision IN ('approve', 'request_changes', 'comment')))
);
