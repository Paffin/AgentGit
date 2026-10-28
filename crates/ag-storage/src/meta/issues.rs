//! Repository-local user issues. Mutations compose with audit and idempotency in one transaction.
use super::{MetaError, Row, Tx, db::params};

/// The durable user-facing issue, separate from executable agent tasks.
#[derive(Clone, Debug)]
pub struct IssueRow {
    /// Repository-local issue number.
    pub number: u64,
    /// Short issue title.
    pub title: String,
    /// Plain text source for the description or comment.
    pub body: String,
    /// Creating principal.
    pub author: String,
    /// Open or closed.
    pub state: String,
    /// Creation time in milliseconds.
    pub created_at_ms: i64,
    /// Last issue update in milliseconds.
    pub updated_at_ms: i64,
    /// Optimistic concurrency version.
    pub revision: u64,
    /// Optional task in the same repository.
    pub task_id: Option<String>,
}
/// An immutable comment or lifecycle event.
#[derive(Clone, Debug)]
pub struct IssueActivityRow {
    /// Monotonic activity identifier.
    pub id: u64,
    /// Owning issue number.
    pub issue_number: u64,
    /// Created, updated, closed, reopened or commented.
    pub kind: String,
    /// Plain text source for the description or comment.
    pub body: String,
    /// Creating principal.
    pub author: String,
    /// Creation time in milliseconds.
    pub created_at_ms: i64,
}
/// Bounded, newest-first issue listing parameters.
pub struct IssueFilter<'a> {
    /// Open or closed.
    pub state: &'a str,
    /// Literal Unicode-lowercased search text.
    pub query: &'a str,
    /// Exclusive issue-number cursor; zero starts at the newest issue.
    pub before: u64,
    /// Maximum rows to return.
    pub limit: usize,
}
const COLS: &str = "number,title,body,author,state,created_at,updated_at,revision,task_id";
fn issue(r: &Row) -> Result<IssueRow, MetaError> {
    Ok(IssueRow {
        number: r.i64(0)? as u64,
        title: r.string(1)?,
        body: r.string(2)?,
        author: r.string(3)?,
        state: r.string(4)?,
        created_at_ms: r.i64(5)?,
        updated_at_ms: r.i64(6)?,
        revision: r.i64(7)? as u64,
        task_id: r.opt_string(8)?,
    })
}
fn activity(r: &Row) -> Result<IssueActivityRow, MetaError> {
    Ok(IssueActivityRow {
        id: r.i64(0)? as u64,
        issue_number: r.i64(1)? as u64,
        kind: r.string(2)?,
        body: r.string(3)?,
        author: r.string(4)?,
        created_at_ms: r.i64(5)?,
    })
}
impl Tx<'_> {
    /// Reads one issue.
    pub fn issue(&self, number: u64) -> Result<Option<IssueRow>, MetaError> {
        self.conn
            .query_opt(&format!("SELECT {COLS} FROM issues WHERE number=?1"), &params![number as i64])?
            .as_ref()
            .map(issue)
            .transpose()
    }
    /// Returns a bounded filtered page.
    pub fn issues(&self, f: IssueFilter<'_>) -> Result<Vec<IssueRow>, MetaError> {
        let pattern = format!("%{}%", super::escape_like(&f.query.to_lowercase()));
        self.conn.query(&format!("SELECT {COLS} FROM issues WHERE (?1='' OR state=?1) AND search_text LIKE ?2 ESCAPE '\\' AND (?3=0 OR number<?3) ORDER BY number DESC LIMIT ?4"), &params![f.state, pattern, f.before as i64, f.limit as i64])?.iter().map(issue).collect()
    }
    /// Creates an open issue in the current write transaction.
    pub fn insert_issue(
        &self,
        title: &str,
        body: &str,
        author: &str,
        task: Option<&str>,
    ) -> Result<IssueRow, MetaError> {
        let now = self.now().as_millis();
        let search = format!("{title}\n{body}").to_lowercase();
        let number=self.conn.query_one("INSERT INTO issues(title,body,author,state,created_at,updated_at,revision,task_id,search_text) VALUES(?1,?2,?3,'open',?4,?4,1,?5,?6) RETURNING number", &params![title,body,author,now,task,search])?.i64(0)? as u64;
        self.issue(number)?.ok_or_else(|| MetaError::Corrupt("new issue missing".into()))
    }
    /// Applies a compare-and-swap update; false means a stale revision.
    pub fn update_issue(&self, row: &IssueRow, expected: u64) -> Result<bool, MetaError> {
        let search = format!("{}\n{}", row.title, row.body).to_lowercase();
        Ok(self.conn.execute("UPDATE issues SET title=?2,body=?3,state=?4,updated_at=?5,revision=revision+1,task_id=?6,search_text=?7 WHERE number=?1 AND revision=?8", &params![row.number as i64,&row.title,&row.body,&row.state,self.now().as_millis(),row.task_id.as_deref(),search,expected as i64])? == 1)
    }
    /// Appends an immutable discussion or lifecycle event.
    pub fn add_issue_activity(
        &self,
        number: u64,
        kind: &str,
        body: &str,
        author: &str,
    ) -> Result<IssueActivityRow, MetaError> {
        let r=self.conn.query_one("INSERT INTO issue_activity(issue_number,kind,body,author,created_at) VALUES(?1,?2,?3,?4,?5) RETURNING id,issue_number,kind,body,author,created_at", &params![number as i64,kind,body,author,self.now().as_millis()])?;
        activity(&r)
    }
    /// Returns activities after an exclusive cursor.
    pub fn issue_activity(&self, number: u64, after: u64, limit: usize) -> Result<Vec<IssueActivityRow>, MetaError> {
        self.conn.query("SELECT id,issue_number,kind,body,author,created_at FROM issue_activity WHERE issue_number=?1 AND id>?2 ORDER BY id LIMIT ?3", &params![number as i64,after as i64,limit as i64])?.iter().map(activity).collect()
    }
}
