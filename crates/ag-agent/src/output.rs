//! Opt-in bounded output snapshots, separate from the authoritative run outcome.
use crate::AgentError;
use ag_storage::{
    Meta,
    meta::{MetaError, Tx},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Maximum UTF-8 bytes retained for each stream.
pub const MAX_STREAM_BYTES: usize = 32 * 1024;

/// The last saved executor snapshot. Byte counts describe original streams.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Output {
    /// Monotonic producer sequence.
    pub sequence: u64,
    /// Captured stdout tail, already redacted by the producer.
    pub stdout: String,
    /// Captured stderr tail, already redacted by the producer.
    pub stderr: String,
    /// Original stdout size.
    pub stdout_bytes: u64,
    /// Original stderr size.
    pub stderr_bytes: u64,
    /// Server receive time.
    pub captured_at_ms: i64,
}

/// Reads a saved snapshot. The caller separately verifies run visibility.
pub fn read(tx: &Tx<'_>, run: &str) -> Result<Option<Output>, MetaError> {
    tx.meta_get(&format!("run_output:{run}"))?
        .map(|value| serde_json::from_str(&value).map_err(MetaError::from))
        .transpose()
}

/// Stores an operator-provided snapshot while a run is active; exact retries are idempotent.
pub fn save(meta: &Meta, run: &str, snapshot: &Output, actor: &str) -> Result<Output, AgentError> {
    if snapshot.sequence == 0 || snapshot.stdout.len() > MAX_STREAM_BYTES || snapshot.stderr.len() > MAX_STREAM_BYTES {
        return Err(AgentError::Invalid(
            "output needs a positive sequence and at most 32768 UTF-8 bytes per stream".into(),
        ));
    }
    meta.write(|tx| save_in_tx(tx, run, snapshot, actor)).map_err(map_error)
}

fn save_in_tx(tx: &Tx<'_>, run: &str, snapshot: &Output, actor: &str) -> Result<Output, MetaError> {
    let current = tx.run(run)?.ok_or_else(|| MetaError::NotFound("run not found".into()))?;
    if let Some(previous) = accepted_retry(tx, run, snapshot)? {
        return Ok(previous);
    }
    if current.status != "running" {
        return Err(MetaError::Invalid("output can only be saved while a run is active".into()));
    }
    let saved = Output { captured_at_ms: tx.now().as_millis(), ..snapshot.clone() };
    tx.meta_set(&format!("run_output:{run}"), &serde_json::to_string(&saved)?)?;
    tx.audit_append(
        actor,
        "run.output",
        run,
        &json!({"sequence":saved.sequence,"stdout_bytes":saved.stdout_bytes,"stderr_bytes":saved.stderr_bytes}),
    )?;
    Ok(saved)
}

fn same_snapshot(a: &Output, b: &Output) -> bool {
    a.sequence == b.sequence
        && a.stdout == b.stdout
        && a.stderr == b.stderr
        && a.stdout_bytes == b.stdout_bytes
        && a.stderr_bytes == b.stderr_bytes
}

/// Removes captured text, retaining an audit entry without the output itself.
pub fn delete(meta: &Meta, run: &str, actor: &str) -> Result<(), AgentError> {
    meta.write(|tx| {
        if tx.run(run)?.is_none() {
            return Err(MetaError::NotFound("run not found".into()));
        }
        let marker = format!("run_output_deleted:{run}");
        if tx.meta_get(&marker)?.is_none() {
            let had_output = read(tx, run)?.is_some();
            tx.meta_set(&marker, "1")?;
            tx.meta_delete(&format!("run_output:{run}"))?;
            tx.audit_append(actor, "run.output.delete", run, &json!({"had_output":had_output}))?;
        }
        Ok(())
    })
    .map_err(map_error)
}

fn map_error(error: MetaError) -> AgentError {
    match error {
        MetaError::NotFound(message) => AgentError::NotFound(message),
        MetaError::Invalid(message) => AgentError::Conflict(message),
        other => AgentError::Meta(other),
    }
}

fn accepted_retry(tx: &Tx<'_>, run: &str, snapshot: &Output) -> Result<Option<Output>, MetaError> {
    if tx.meta_get(&format!("run_output_deleted:{run}"))?.is_some() {
        return Err(MetaError::Invalid("saved output was deleted; further uploads for this run are disabled".into()));
    }

    if let Some(old) = read(tx, run)? {
        if same_snapshot(&old, snapshot) {
            return Ok(Some(old));
        }
        if old.sequence >= snapshot.sequence {
            return Err(MetaError::Invalid("a newer output snapshot is already saved".into()));
        }
    }
    Ok(None)
}
