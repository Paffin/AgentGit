use ag_object::canon::Writer;
use ag_types::Timestamp;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::db::{Row, params};
use super::{MetaError, Tx, escape_like, ts};

/// A pending or dispatched outbox event.
#[derive(Clone, Debug, Serialize)]
pub struct OutboxRow {
    /// Sequence id.
    pub id: u64,
    /// Event topic, e.g. `changeset.published`.
    pub topic: String,
    /// Event payload.
    pub payload: Value,
    /// Creation time.
    pub created_at: Timestamp,
    /// Dispatch time, if delivered.
    pub dispatched_at: Option<Timestamp>,
    /// Delivery attempts so far.
    pub attempts: u32,
    /// Last delivery error.
    pub last_error: Option<String>,
}

/// One entry of the hash-chained audit log.
#[derive(Clone, Debug, Serialize)]
pub struct AuditEntry {
    /// Sequence number.
    pub seq: u64,
    /// When.
    pub timestamp: Timestamp,
    /// Who.
    pub actor: String,
    /// What, e.g. `stream.update`.
    pub action: String,
    /// On what.
    pub target: String,
    /// Structured details (JSON text as stored).
    pub details: String,
    /// Hash of the previous entry.
    pub prev_hash: String,
    /// Hash of this entry.
    pub hash: String,
}

/// Result of verifying the audit chain.
#[derive(Clone, Debug, Serialize)]
pub struct AuditReport {
    /// Number of entries checked.
    pub entries: u64,
    /// Hash of the last entry (the chain head); anchor it externally.
    pub head: String,
    /// Sequence number of the first entry that fails verification.
    pub first_bad: Option<u64>,
}

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

const OUTBOX_COLS: &str = "id, topic, payload, created_at, dispatched_at, attempts, last_error";
const AUDIT_COLS: &str = "seq, ts, actor, action, target, details, prev_hash, hash";

fn audit_hash(prev: &str, t: i64, actor: &str, action: &str, target: &str, details: &str) -> String {
    let mut w = Writer::new();
    w.str("agentgit.audit.v1").str(prev).i64(t).str(actor).str(action).str(target).str(details);
    hex::encode(Sha256::digest(w.finish()))
}

fn outbox_row(r: &Row) -> Result<OutboxRow, MetaError> {
    Ok(OutboxRow {
        id: r.i64(0)? as u64,
        topic: r.string(1)?,
        payload: serde_json::from_str(r.str(2)?)?,
        created_at: ts(r.i64(3)?),
        dispatched_at: r.opt_i64(4)?.map(ts),
        attempts: r.i64(5)? as u32,
        last_error: r.opt_string(6)?,
    })
}

fn audit_entry(r: &Row) -> Result<AuditEntry, MetaError> {
    Ok(AuditEntry {
        seq: r.i64(0)? as u64,
        timestamp: ts(r.i64(1)?),
        actor: r.string(2)?,
        action: r.string(3)?,
        target: r.string(4)?,
        details: r.string(5)?,
        prev_hash: r.string(6)?,
        hash: r.string(7)?,
    })
}

