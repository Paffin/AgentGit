CREATE TABLE issues (
    number INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    author TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('open', 'closed')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,
    task_id TEXT REFERENCES tasks(id),
    search_text TEXT NOT NULL
);
CREATE INDEX issues_state_number ON issues(state, number DESC);
CREATE TABLE issue_activity (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_number INTEGER NOT NULL REFERENCES issues(number),
    kind TEXT NOT NULL,
    body TEXT NOT NULL,
    author TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX issue_activity_issue_id ON issue_activity(issue_number, id);
