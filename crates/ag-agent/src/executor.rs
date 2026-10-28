//! Expiring, operator-reported executor connectivity; never an authorization grant.
use crate::AgentError;
use ag_storage::{
    Meta,
    meta::{MetaError, Tx},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Latest executor report for one registered agent. Capacity is a snapshot.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Presence {
    /// Operator-provided executor name, not a command or credential.
    pub name: String,
    /// Operator identity responsible for the report.
    pub reported_by: String,
    /// Server receive time.
    pub reported_at_ms: i64,
    /// Server-controlled expiry.
    pub expires_at_ms: i64,
    /// Configured concurrent capacity.
    pub slots: u32,
    /// Available slots at report time.
    pub available: u32,
}

/// Reads the last report, including an expired report for diagnosis.
pub fn read(tx: &Tx<'_>, agent: &str) -> Result<Option<Presence>, MetaError> {
    tx.meta_get(&format!("executor:{agent}"))?
        .map(|value| serde_json::from_str(&value).map_err(MetaError::from))
        .transpose()
}

/// Records a bounded report for an enabled agent. Callers require agent.operate.
pub fn report(
    meta: &Meta,
    agent: &str,
    name: &str,
    slots: u32,
    available: u32,
    ttl_secs: u32,
    actor: &str,
) -> Result<Presence, AgentError> {
    validate_report(name, slots, available, ttl_secs)?;
    meta.write(|tx| {
        ensure_enabled(tx, agent)?;
        let old = read(tx, agent)?;
        let now = tx.now().as_millis();
        let presence = Presence {
            name: name.trim().into(),
            reported_by: actor.into(),
            reported_at_ms: now,
            expires_at_ms: now.saturating_add(i64::from(ttl_secs) * 1000),
            slots,
            available,
        };
        tx.meta_set(&format!("executor:{agent}"), &serde_json::to_string(&presence)?)?;
        audit_connection(tx, agent, old.as_ref(), &presence)?;
        Ok(presence)
    })
    .map_err(report_error)
}

fn validate_report(name: &str, slots: u32, available: u32, ttl_secs: u32) -> Result<(), AgentError> {
    if name.trim().is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(AgentError::Invalid("executor name must be 1–128 bytes with no control characters".into()));
    }
    if slots == 0 || slots > 1024 || available > slots || !(30..=900).contains(&ttl_secs) {
        return Err(AgentError::Invalid(
            "executor slots must be 1–1024, available <= slots, and ttl_secs 30–900".into(),
        ));
    }
    Ok(())
}

fn ensure_enabled(tx: &Tx<'_>, agent: &str) -> Result<(), MetaError> {
    let registered = tx.agent(agent)?.ok_or_else(|| MetaError::NotFound(format!("agent '{agent}'")))?;
    if registered.disabled {
        return Err(MetaError::Invalid("agent is disabled".into()));
    }
    Ok(())
}

fn audit_connection(tx: &Tx<'_>, agent: &str, old: Option<&Presence>, presence: &Presence) -> Result<(), MetaError> {
    if old.is_none_or(|old| {
        old.name != presence.name
            || old.reported_by != presence.reported_by
            || old.expires_at_ms <= presence.reported_at_ms
    }) {
        tx.audit_append(
            &presence.reported_by,
            "agent.executor.report",
            agent,
            &json!({"executor":presence.name,"slots":presence.slots}),
        )?;
    }
    Ok(())
}

fn report_error(error: MetaError) -> AgentError {
    match error {
        MetaError::NotFound(message) => AgentError::NotFound(message),
        MetaError::Invalid(message) => AgentError::Conflict(message),
        other => AgentError::Meta(other),
    }
}
