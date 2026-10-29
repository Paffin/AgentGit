//! Account-private file bytes, never stored as published objects.
use super::db::{Row, params};
use super::{MetaError, Tx, parse, ts};
use ag_types::{BlobId, ChangesetId, Timestamp};

/// A private draft; null base is a versioned clear tombstone.
#[derive(Clone, Debug)]
pub struct FileDraftRow {
    /// Branch for this draft.
    pub stream: String,
    /// Existing file's immutable path.
    pub path: String,
    /// Per-slot optimistic version.
    pub version: u64,
    /// Original revision, retained until cleared.
    pub base: Option<ChangesetId>,
    /// Original file blob, derived by the server.
    pub original_blob_id: Option<BlobId>,
    /// Original executable bit.
    pub executable: bool,
    /// Original regular/symlink kind.
    pub file_type: String,
    /// Private replacement bytes.
    pub content: Vec<u8>,
    /// Last save or clear.
    pub updated_at: Timestamp,
}
fn decode(row: &Row, with_content: bool) -> Result<FileDraftRow, MetaError> {
    Ok(FileDraftRow {
        stream: row.string(0)?,
        path: row.string(1)?,
        version: row.i64(2)? as u64,
        base: row.opt_str(3)?.map(|s| parse(s, "file draft base")).transpose()?,
        original_blob_id: row.opt_str(4)?.map(|s| parse(s, "original blob")).transpose()?,
        executable: row.i64(5)? != 0,
        file_type: row.string(6)?,
        content: if with_content { row.blob(7)?.to_vec() } else { Vec::new() },
        updated_at: ts(row.i64(if with_content { 8 } else { 7 })?),
    })
}
impl Tx<'_> {
    /// Looks up only the exact actor's slot.
    pub fn file_draft(&self, owner: &str, stream: &str, path: &str) -> Result<Option<FileDraftRow>, MetaError> {
        self.conn.query_opt("SELECT stream,path,version,base,original_blob_id,executable,file_type,content,updated_at FROM file_drafts WHERE owner=?1 AND stream=?2 AND path=?3", &params![owner,stream,path])?.as_ref().map(|row|decode(row,true)).transpose()
    }
    /// Lists at most twenty active drafts, excluding content bytes from the query.
    pub fn file_drafts(&self, owner: &str, stream: &str) -> Result<Vec<FileDraftRow>, MetaError> {
        self.conn.query("SELECT stream,path,version,base,original_blob_id,executable,file_type,updated_at FROM file_drafts WHERE owner=?1 AND stream=?2 AND base IS NOT NULL ORDER BY path LIMIT 20", &params![owner,stream])?.iter().map(|row|decode(row,false)).collect()
    }
    /// Counts active drafts across the actor's streams in this repository.
    pub fn file_draft_count(&self, owner: &str) -> Result<u64, MetaError> {
        Ok(self.conn.query("SELECT COUNT(*) FROM file_drafts WHERE owner=?1 AND base IS NOT NULL", &params![owner])?[0]
            .i64(0)? as u64)
    }
    /// Writes a state already checked in this transaction.
    pub fn replace_file_draft(&self, owner: &str, draft: &FileDraftRow) -> Result<FileDraftRow, MetaError> {
        let version = i64::try_from(draft.version).map_err(|_| MetaError::Invalid("draft version exhausted".into()))?;
        self.conn.execute("INSERT INTO file_drafts(owner,stream,path,version,base,original_blob_id,executable,file_type,content,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(owner,stream,path) DO UPDATE SET version=excluded.version,base=excluded.base,original_blob_id=excluded.original_blob_id,executable=excluded.executable,file_type=excluded.file_type,content=excluded.content,updated_at=excluded.updated_at", &params![owner,&draft.stream,&draft.path,version,draft.base.map(|id|id.to_hex()),draft.original_blob_id.map(|id|id.to_hex()),draft.executable,&draft.file_type,draft.content.as_slice(),self.now().as_millis()])?;
        self.file_draft(owner, &draft.stream, &draft.path)?
            .ok_or_else(|| MetaError::Corrupt("file draft vanished".into()))
    }
}
