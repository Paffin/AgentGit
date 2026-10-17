use ag_types::{ChangesetId, StreamName, Timestamp};
use serde::Serialize;

use super::db::{Row, params};
use super::{MetaError, Tx, parse, stream_name, ts};

/// The full compare-and-swap token of a stream: head *and* version, so a
/// head that moves A → B → A is still detected (no ABA problem).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct StreamState {
    /// Current head, `None` for a stream that has no changesets yet.
    pub head: Option<ChangesetId>,
    /// Number of successful updates so far.
    pub version: u64,
}

/// A stream.
#[derive(Clone, Debug, Serialize)]
pub struct StreamRow {
    /// Name.
    pub name: StreamName,
    /// Current CAS token.
    pub state: StreamState,
    /// Creation time.
    pub created_at: Timestamp,
    /// Creator identity.
    pub created_by: String,
}

/// One append-only reflog record.
#[derive(Clone, Debug, Serialize)]
pub struct ReflogEntry {
    /// Monotonic sequence number.
    pub seq: u64,
    /// Stream name.
    pub stream: StreamName,
    /// Head before the update.
    pub old_head: Option<ChangesetId>,
    /// Head after the update.
    pub new_head: Option<ChangesetId>,
    /// Stream version after the update.
    pub version: u64,
    /// Operation that caused the update.
    pub operation_id: Option<String>,
    /// Who did it.
    pub actor: String,
    /// When.
    pub timestamp: Timestamp,
    /// Why.
    pub reason: String,
}

fn opt_id(s: Option<String>, what: &str) -> Result<Option<ChangesetId>, MetaError> {
    s.map(|s| parse::<ChangesetId>(&s, what)).transpose()
}

fn stream_row(r: &Row) -> Result<StreamRow, MetaError> {
    Ok(StreamRow {
        name: stream_name(r.str(0)?)?,
        state: StreamState { head: opt_id(r.opt_string(1)?, "stream head")?, version: r.i64(2)? as u64 },
        created_at: ts(r.i64(3)?),
        created_by: r.string(4)?,
    })
}

impl Tx<'_> {
    /// Creates a stream with no head.
    pub fn create_stream(&self, name: &StreamName, actor: &str) -> Result<StreamRow, MetaError> {
        let exists =
            self.conn.query_one("SELECT COUNT(*) FROM streams WHERE name = ?1", &params![name.as_str()])?.i64(0)?;
        if exists > 0 {
            return Err(MetaError::AlreadyExists(format!("stream '{name}'")));
        }
        self.conn.execute(
            "INSERT INTO streams (name, head, version, created_at, created_by) VALUES (?1, NULL, 0, ?2, ?3)",
            &params![name.as_str(), self.now().as_millis(), actor],
        )?;
        Ok(StreamRow {
            name: name.clone(),
            state: StreamState { head: None, version: 0 },
            created_at: self.now(),
            created_by: actor.to_string(),
        })
    }

    /// Looks up a stream.
    pub fn stream(&self, name: &str) -> Result<Option<StreamRow>, MetaError> {
        self.conn
            .query_opt(
                "SELECT name, head, version, created_at, created_by FROM streams WHERE name = ?1",
                &params![name],
            )?
            .map(|r| stream_row(&r))
            .transpose()
    }

    /// Lists all streams by name.
    pub fn list_streams(&self) -> Result<Vec<StreamRow>, MetaError> {
        self.conn
            .query("SELECT name, head, version, created_at, created_by FROM streams ORDER BY name", &[])?
            .iter()
            .map(stream_row)
            .collect()
    }

    /// Atomically moves a stream head if (and only if) its state still equals
    /// `expected`. Appends the reflog entry in the same transaction and
    /// returns the new state.
    pub fn cas_stream(
        &self,
        name: &str,
        expected: &StreamState,
        new_head: &ChangesetId,
        operation_id: Option<&str>,
        actor: &str,
        reason: &str,
    ) -> Result<StreamState, MetaError> {
        let current = self.stream(name)?.ok_or_else(|| MetaError::NotFound(format!("stream '{name}'")))?;
        if &current.state != expected {
            return Err(MetaError::CasMismatch {
                stream: name.to_string(),
                expected: expected.version,
                actual: current.state.version,
            });
        }
        if !self.is_ready(new_head)? {
            return Err(MetaError::Invalid(format!("changeset {new_head} is not published")));
        }
        let version = expected.version + 1;
        let changed = self.conn.execute(
            "UPDATE streams SET head = ?1, version = ?2 WHERE name = ?3 AND version = ?4",
            &params![new_head.to_hex(), version as i64, name, expected.version as i64],
        )?;
        if changed != 1 {
            return Err(MetaError::CasMismatch {
                stream: name.to_string(),
                expected: expected.version,
                actual: expected.version,
            });
        }
        self.conn.execute(
            "INSERT INTO reflog (stream, old_head, new_head, version, operation_id, actor, ts, reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            &params![
                name,
                expected.head.map(|h| h.to_hex()),
                new_head.to_hex(),
                version as i64,
                operation_id,
                actor,
                self.now().as_millis(),
                reason,
            ],
        )?;
        Ok(StreamState { head: Some(*new_head), version })
    }

    /// The newest `limit` reflog entries of a stream, newest first.
    pub fn reflog(&self, stream: &str, limit: usize) -> Result<Vec<ReflogEntry>, MetaError> {
        let rows = self.conn.query(
            "SELECT seq, stream, old_head, new_head, version, operation_id, actor, ts, reason
             FROM reflog WHERE stream = ?1 ORDER BY seq DESC LIMIT ?2",
            &params![stream, limit as i64],
        )?;
        rows.iter()
            .map(|r| {
                Ok(ReflogEntry {
                    seq: r.i64(0)? as u64,
                    stream: stream_name(r.str(1)?)?,
                    old_head: opt_id(r.opt_string(2)?, "reflog old head")?,
                    new_head: opt_id(r.opt_string(3)?, "reflog new head")?,
                    version: r.i64(4)? as u64,
                    operation_id: r.opt_string(5)?,
                    actor: r.string(6)?,
                    timestamp: ts(r.i64(7)?),
                    reason: r.string(8)?,
                })
            })
            .collect()
    }

    /// Reflog entries of all streams newer than `since` (retention roots for GC).
    pub fn reflog_heads_since(&self, since: Timestamp) -> Result<Vec<ChangesetId>, MetaError> {
        let rows = self.conn.query(
            "SELECT DISTINCT new_head FROM reflog WHERE new_head IS NOT NULL AND ts >= ?1",
            &params![since.as_millis()],
        )?;
        rows.iter().map(|r| parse::<ChangesetId>(r.str(0)?, "reflog head")).collect()
    }
}
