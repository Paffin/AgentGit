use ag_types::{ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::db::{Row, params};
use super::{MetaError, Tx, escape_like, parse, ts};

/// Lifecycle of a publication intent.
///
/// ```text
/// publish:    Prepared -> Uploaded -> Verified -> Published
/// integrate:  Prepared -> Ready -> Integrated
///                              \-> Blocked (policy/validation/approval missing; retryable)
///                              \-> Rejected (conflict, policy denial; terminal)
/// any non-terminal -> Failed | Aborted
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentState {
    /// Intent recorded, idempotency key reserved.
    Prepared,
    /// Objects were handed to the object store.
    Uploaded,
    /// Hashes, sizes, parent links and durability were verified.
    Verified,
    /// The head is published and the integration may be attempted.
    Ready,
    /// Terminal state of a publish intent: objects and DAG are registered.
    Published,
    /// Waiting for validation, approval or a lease; can be retried.
    Blocked,
    /// The stream was atomically updated.
    Integrated,
    /// Terminal: refused (conflict, policy).
    Rejected,
    /// Terminal: failed for a non-business reason.
    Failed,
    /// Terminal: abandoned or expired.
    Aborted,
}

impl IntentState {
    /// Stored name.
    pub fn as_str(self) -> &'static str {
        match self {
            IntentState::Prepared => "prepared",
            IntentState::Uploaded => "uploaded",
            IntentState::Verified => "verified",
            IntentState::Ready => "ready",
            IntentState::Published => "published",
            IntentState::Blocked => "blocked",
            IntentState::Integrated => "integrated",
            IntentState::Rejected => "rejected",
            IntentState::Failed => "failed",
            IntentState::Aborted => "aborted",
        }
    }

    /// True for states that never change again.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            IntentState::Published
                | IntentState::Integrated
                | IntentState::Rejected
                | IntentState::Failed
                | IntentState::Aborted
        )
    }
}

impl std::fmt::Display for IntentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for IntentState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "prepared" => IntentState::Prepared,
            "uploaded" => IntentState::Uploaded,
            "verified" => IntentState::Verified,
            "ready" => IntentState::Ready,
            "published" => IntentState::Published,
            "blocked" => IntentState::Blocked,
            "integrated" => IntentState::Integrated,
            "rejected" => IntentState::Rejected,
            "failed" => IntentState::Failed,
            "aborted" => IntentState::Aborted,
            other => return Err(format!("unknown intent state '{other}'")),
        })
    }
}

/// Parameters for creating an intent.
pub struct NewIntent<'a> {
    /// Operation identifier (what clients query after losing a response).
    pub operation_id: &'a str,
    /// Idempotency key; replays with the same key return the same intent.
    pub idempotency_key: &'a str,
    /// Hash of the request; a key reused with a different hash is an error.
    pub request_hash: &'a str,
    /// Intent kind, e.g. `publish`.
    pub kind: &'a str,
    /// Acting identity.
    pub actor: &'a str,
    /// Target stream, if any.
    pub stream: Option<&'a str>,
    /// Request payload.
    pub payload: &'a Value,
}

/// A stored intent.
#[derive(Clone, Debug, Serialize)]
pub struct IntentRow {
    /// Operation id.
    pub operation_id: String,
    /// Idempotency key.
    pub idempotency_key: String,
    /// Request hash.
    pub request_hash: String,
    /// Kind.
    pub kind: String,
    /// Current state.
    pub state: IntentState,
    /// Acting identity.
    pub actor: String,
    /// Target stream.
    pub stream: Option<String>,
    /// Request payload.
    pub payload: Value,
    /// Outcome details once decided.
    pub result: Option<Value>,
    /// Error text for failed intents.
    pub error: Option<String>,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last state change.
    pub updated_at: Timestamp,
}

const INTENT_COLS: &str = "operation_id, idempotency_key, request_hash, kind, state, actor, stream, payload, result, error, created_at, updated_at";

fn cook(r: &Row) -> Result<IntentRow, MetaError> {
    Ok(IntentRow {
        operation_id: r.string(0)?,
        idempotency_key: r.string(1)?,
        request_hash: r.string(2)?,
        kind: r.string(3)?,
        state: parse(r.str(4)?, "intent state")?,
        actor: r.string(5)?,
        stream: r.opt_string(6)?,
        payload: serde_json::from_str(r.str(7)?)?,
        result: r.opt_str(8)?.map(serde_json::from_str).transpose()?,
        error: r.opt_string(9)?,
        created_at: ts(r.i64(10)?),
        updated_at: ts(r.i64(11)?),
    })
}

