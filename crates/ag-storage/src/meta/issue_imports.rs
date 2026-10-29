//! Durable mapping from an external issue to its imported local issue.
use super::{IssueActivityRow, IssueRow, MetaError, Tx, db::params};

/// The imported source snapshot, retained without impersonating its original author.
#[derive(Clone, Debug)]
pub struct IssueImportRow {
    /// Hosting provider.
    pub provider: String,
    /// Canonical source repository URL.
    pub source_repo: String,
    /// Provider-local issue number or iid.
    pub source_id: String,
    /// Digest of the complete source issue and its comments.
    pub digest: String,
    /// Created local issue.
    pub issue_number: u64,
    /// Canonical source issue JSON, including original authors, times and URLs.
    pub source_json: String,
    /// Authenticated importing actor.
    pub imported_by: String,
}
impl Tx<'_> {
    /// Reads a durable source mapping.
    pub fn issue_import(
        &self,
        provider: &str,
        source_repo: &str,
        source_id: &str,
    ) -> Result<Option<IssueImportRow>, MetaError> {
        self.conn.query_opt("SELECT digest,issue_number,source_json,imported_by FROM issue_imports WHERE provider=?1 AND source_repo=?2 AND source_id=?3", &params![provider,source_repo,source_id])?.map(|row|Ok(IssueImportRow{provider:provider.into(),source_repo:source_repo.into(),source_id:source_id.into(),digest:row.string(0)?,issue_number:row.i64(1)? as u64,source_json:row.string(2)?,imported_by:row.string(3)?})).transpose()
    }
    /// Inserts an issue with external state and timestamps in the caller's transaction.
    pub fn insert_imported_issue(&self, row: &IssueRow) -> Result<IssueRow, MetaError> {
        let search = format!("{}\n{}", row.title, row.body).to_lowercase();
        let number=self.conn.query_one("INSERT INTO issues(title,body,author,state,created_at,updated_at,revision,task_id,search_text) VALUES(?1,?2,?3,?4,?5,?6,1,NULL,?7) RETURNING number", &params![&row.title,&row.body,&row.author,&row.state,row.created_at_ms,row.updated_at_ms,search])?.i64(0)? as u64;
        self.issue(number)?.ok_or_else(|| MetaError::Corrupt("imported issue missing".into()))
    }
    /// Adds source-timed activity; its author remains the authenticated importer.
    pub fn insert_imported_activity(&self, row: &IssueActivityRow) -> Result<(), MetaError> {
        self.conn.execute(
            "INSERT INTO issue_activity(issue_number,kind,body,author,created_at) VALUES(?1,?2,?3,?4,?5)",
            &params![row.issue_number as i64, &row.kind, &row.body, &row.author, row.created_at_ms],
        )?;
        Ok(())
    }
    /// Commits the mapping in the same transaction as the issue and all comments.
    pub fn insert_issue_import(&self, row: &IssueImportRow) -> Result<(), MetaError> {
        self.conn.execute("INSERT INTO issue_imports(provider,source_repo,source_id,digest,issue_number,source_json,imported_by,imported_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", &params![&row.provider,&row.source_repo,&row.source_id,&row.digest,row.issue_number as i64,&row.source_json,&row.imported_by,self.now().as_millis()])?;
        Ok(())
    }
}
