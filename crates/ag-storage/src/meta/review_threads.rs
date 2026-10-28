//! Immutable line anchors and versioned discussion state, independent of approvals.
use super::{MetaError, Row, Tx, db::params, parse};
use ag_types::ChangesetId;

/// One revision-bound discussion.
#[derive(Clone, Debug)]
pub struct ReviewThreadRow {
    /// Opaque thread identifier.
    pub id: String,
    /// Owning proposal.
    pub proposal: String,
    /// Immutable source location and text.
    pub anchor: ReviewThreadAnchorRow,
    /// Initial discussion body.
    pub body: String,
    /// Initial author.
    pub author: String,
    /// Whether the discussion is resolved.
    pub resolved: bool,
    /// Changed by both replies and resolution updates.
    pub version: u64,
    /// Creation time in milliseconds.
    pub created_at_ms: i64,
    /// Last activity time in milliseconds.
    pub updated_at_ms: i64,
}
/// Immutable source location for a line discussion.
#[derive(Clone, Debug)]
pub struct ReviewThreadAnchorRow {
    /// Old comparison revision, absent for an empty base tree.
    pub base: Option<ChangesetId>,
    /// Inspected proposal head.
    pub head: ChangesetId,
    /// Repository-relative file path.
    pub path: String,
    /// Old or new side of the comparison.
    pub side: String,
    /// One-based source line.
    pub line: u32,
    /// Line read from the immutable source by the server.
    pub source_text: String,
}
/// Immutable reply to a thread.
#[derive(Clone, Debug)]
pub struct ThreadCommentRow {
    /// Monotonic reply ID.
    pub id: u64,
    /// Owning thread.
    pub thread_id: String,
    /// Plain text body.
    pub body: String,
    /// Reply author.
    pub author: String,
    /// Creation time in milliseconds.
    pub created_at_ms: i64,
}
const COLS: &str =
    "id,proposal,source_base,source_head,path,side,line,source_text,body,author,resolved,version,created_at,updated_at";
fn anchor(row: &Row) -> Result<ReviewThreadAnchorRow, MetaError> {
    Ok(ReviewThreadAnchorRow {
        base: row.opt_string(2)?.map(|v| parse(&v, "thread base")).transpose()?,
        head: parse(row.str(3)?, "thread head")?,
        path: row.string(4)?,
        side: row.string(5)?,
        line: row.i64(6)? as u32,
        source_text: row.string(7)?,
    })
}
fn times(row: &Row) -> Result<(i64, i64), MetaError> {
    Ok((row.i64(12)?, row.i64(13)?))
}
fn thread(row: &Row) -> Result<ReviewThreadRow, MetaError> {
    let (created_at_ms, updated_at_ms) = times(row)?;
    Ok(ReviewThreadRow {
        id: row.string(0)?,
        proposal: row.string(1)?,
        anchor: anchor(row)?,
        body: row.string(8)?,
        author: row.string(9)?,
        resolved: row.i64(10)? != 0,
        version: row.i64(11)? as u64,
        created_at_ms,
        updated_at_ms,
    })
}
fn comment(row: &Row) -> Result<ThreadCommentRow, MetaError> {
    Ok(ThreadCommentRow {
        id: row.i64(0)? as u64,
        thread_id: row.string(1)?,
        body: row.string(2)?,
        author: row.string(3)?,
        created_at_ms: row.i64(4)?,
    })
}
impl Tx<'_> {
    /// Immutable source revisions retained by review history.
    pub fn review_thread_roots(&self) -> Result<Vec<ChangesetId>, MetaError> {
        self.conn.query("SELECT source_head FROM review_threads UNION SELECT source_base FROM review_threads WHERE source_base IS NOT NULL", &[])?.iter().map(|row| parse(row.str(0)?,"thread source revision")).collect()
    }

    /// Reads a thread only inside its owning proposal.
    pub fn review_thread(&self, proposal: &str, id: &str) -> Result<Option<ReviewThreadRow>, MetaError> {
        self.conn
            .query_opt(
                &format!("SELECT {COLS} FROM review_threads WHERE proposal=?1 AND id=?2"),
                &params![proposal, id],
            )?
            .as_ref()
            .map(thread)
            .transpose()
    }
    /// Newest-first keyset page, independent of future replies or resolution changes.
    pub fn review_threads(
        &self,
        proposal: &str,
        after: Option<(i64, &str)>,
        limit: usize,
    ) -> Result<Vec<ReviewThreadRow>, MetaError> {
        let (time, id) = after.unwrap_or((i64::MAX, ""));
        self.conn.query(&format!("SELECT {COLS} FROM review_threads WHERE proposal=?1 AND (created_at<?2 OR (created_at=?2 AND id<?3)) ORDER BY created_at DESC,id DESC LIMIT ?4"), &params![proposal,time,id,limit as i64])?.iter().map(thread).collect()
    }
    /// Inserts the first immutable message and anchor in the caller's transaction.
    pub fn insert_review_thread(&self, r: &ReviewThreadRow) -> Result<(), MetaError> {
        self.conn.execute("INSERT INTO review_threads(id,proposal,source_base,source_head,path,side,line,source_text,body,author,resolved,version,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,0,1,?11,?11)", &params![&r.id,&r.proposal,r.anchor.base.map(|id| id.to_hex()),r.anchor.head.to_hex(),&r.anchor.path,&r.anchor.side,i64::from(r.anchor.line),&r.anchor.source_text,&r.body,&r.author,self.now().as_millis()])?;
        Ok(())
    }
    /// Appends a reply and increments the thread revision atomically.
    pub fn add_thread_comment(&self, id: &str, body: &str, author: &str) -> Result<ThreadCommentRow, MetaError> {
        let now = self.now().as_millis();
        if self.conn.execute(
            "UPDATE review_threads SET version=version+1,updated_at=?2 WHERE id=?1 AND resolved=0",
            &params![id, now],
        )? != 1
        {
            return Err(MetaError::Invalid("reopen the resolved thread before replying".into()));
        }
        let row = self.conn.query_one("INSERT INTO review_thread_comments(thread_id,body,author,created_at) VALUES(?1,?2,?3,?4) RETURNING id,thread_id,body,author,created_at", &params![id,body,author,now])?;
        comment(&row)
    }
    /// Oldest-first reply page after an exclusive ID.
    pub fn thread_comments(&self, id: &str, after: u64, limit: usize) -> Result<Vec<ThreadCommentRow>, MetaError> {
        self.conn.query("SELECT id,thread_id,body,author,created_at FROM review_thread_comments WHERE thread_id=?1 AND id>?2 ORDER BY id LIMIT ?3", &params![id,after as i64,limit as i64])?.iter().map(comment).collect()
    }
    /// Compare-and-swap resolution; a concurrent reply also invalidates the version.
    pub fn resolve_review_thread(&self, id: &str, expected: u64, resolved: bool) -> Result<bool, MetaError> {
        Ok(self.conn.execute(
            "UPDATE review_threads SET resolved=?2,version=version+1,updated_at=?3 WHERE id=?1 AND version=?4",
            &params![id, i64::from(resolved), self.now().as_millis(), expected as i64],
        )? == 1)
    }
}