impl Tx<'_> {
    /// Creates an intent, or returns the existing one for a replayed
    /// idempotency key. The boolean is `true` when a new intent was created.
    pub fn intent_create(&self, n: &NewIntent<'_>) -> Result<(IntentRow, bool), MetaError> {
        if let Some(existing) = self.intent_by_key(n.idempotency_key)? {
            if existing.request_hash != n.request_hash {
                return Err(MetaError::IdempotencyKeyReused(n.idempotency_key.to_string()));
            }
            return Ok((existing, false));
        }
        if self.intent(n.operation_id)?.is_some() {
            return Err(MetaError::AlreadyExists(format!("operation '{}'", n.operation_id)));
        }
        let now = self.now().as_millis();
        self.conn.execute(
            "INSERT INTO intents (operation_id, idempotency_key, request_hash, kind, state, actor, stream, payload, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'prepared', ?5, ?6, ?7, ?8, ?8)",
            &params![
                n.operation_id,
                n.idempotency_key,
                n.request_hash,
                n.kind,
                n.actor,
                n.stream,
                serde_json::to_string(n.payload)?,
                now
            ],
        )?;
        let row = self.intent(n.operation_id)?.ok_or_else(|| MetaError::Corrupt("intent vanished".into()))?;
        Ok((row, true))
    }

    /// Looks up an intent by operation id.
    pub fn intent(&self, operation_id: &str) -> Result<Option<IntentRow>, MetaError> {
        self.conn
            .query_opt(&format!("SELECT {INTENT_COLS} FROM intents WHERE operation_id = ?1"), &params![operation_id])?
            .map(|r| cook(&r))
            .transpose()
    }

    /// Looks up an intent by idempotency key.
    pub fn intent_by_key(&self, key: &str) -> Result<Option<IntentRow>, MetaError> {
        self.conn
            .query_opt(&format!("SELECT {INTENT_COLS} FROM intents WHERE idempotency_key = ?1"), &params![key])?
            .map(|r| cook(&r))
            .transpose()
    }

    /// Moves an intent to `to`, but only from one of the `allowed` states.
    pub fn intent_transition(
        &self,
        operation_id: &str,
        allowed: &[IntentState],
        to: IntentState,
        result: Option<&Value>,
        error: Option<&str>,
    ) -> Result<IntentRow, MetaError> {
        let cur =
            self.intent(operation_id)?.ok_or_else(|| MetaError::NotFound(format!("operation '{operation_id}'")))?;
        if !allowed.contains(&cur.state) {
            return Err(MetaError::InvalidTransition { from: cur.state.to_string(), to: to.to_string() });
        }
        let result_text = result.map(serde_json::to_string).transpose()?;
        self.conn.execute(
            "UPDATE intents SET state = ?1, result = COALESCE(?2, result), error = COALESCE(?3, error), updated_at = ?4
             WHERE operation_id = ?5",
            &params![to.as_str(), result_text, error, self.now().as_millis(), operation_id],
        )?;
        self.intent(operation_id)?.ok_or_else(|| MetaError::Corrupt("intent vanished".into()))
    }

    /// Replaces an intent's payload.
    pub fn intent_set_payload(&self, operation_id: &str, payload: &Value) -> Result<(), MetaError> {
        self.conn.execute(
            "UPDATE intents SET payload = ?1, updated_at = ?2 WHERE operation_id = ?3",
            &params![serde_json::to_string(payload)?, self.now().as_millis(), operation_id],
        )?;
        Ok(())
    }

    /// Records an object staged by an intent (protects it from GC).
    pub fn intent_add_object(&self, operation_id: &str, id: &ObjectId) -> Result<(), MetaError> {
        self.conn.execute(
            "INSERT INTO intent_objects (operation_id, object_id) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
            &params![operation_id, id.to_hex()],
        )?;
        Ok(())
    }

    /// Objects staged by an intent.
    pub fn intent_objects(&self, operation_id: &str) -> Result<Vec<ObjectId>, MetaError> {
        let rows = self.conn.query(
            "SELECT object_id FROM intent_objects WHERE operation_id = ?1 ORDER BY object_id",
            &params![operation_id],
        )?;
        rows.iter().map(|r| parse::<ObjectId>(r.str(0)?, "object id")).collect()
    }

    /// Objects staged by intents that are not terminal, or that changed since
    /// `since` (GC protects both).
    pub fn protected_intent_objects(&self, since: Timestamp) -> Result<Vec<ObjectId>, MetaError> {
        let rows = self.conn.query(
            "SELECT DISTINCT o.object_id FROM intent_objects o JOIN intents i ON i.operation_id = o.operation_id
             WHERE i.state NOT IN ('published','integrated','rejected','failed','aborted') OR i.updated_at >= ?1",
            &params![since.as_millis()],
        )?;
        rows.iter().map(|r| parse::<ObjectId>(r.str(0)?, "object id")).collect()
    }

    /// Intents in any of `states`, oldest update first.
    pub fn intents_in_states(&self, states: &[IntentState], limit: usize) -> Result<Vec<IntentRow>, MetaError> {
        let list = states.iter().map(|s| format!("'{}'", s.as_str())).collect::<Vec<_>>().join(",");
        let rows = self.conn.query(
            &format!(
                "SELECT {INTENT_COLS} FROM intents WHERE state IN ({list}) ORDER BY updated_at, operation_id LIMIT ?1"
            ),
            &params![limit as i64],
        )?;
        rows.iter().map(cook).collect()
    }

    /// Intents whose idempotency key starts with `prefix`, oldest first.
    pub fn intents_with_key_prefix(&self, prefix: &str) -> Result<Vec<IntentRow>, MetaError> {
        let like = format!("{}%", escape_like(prefix));
        // lower() on both sides: SQLite's LIKE is ASCII case-insensitive, PostgreSQL's is not
        let rows = self.conn.query(
            &format!(
                "SELECT {INTENT_COLS} FROM intents WHERE lower(idempotency_key) LIKE lower(?1) ESCAPE '\\' ORDER BY created_at, operation_id"
            ),
            &params![like],
        )?;
        rows.iter().map(cook).collect()
    }

    /// The most recent intents, newest first.
    pub fn intents_recent(&self, limit: usize) -> Result<Vec<IntentRow>, MetaError> {
        let rows = self.conn.query(
            &format!("SELECT {INTENT_COLS} FROM intents ORDER BY created_at DESC, operation_id DESC LIMIT ?1"),
            &params![limit as i64],
        )?;
        rows.iter().map(cook).collect()
    }
}
