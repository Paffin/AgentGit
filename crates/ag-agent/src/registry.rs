//! Agent identities, key storage and runs.

use std::path::{Path, PathBuf};

use ag_crypto::{KeyPair, random_id};
use ag_storage::Meta;
use ag_storage::meta::{AgentRow, RunRow};
use ag_types::{Identity, IdentityKind};
use serde_json::json;

use crate::AgentError;

/// Directory of agent private keys, one `<name>.json` file each (mode 0600).
///
/// Keep this directory out of any environment that runs untrusted code: a
/// process that can read the key can forge provenance as that agent.
#[derive(Clone, Debug)]
pub struct KeyStore {
    dir: PathBuf,
}

impl KeyStore {
    /// Opens (creating) a key directory with owner-only permissions.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, AgentError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(ag_crypto::CryptoError::from)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(ag_crypto::CryptoError::from)?;
        }
        Ok(Self { dir })
    }

    /// Path of a named key.
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.json"))
    }

    /// Generates and saves a new key; fails if one exists.
    pub fn create(&self, name: &str) -> Result<KeyPair, AgentError> {
        let kp = KeyPair::generate()?;
        kp.save(&self.path(name))?;
        Ok(kp)
    }

    /// Loads a key.
    pub fn load(&self, name: &str) -> Result<KeyPair, AgentError> {
        Ok(KeyPair::load(&self.path(name))?)
    }

    /// The key directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// Validates an agent (or key) name: `[a-z0-9][a-z0-9_-]{0,63}`.
pub fn validate_name(name: &str) -> Result<(), AgentError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if ok { Ok(()) } else { Err(AgentError::Invalid(format!("agent name '{name}' must match [a-z0-9][a-z0-9_-]{{0,63}}"))) }
}

/// Registers an agent: creates its key pair and stores the public key.
pub fn register_agent(meta: &Meta, keys: &KeyStore, name: &str, role: &str, actor: &str) -> Result<AgentRow, AgentError> {
    validate_name(name)?;
    if !["agent", "reviewer"].contains(&role) {
        return Err(AgentError::Invalid(format!("unknown role '{role}' (expected agent or reviewer)")));
    }
    let identity = Identity::of(IdentityKind::Agent, name).map_err(|e| AgentError::Invalid(e.to_string()))?;
    // Fail before creating a key if the name is taken.
    if meta.read(|tx| tx.agent(name))?.is_some() {
        return Err(AgentError::Conflict(format!("agent '{name}' already exists")));
    }
    let kp = keys.create(name)?;
    let row = meta.write(|tx| {
        let row = tx.agent_register(name, identity.as_str(), &kp.public().to_hex(), role)?;
        tx.audit_append(actor, "agent.register", identity.as_str(), &json!({"role": role, "key_id": kp.public().key_id()}))?;
        Ok(row)
    })?;
    Ok(row)
}

/// A started run: the identity under which the agent acts for this execution.
#[derive(Clone, Debug)]
pub struct Run {
    /// Run id.
    pub run_id: String,
    /// Agent name.
    pub agent: String,
    /// Identity including the run suffix, e.g. `agent:bot#run_ab12…`.
    pub identity: Identity,
}

/// Starts a run of an agent, optionally bound to a task (which must be ready).
pub fn start_run(meta: &Meta, agent: &str, task_id: Option<&str>) -> Result<Run, AgentError> {
    let run_id = random_id("run")?;
    let agent_row = meta.read(|tx| tx.agent(agent))?.ok_or_else(|| AgentError::NotFound(format!("agent '{agent}'")))?;
    if agent_row.disabled {
        return Err(AgentError::Conflict(format!("agent '{agent}' is disabled")));
    }
    let identity = Identity::new(format!("{}#{}", agent_row.identity, run_id)).map_err(|e| AgentError::Invalid(e.to_string()))?;
    meta.write(|tx| {
        if let Some(t) = task_id {
            crate::tasks::claim_in_tx(tx, t, &agent_row.identity)?;
        }
        tx.run_start(&run_id, agent, task_id)?;
        tx.audit_append(identity.as_str(), "run.start", &run_id, &json!({"task": task_id}))?;
        Ok(())
    })?;
    Ok(Run { run_id, agent: agent.to_string(), identity })
}

/// Records a heartbeat.
pub fn heartbeat(meta: &Meta, run_id: &str) -> Result<(), AgentError> {
    meta.write(|tx| tx.run_heartbeat(run_id))?;
    Ok(())
}

/// Finishes a run (`completed`, `failed`) and releases the leases it holds.
pub fn finish_run(meta: &Meta, run_id: &str, status: &str, summary: Option<&str>) -> Result<(), AgentError> {
    if !["completed", "failed"].contains(&status) {
        return Err(AgentError::Invalid("status must be 'completed' or 'failed'".into()));
    }
    meta.write(|tx| {
        let run = tx.run(run_id)?.ok_or_else(|| ag_storage::meta::MetaError::NotFound(format!("run '{run_id}'")))?;
        tx.run_finish(run_id, status, summary)?;
        release_run_leases(tx, &run)?;
        tx.audit_append(&format!("agent:{}#{}", run.agent, run_id), "run.finish", run_id, &json!({"status": status}))?;
        Ok(())
    })?;
    Ok(())
}

fn release_run_leases(tx: &ag_storage::meta::Tx<'_>, run: &RunRow) -> Result<(), ag_storage::meta::MetaError> {
    let holder = format!("agent:{}#{}", run.agent, run.run_id);
    for l in tx.leases_active()? {
        if l.holder == holder {
            tx.lease_release(&l.id)?;
        }
    }
    Ok(())
}

/// Marks runs without a heartbeat since `timeout_ms` as abandoned and releases
/// their leases. Returns the abandoned run ids.
pub fn reap_stale_runs(meta: &Meta, timeout_ms: i64, actor: &str) -> Result<Vec<String>, AgentError> {
    Ok(meta.write(|tx| {
        let cutoff = ag_types::Timestamp::from_millis(tx.now().as_millis() - timeout_ms);
        let mut reaped = Vec::new();
        for run in tx.runs_stale(cutoff)? {
            tx.run_finish(&run.run_id, "abandoned", Some("no heartbeat"))?;
            release_run_leases(tx, &run)?;
            tx.audit_append(actor, "run.abandon", &run.run_id, &json!({"agent": run.agent}))?;
            reaped.push(run.run_id);
        }
        Ok(reaped)
    })?)
}
