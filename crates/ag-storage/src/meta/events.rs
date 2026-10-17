//! Reading the audit log as an ordered event feed.

use rusqlite::params;

use super::{AuditEntry, MetaError, Tx, ts};

impl Tx<'_> {
    /// Audit entries with `seq > after_seq`, oldest first.
    pub fn audit_after(&self, after_seq: u64, limit: usize) -> Result<Vec<AuditEntry>, MetaError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT seq, ts, actor, action, target, details, prev_hash, hash FROM audit
             WHERE seq > ?1 ORDER BY seq LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![after_seq as i64, limit as i64], |r| {
            Ok(AuditEntry {
                seq: r.get::<_, i64>(0)? as u64,
                timestamp: ts(r.get(1)?),
                actor: r.get(2)?,
                action: r.get(3)?,
                target: r.get(4)?,
                details: r.get(5)?,
                prev_hash: r.get(6)?,
                hash: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// The highest audit sequence number (0 when the log is empty).
    pub fn audit_latest_seq(&self) -> Result<u64, MetaError> {
        let n: Option<i64> = self.conn.query_row("SELECT MAX(seq) FROM audit", [], |r| r.get(0))?;
        Ok(n.unwrap_or(0).max(0) as u64)
    }
}
