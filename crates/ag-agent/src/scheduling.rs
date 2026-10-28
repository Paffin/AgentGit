//! Repository-wide gate for starting task-bound agent runs.
use crate::AgentError;
use ag_storage::{
    Meta,
    meta::{MetaError, Tx},
};
use serde::{Deserialize, Serialize};

const KEY: &str = "task_scheduling";

/// The persisted operator decision; pausing does not cancel existing runs.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct TaskScheduling {
    /// Whether new task-bound runs are refused.
    pub paused: bool,
    /// Compare-and-swap revision, initially zero.
    pub version: u64,
    /// Optional operator explanation.
    pub reason: String,
    /// Last operator identity.
    pub updated_by: String,
    /// Last change, in milliseconds since the epoch.
    pub updated_at_ms: i64,
}

pub(crate) fn in_tx(tx: &Tx<'_>) -> Result<TaskScheduling, MetaError> {
    tx.meta_get(KEY)?
        .map(|s| serde_json::from_str(&s).map_err(MetaError::from))
        .transpose()
        .map(Option::unwrap_or_default)
}

/// Reads the scheduling gate from the repository metadata.
pub fn get(meta: &Meta) -> Result<TaskScheduling, AgentError> {
    Ok(meta.read(in_tx)?)
}

/// Changes the gate atomically with its audit and event, refusing a stale decision.
pub fn set(meta: &Meta, paused: bool, reason: &str, expected: u64, actor: &str) -> Result<TaskScheduling, AgentError> {
    validate_reason(reason)?;
    meta.write(|tx| {
        let current = in_tx(tx)?;
        if current.version != expected {
            return Err(MetaError::Invalid("task scheduling changed; reload before applying your decision".into()));
        }
        let next = next_decision(&current, tx, paused, reason, actor)?;
        persist(tx, &next)?;
        Ok(next)
    })
    .map_err(|e| match e {
        MetaError::Invalid(m) => AgentError::Conflict(m),
        other => AgentError::Meta(other),
    })
}

fn validate_reason(reason: &str) -> Result<(), AgentError> {
    if reason.len() > 1024 || reason.contains('\0') {
        return Err(AgentError::Invalid(
            "scheduling reason must be at most 1024 UTF-8 bytes and contain no NUL".into(),
        ));
    }
    Ok(())
}

fn next_decision(
    current: &TaskScheduling,
    tx: &Tx<'_>,
    paused: bool,
    reason: &str,
    actor: &str,
) -> Result<TaskScheduling, MetaError> {
    Ok(TaskScheduling {
        paused,
        reason: if paused { reason.trim().into() } else { String::new() },
        version: current
            .version
            .checked_add(1)
            .ok_or_else(|| MetaError::Corrupt("scheduling revision exhausted".into()))?,
        updated_by: actor.into(),
        updated_at_ms: tx.now().as_millis(),
    })
}

fn persist(tx: &Tx<'_>, next: &TaskScheduling) -> Result<(), MetaError> {
    let value = serde_json::to_value(next)?;
    tx.meta_set(KEY, &value.to_string())?;
    tx.audit_append(&next.updated_by, "task.scheduling", "repository", &value)?;
    tx.outbox_push("task.scheduling", &value)?;
    Ok(())
}