impl Tx<'_> {
    /// Appends an event to the transactional outbox.
    pub fn outbox_push(&self, topic: &str, payload: &Value) -> Result<u64, MetaError> {
        let id = self
            .conn
            .query_one(
                "INSERT INTO outbox (topic, payload, created_at) VALUES (?1, ?2, ?3) RETURNING id",
                &params![topic, serde_json::to_string(payload)?, self.now().as_millis()],
            )?
            .i64(0)?;
        Ok(id as u64)
    }

    /// Undelivered events, oldest first.
    pub fn outbox_pending(&self, limit: usize) -> Result<Vec<OutboxRow>, MetaError> {
        let rows = self.conn.query(
            &format!("SELECT {OUTBOX_COLS} FROM outbox WHERE dispatched_at IS NULL ORDER BY id LIMIT ?1"),
            &params![limit as i64],
        )?;
        rows.iter().map(outbox_row).collect()
    }

    /// Marks an event delivered.
    pub fn outbox_ack(&self, id: u64) -> Result<(), MetaError> {
        self.conn.execute(
            "UPDATE outbox SET dispatched_at = ?1, attempts = attempts + 1, last_error = NULL WHERE id = ?2",
            &params![self.now().as_millis(), id as i64],
        )?;
        Ok(())
    }

    /// Records a failed delivery attempt (the event stays pending).
    pub fn outbox_fail(&self, id: u64, error: &str) -> Result<(), MetaError> {
        self.conn.execute(
            "UPDATE outbox SET attempts = attempts + 1, last_error = ?1 WHERE id = ?2",
            &params![error, id as i64],
        )?;
        Ok(())
    }

    /// Number of undelivered events.
    pub fn outbox_pending_count(&self) -> Result<u64, MetaError> {
        Ok(self.conn.query_one("SELECT COUNT(*) FROM outbox WHERE dispatched_at IS NULL", &[])?.i64(0)? as u64)
    }

    /// Delivered events, newest first (replay support).
    pub fn outbox_history(&self, limit: usize) -> Result<Vec<OutboxRow>, MetaError> {
        let rows = self
            .conn
            .query(&format!("SELECT {OUTBOX_COLS} FROM outbox ORDER BY id DESC LIMIT ?1"), &params![limit as i64])?;
        rows.iter().map(outbox_row).collect()
    }

    /// Appends an audit event, chained to the previous one.
    pub fn audit_append(
        &self,
        actor: &str,
        action: &str,
        target: &str,
        details: &Value,
    ) -> Result<AuditEntry, MetaError> {
        let prev = match self.conn.query_opt("SELECT hash FROM audit ORDER BY seq DESC LIMIT 1", &[])? {
            Some(r) => r.string(0)?,
            None => GENESIS.to_string(),
        };
        let details = serde_json::to_string(details)?;
        let t = self.now().as_millis();
        let hash = audit_hash(&prev, t, actor, action, target, &details);
        let seq = self
            .conn
            .query_one(
                "INSERT INTO audit (ts, actor, action, target, details, prev_hash, hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) RETURNING seq",
                &params![t, actor, action, target, &details, &prev, &hash],
            )?
            .i64(0)?;
        Ok(AuditEntry {
            seq: seq as u64,
            timestamp: ts(t),
            actor: actor.into(),
            action: action.into(),
            target: target.into(),
            details,
            prev_hash: prev,
            hash,
        })
    }

    /// Reads audit entries, newest first. With a needle, only entries whose
    /// target, actor or details contain it (a run id, proposal id, identity,
    /// changeset id, ...).
    pub fn audit_list(&self, target_contains: Option<&str>, limit: usize) -> Result<Vec<AuditEntry>, MetaError> {
        let pattern = format!("%{}%", escape_like(target_contains.unwrap_or("")));
        // lower() on both sides: SQLite's LIKE is ASCII case-insensitive, PostgreSQL's is not
        let rows = self.conn.query(
            &format!(
                "SELECT {AUDIT_COLS} FROM audit
                 WHERE lower(target) LIKE lower(?1) ESCAPE '\\' OR lower(actor) LIKE lower(?1) ESCAPE '\\'
                    OR lower(details) LIKE lower(?1) ESCAPE '\\'
                 ORDER BY seq DESC LIMIT ?2"
            ),
            &params![pattern, limit as i64],
        )?;
        rows.iter().map(audit_entry).collect()
    }

    /// Audit entries with `seq > after_seq`, oldest first: the event feed.
    pub fn audit_after(&self, after_seq: u64, limit: usize) -> Result<Vec<AuditEntry>, MetaError> {
        let rows = self.conn.query(
            &format!("SELECT {AUDIT_COLS} FROM audit WHERE seq > ?1 ORDER BY seq LIMIT ?2"),
            &params![after_seq as i64, limit as i64],
        )?;
        rows.iter().map(audit_entry).collect()
    }

    /// The highest audit sequence number (0 when the log is empty).
    pub fn audit_latest_seq(&self) -> Result<u64, MetaError> {
        let rows = self.conn.query("SELECT COALESCE(MAX(seq), 0) FROM audit", &[])?;
        Ok(rows.first().map(|r| r.i64(0)).transpose()?.unwrap_or(0).max(0) as u64)
    }

    /// Recomputes the whole chain and reports the first inconsistent entry.
    pub fn audit_verify(&self) -> Result<AuditReport, MetaError> {
        const PAGE: i64 = 2000;
        let mut prev = GENESIS.to_string();
        let mut n = 0u64;
        let mut first_bad = None;
        let mut after = i64::MIN;
        loop {
            // paged so memory stays flat however long the log is
            let rows = self.conn.query(
                &format!("SELECT {AUDIT_COLS} FROM audit WHERE seq > ?1 ORDER BY seq LIMIT ?2"),
                &params![after, PAGE],
            )?;
            for row in &rows {
                let e = audit_entry(row)?;
                n += 1;
                let expect =
                    audit_hash(&e.prev_hash, e.timestamp.as_millis(), &e.actor, &e.action, &e.target, &e.details);
                if first_bad.is_none() && (e.prev_hash != prev || expect != e.hash) {
                    first_bad = Some(e.seq);
                }
                after = e.seq as i64;
                prev = e.hash;
            }
            if (rows.len() as i64) < PAGE {
                break;
            }
        }
        Ok(AuditReport { entries: n, head: prev, first_bad })
    }
}
