CREATE TABLE proposal_reviewers (
    proposal TEXT NOT NULL REFERENCES proposals(id),
    reviewer TEXT NOT NULL,
    PRIMARY KEY (proposal, reviewer)
);
CREATE INDEX proposal_reviewers_identity ON proposal_reviewers(reviewer, proposal);
