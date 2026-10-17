use ag_object::canon::Writer;
use ag_types::Timestamp;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{MetaError, Tx, ts};

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

fn audit_hash(prev: &str, t: i64, actor: &str, action: &str, target: &str, details: &str) -> String {
    let mut w = Writer::new();
    w.str("agentgit.audit.v1").str(prev).i64(t).str(actor).str(action).str(target).str(details);
    hex::encode(Sha256::digest(w.finish()))
}

impl Tx<'_> {
    /// Appends an event to the transactional outbox.
    pub fn outbox_push(&self, topic: &str, payload: &Value) -> Result<u64, MetaError> {
        self.conn.execute(
            "INSERT INTO outbox (topic, payload, created_at) VALUES (?1, ?2, ?3)",
            params![topic, serde_json::to_string(payload)?, self.now().as_millis()],
        )?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    /// Undelivered events, oldest first.
    pub fn outbox_pending(&self, limit: usize) -> Result<Vec<OutboxRow>, MetaError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, topic, payload, created_at, dispatched_at, attempts, last_error
             FROM outbox WHERE dispatched_at IS NULL ORDER BY id LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, topic, payload, c, d, attempts, err) = r?;
            out.push(OutboxRow {
                id: id as u64,
                topic,
                payload: serde_json::from_str(&payload)?,
                created_at: ts(c),
                dispatched_at: d.map(ts),
                attempts: attempts as u32,
                last_error: err,
            });
        }
        Ok(out)
    }

    /// Marks an event delivered.
    pub fn outbox_ack(&self, id: u64) -> Result<(), MetaError> {
        self.conn.execute(
            "UPDATE outbox SET dispatched_at = ?1, attempts = attempts + 1, last_error = NULL WHERE id = ?2",
            params![self.now().as_millis(), id as i64],
        )?;
        Ok(())
    }

    /// Records a failed delivery attempt (the event stays pending).
    pub fn outbox_fail(&self, id: u64, error: &str) -> Result<(), MetaError> {
        self.conn.execute(
            "UPDATE outbox SET attempts = attempts + 1, last_error = ?1 WHERE id = ?2",
            params![error, id as i64],
        )?;
        Ok(())
    }

    /// Number of undelivered events.
    pub fn outbox_pending_count(&self) -> Result<u64, MetaError> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM outbox WHERE dispatched_at IS NULL", [], |r| r.get::<_, i64>(0))?
            as u64)
    }

    /// Delivered events, newest first (replay support).
    pub fn outbox_history(&self, limit: usize) -> Result<Vec<OutboxRow>, MetaError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, topic, payload, created_at, dispatched_at, attempts, last_error FROM outbox ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, topic, payload, c, d, attempts, err) = r?;
            out.push(OutboxRow {
                id: id as u64,
                topic,
                payload: serde_json::from_str(&payload)?,
                created_at: ts(c),
                dispatched_at: d.map(ts),
                attempts: attempts as u32,
                last_error: err,
            });
        }
        Ok(out)
    }

    /// Appends an audit event, chained to the previous one.
    pub fn audit_append(
        &self,
        actor: &str,
        action: &str,
        target: &str,
        details: &Value,
    ) -> Result<AuditEntry, MetaError> {
        let prev: String = self
            .conn
            .query_row("SELECT hash FROM audit ORDER BY seq DESC LIMIT 1", [], |r| r.get(0))
            .optional()?
            .unwrap_or_else(|| GENESIS.to_string());
        let details = serde_json::to_string(details)?;
        let t = self.now().as_millis();
        let hash = audit_hash(&prev, t, actor, action, target, &details);
        self.conn.execute(
            "INSERT INTO audit (ts, actor, action, target, details, prev_hash, hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![t, actor, action, target, details, prev, hash],
        )?;
        Ok(AuditEntry {
            seq: self.conn.last_insert_rowid() as u64,
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
        let pattern = format!("%{}%", target_contains.unwrap_or("").replace('%', "\\%").replace('_', "\\_"));
        let mut stmt = self.conn.prepare_cached(
            "SELECT seq, ts, actor, action, target, details, prev_hash, hash FROM audit
             WHERE target LIKE ?1 ESCAPE '\\' OR actor LIKE ?1 ESCAPE '\\' OR details LIKE ?1 ESCAPE '\\'
             ORDER BY seq DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, limit as i64], |r| {
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

    /// Recomputes the whole chain and reports the first inconsistent entry.
    pub fn audit_verify(&self) -> Result<AuditReport, MetaError> {
        let mut stmt = self
            .conn
            .prepare("SELECT seq, ts, actor, action, target, details, prev_hash, hash FROM audit ORDER BY seq")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
            ))
        })?;
        let mut prev = GENESIS.to_string();
        let mut n = 0u64;
        let mut first_bad = None;
        for row in rows {
            let (seq, t, actor, action, target, details, prev_hash, hash) = row?;
            n += 1;
            let expect = audit_hash(&prev_hash, t, &actor, &action, &target, &details);
            if first_bad.is_none() && (prev_hash != prev || expect != hash) {
                first_bad = Some(seq as u64);
            }
            prev = hash;
        }
        Ok(AuditReport { entries: n, head: prev, first_bad })
    }
}
