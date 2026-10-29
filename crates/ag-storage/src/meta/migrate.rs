//! Numbered schema migrations, one list per backend.
//!
//! The SQL lives in the top-level `migrations/` directory; this crate compiles in its own copy
//! (`crates/ag-storage/migrations/`, so that `cargo package` is self-contained), kept identical by
//! `scripts/sync_crate_files.py` and a test, with `include_str!`. A migration is applied exactly once, in order, inside the
//! transaction that records the new schema version; the version is kept in
//! `PRAGMA user_version` (SQLite) and in the `meta` table, key `schema` (both).
//!
//! To add one: create `migrations/<backend>/NNNN_name.sql` for *both* backends, run
//! `python3 scripts/sync_crate_files.py`,
//! list it below and raise [`SCHEMA_VERSION`]. A test fails if a file in the
//! directory is not listed or if the two lists disagree.

/// One schema migration.
#[derive(Clone, Copy, Debug)]
pub struct Migration {
    /// The schema version after applying it.
    pub version: i64,
    /// File name, for diagnostics.
    pub name: &'static str,
    /// The SQL (possibly several statements).
    pub sql: &'static str,
}

impl Migration {
    /// Names the migration in an error; transient and connection errors pass through untouched.
    pub(crate) fn failed(&self, e: super::MetaError) -> super::MetaError {
        use super::MetaError;
        match e {
            MetaError::Retriable(_) | MetaError::ConnectionLost { .. } => e,
            other => MetaError::Invalid(format!("schema migration {} failed: {other}", self.name)),
        }
    }
}

/// Schema version understood by this build.
pub const SCHEMA_VERSION: i64 = 10;

pub(crate) const SQLITE_MIGRATIONS: &[Migration] = &[
    Migration { version: 1, name: "0001_init.sql", sql: include_str!("../../migrations/sqlite/0001_init.sql") },
    Migration { version: 2, name: "0002_jobs.sql", sql: include_str!("../../migrations/sqlite/0002_jobs.sql") },
    Migration {
        version: 3,
        name: "0003_proposal_paging.sql",
        sql: include_str!("../../migrations/sqlite/0003_proposal_paging.sql"),
    },
    Migration { version: 4, name: "0004_issues.sql", sql: include_str!("../../migrations/sqlite/0004_issues.sql") },
    Migration {
        version: 5,
        name: "0005_task_scheduling.sql",
        sql: include_str!("../../migrations/sqlite/0005_task_scheduling.sql"),
    },
    Migration {
        version: 6,
        name: "0006_review_threads.sql",
        sql: include_str!("../../migrations/sqlite/0006_review_threads.sql"),
    },
    Migration {
        version: 7,
        name: "0007_task_sources.sql",
        sql: include_str!("../../migrations/sqlite/0007_task_sources.sql"),
    },
    Migration {
        version: 8,
        name: "0008_task_attempt_limits.sql",
        sql: include_str!("../../migrations/sqlite/0008_task_attempt_limits.sql"),
    },
    Migration {
        version: 9,
        name: "0009_proposal_reviewers.sql",
        sql: include_str!("../../migrations/sqlite/0009_proposal_reviewers.sql"),
    },
    Migration {
        version: 10,
        name: "0010_review_drafts.sql",
        sql: include_str!("../../migrations/sqlite/0010_review_drafts.sql"),
    },
];

pub(crate) const POSTGRES_MIGRATIONS: &[Migration] = &[
    Migration { version: 1, name: "0001_init.sql", sql: include_str!("../../migrations/postgres/0001_init.sql") },
    Migration { version: 2, name: "0002_jobs.sql", sql: include_str!("../../migrations/postgres/0002_jobs.sql") },
    Migration {
        version: 3,
        name: "0003_proposal_paging.sql",
        sql: include_str!("../../migrations/postgres/0003_proposal_paging.sql"),
    },
    Migration { version: 4, name: "0004_issues.sql", sql: include_str!("../../migrations/postgres/0004_issues.sql") },
    Migration {
        version: 5,
        name: "0005_task_scheduling.sql",
        sql: include_str!("../../migrations/postgres/0005_task_scheduling.sql"),
    },
    Migration {
        version: 6,
        name: "0006_review_threads.sql",
        sql: include_str!("../../migrations/postgres/0006_review_threads.sql"),
    },
    Migration {
        version: 7,
        name: "0007_task_sources.sql",
        sql: include_str!("../../migrations/postgres/0007_task_sources.sql"),
    },
    Migration {
        version: 8,
        name: "0008_task_attempt_limits.sql",
        sql: include_str!("../../migrations/postgres/0008_task_attempt_limits.sql"),
    },
    Migration {
        version: 9,
        name: "0009_proposal_reviewers.sql",
        sql: include_str!("../../migrations/postgres/0009_proposal_reviewers.sql"),
    },
    Migration {
        version: 10,
        name: "0010_review_drafts.sql",
        sql: include_str!("../../migrations/postgres/0010_review_drafts.sql"),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn files(backend: &str) -> Vec<String> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations").join(backend);
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.ends_with(".sql"))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn every_migration_file_is_listed_in_order_for_both_backends() {
        for (backend, list) in [("sqlite", SQLITE_MIGRATIONS), ("postgres", POSTGRES_MIGRATIONS)] {
            let listed: Vec<&str> = list.iter().map(|m| m.name).collect();
            assert_eq!(files(backend), listed, "{backend}: migrations/ and the list in migrate.rs disagree");
            for (i, m) in list.iter().enumerate() {
                assert_eq!(m.version, i as i64 + 1, "{backend}: versions are consecutive from 1");
                assert!(
                    m.name.starts_with(&format!("{:04}_", m.version)),
                    "{backend}: {} is numbered {}",
                    m.name,
                    m.version
                );
                assert!(!m.sql.trim().is_empty());
            }
            assert_eq!(list.last().unwrap().version, SCHEMA_VERSION, "{backend}");
        }
    }
}
