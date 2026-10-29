CREATE TABLE comment_drafts (
    proposal TEXT NOT NULL REFERENCES proposals(id),
    owner TEXT NOT NULL,
    version BIGINT NOT NULL CHECK (version > 0),
    head TEXT,
    body TEXT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (proposal, owner),
    CHECK (head IS NOT NULL OR body = '')
);
