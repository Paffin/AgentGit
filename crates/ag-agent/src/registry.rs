//! Agent identities, key storage and runs.

use std::path::{Path, PathBuf};

use ag_crypto::{KeyPair, PublicKey, random_id};
use ag_storage::Meta;
use ag_storage::meta::{AgentRow, MetaError, RunRow, Tx};
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
        ag_crypto::private_file::create_dir_all(&dir).map_err(ag_crypto::CryptoError::from)?;
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
    if ok {
        Ok(())
    } else {
        Err(AgentError::Invalid(format!("agent name '{name}' must match [a-z0-9][a-z0-9_-]{{0,63}}")))
    }
}

/// Registers an agent: creates its key pair and stores the public key.
pub fn register_agent(
    meta: &Meta,
    keys: &KeyStore,
    name: &str,
    role: &str,
    actor: &str,
) -> Result<AgentRow, AgentError> {
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
        tx.audit_append(
            actor,
            "agent.register",
            identity.as_str(),
            &json!({"role": role, "key_id": kp.public().key_id()}),
        )?;
        Ok(row)
    })?;
    Ok(row)
}

/// Registers an agent whose key pair lives elsewhere (a remote agent keeps
/// its private key; only the public key is submitted).
pub fn register_agent_with_key(
    meta: &Meta,
    name: &str,
    public_key: &PublicKey,
    role: &str,
    actor: &str,
) -> Result<AgentRow, AgentError> {
    validate_name(name)?;
    if !["agent", "reviewer"].contains(&role) {
        return Err(AgentError::Invalid(format!("unknown role '{role}' (expected agent or reviewer)")));
    }
    let identity = Identity::of(IdentityKind::Agent, name).map_err(|e| AgentError::Invalid(e.to_string()))?;
    let row = meta.write(|tx| {
        if tx.agent(name)?.is_some() {
            return Err(MetaError::AlreadyExists(format!("agent '{name}'")));
        }
        let row = tx.agent_register(name, identity.as_str(), &public_key.to_hex(), role)?;
        tx.audit_append(
            actor,
            "agent.register",
            identity.as_str(),
            &json!({"role": role, "key_id": public_key.key_id()}),
        )?;
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

/// Changes admission of new runs without terminating existing sessions.
pub fn set_disabled(
    meta: &Meta,
    name: &str,
    disabled: bool,
    expected: Option<bool>,
    actor: &str,
) -> Result<AgentRow, AgentError> {
    meta.write(|tx| set_disabled_in(tx, name, disabled, expected, actor)).map_err(|e| match e {
        MetaError::Invalid(message) => AgentError::Conflict(message),
        other => AgentError::Meta(other),
    })
}

/// Transactional state change shared by API idempotency and the native CLI.
pub fn set_disabled_in(
    tx: &Tx<'_>,
    name: &str,
    disabled: bool,
    expected: Option<bool>,
    actor: &str,
) -> Result<AgentRow, MetaError> {
    let mut row = tx.agent(name)?.ok_or_else(|| MetaError::NotFound(format!("agent '{name}'")))?;
    if expected.is_some_and(|value| value != row.disabled) {
        return Err(MetaError::Invalid("agent state changed; refresh before trying again".into()));
    }
    if row.disabled != disabled {
        tx.agent_set_disabled(name, disabled)?;
        tx.audit_append(actor, if disabled { "agent.disable" } else { "agent.enable" }, name, &json!({}))?;
        tx.outbox_push("agent.state", &json!({"agent": name, "disabled": disabled}))?;
        row.disabled = disabled;
    }
    Ok(row)
}

/// Starts a run of an agent, optionally bound to a task (which must be ready).
pub fn start_run(meta: &Meta, agent: &str, task_id: Option<&str>) -> Result<Run, AgentError> {
    start_run_with(meta, agent, task_id, |_, _| Ok(())).map(|(run, ())| run)
}

/// Like [`start_run`], and runs `also` in the very transaction that starts the run: whatever it writes (an
/// idempotency record, say) is committed with the run or not at all. `also` may run again if the transaction is
/// retried, and its error rolls the run back.
pub fn start_run_with<R>(
    meta: &Meta,
    agent: &str,
    task_id: Option<&str>,
    also: impl Fn(&Tx<'_>, &Run) -> Result<R, MetaError>,
) -> Result<(Run, R), AgentError> {
    let run_id = random_id("run")?;
    meta.write(|tx| {
        let agent_row = admitted_agent(tx, agent)?;
        let identity = Identity::new(format!("{}#{}", agent_row.identity, run_id))
            .map_err(|e| MetaError::Invalid(e.to_string()))?;
        let run = Run { run_id: run_id.clone(), agent: agent.to_string(), identity };
        claim_run_task(tx, task_id, &agent_row.identity)?;
        tx.run_start(&run.run_id, agent, task_id)?;
        tx.audit_append(run.identity.as_str(), "run.start", &run.run_id, &json!({"task": task_id}))?;
        let extra = also(tx, &run)?;
        Ok((run, extra))
    })
    // A task that cannot be claimed (owned by somebody else, blocked, finished, or already being worked on) is a
    // conflict with the state of things, not a malformed request.
    .map_err(run_state_error)
}

fn run_state_error(error: MetaError) -> AgentError {
    match error {
        MetaError::Invalid(message) => AgentError::Conflict(message),
        MetaError::NotFound(message) => AgentError::NotFound(message),
        other => AgentError::Meta(other),
    }
}

fn admitted_agent(tx: &Tx<'_>, agent: &str) -> Result<AgentRow, MetaError> {
    let row = tx.agent(agent)?.ok_or_else(|| MetaError::NotFound(format!("agent '{agent}'")))?;
    if row.disabled {
        return Err(MetaError::Invalid(format!("agent '{agent}' is disabled")));
    }
    Ok(row)
}

fn claim_run_task(tx: &Tx<'_>, task_id: Option<&str>, identity: &str) -> Result<(), MetaError> {
    let Some(task) = task_id else { return Ok(()) };
    if crate::scheduling::in_tx(tx)?.paused {
        return Err(MetaError::Invalid("new task runs are paused by a repository operator".into()));
    }
    crate::tasks::claim_in_tx(tx, task, identity)?;
    // A task is worked on by one run at a time, even when both runs have the same agent.
    if let Some(active) = tx.runs_running_for_task(task)?.first() {
        return Err(MetaError::Invalid(format!("task '{task}' is already being worked on by run '{}'", active.run_id)));
    }
    Ok(())
}

/// Records a heartbeat.
pub fn heartbeat(meta: &Meta, run_id: &str) -> Result<(), AgentError> {
    meta.write(|tx| tx.run_heartbeat(run_id))?;
    Ok(())
}

/// The identity a run acts as: `agent:<name>#<run_id>`.
pub fn run_identity(agent: &str, run_id: &str) -> String {
    format!("agent:{agent}#{run_id}")
}

/// Whether `actor` is the agent of `run` (the agent itself or one of its runs), as opposed to somebody acting
/// for the agent: an operator, an orchestrator, a person.
pub fn is_agent_of(actor: &str, run: &RunRow) -> bool {
    actor.split('#').next() == Some(format!("agent:{}", run.agent).as_str())
}

/// Finishes a run (`completed`, `failed`) and releases the leases it holds. The audit entry names the run's own
/// identity: this is the agent finishing its own run. Use [`finish_run_as`] when somebody else does.
pub fn finish_run(meta: &Meta, run_id: &str, status: &str, summary: Option<&str>) -> Result<(), AgentError> {
    finish_run_as(meta, run_id, status, summary, None)
}

/// Like [`finish_run`], with the audit entry written under `actor` (`None`: the run's own identity). When the
/// actor is not the run's agent - an operator finishing a run on its behalf - the entry says so (`operator`), so
/// the log never shows an agent doing what an operator did.
pub fn finish_run_as(
    meta: &Meta,
    run_id: &str,
    status: &str,
    summary: Option<&str>,
    actor: Option<&str>,
) -> Result<(), AgentError> {
    if !["completed", "failed"].contains(&status) {
        return Err(AgentError::Invalid("status must be 'completed' or 'failed'".into()));
    }
    meta.write(|tx| {
        let run = tx.run(run_id)?.ok_or_else(|| ag_storage::meta::MetaError::NotFound(format!("run '{run_id}'")))?;
        tx.run_finish(run_id, status, summary)?;
        release_run_leases(tx, &run)?;
        let own = run_identity(&run.agent, run_id);
        let actor = actor.unwrap_or(&own);
        let mut details = json!({"status": status, "agent": run.agent});
        if !is_agent_of(actor, &run) {
            details["operator"] = json!(true);
        }
        tx.audit_append(actor, "run.finish", run_id, &details)?;
        Ok(())
    })?;
    Ok(())
}

fn release_run_leases(tx: &ag_storage::meta::Tx<'_>, run: &RunRow) -> Result<(), ag_storage::meta::MetaError> {
    let holder = run_identity(&run.agent, &run.run_id);
    for l in tx.leases_active()? {
        if l.holder == holder {
            tx.lease_release(&l.id)?;
        }
    }
    Ok(())
}

/// What a reap ended (or, for a dry run, would end).
#[derive(Clone, Debug, Default)]
pub struct Reaped {
    /// The stale runs: abandoned by the reap (still `running` in a dry run).
    pub runs: Vec<RunRow>,
    /// Ids of the leases released: those of the stale runs, and any active lease whose holder is a run that has
    /// already ended.
    pub released_leases: Vec<String>,
}

/// A lease a reap releases: its id, its holder and why.
type LeaseToRelease = (String, String, &'static str);

/// The stale runs and the leases to release, as of the transaction's clock.
fn plan_reap(tx: &Tx<'_>, stale_after_ms: i64) -> Result<(Vec<RunRow>, Vec<LeaseToRelease>), MetaError> {
    let cutoff = ag_types::Timestamp::from_millis(tx.now().as_millis().saturating_sub(stale_after_ms));
    let stale = tx.runs_stale(cutoff)?;
    let mut leases = Vec::new();
    for l in tx.leases_active()? {
        // Only a lease held by a run (`agent:<name>#<run>`) can be abandoned; one held by the agent itself has
        // no run to end and expires on its own.
        let Some((agent, run_id)) = l.holder.strip_prefix("agent:").and_then(|h| h.split_once('#')) else {
            continue;
        };
        if stale.iter().any(|r| r.run_id == run_id && r.agent == agent) {
            leases.push((l.id, l.holder, "its run was abandoned"));
        } else if tx.run(run_id)?.is_some_and(|r| r.agent == agent && r.status != "running") {
            leases.push((l.id, l.holder, "its run has ended"));
        }
    }
    Ok((stale, leases))
}

/// Ends the runs that have not sent a heartbeat for `stale_after_ms` (`abandoned`) and releases the leases they
/// hold - and the leases of runs that ended without releasing theirs. Every run and lease is recorded in the audit
/// log under `actor`. With `dry_run`, nothing is changed and the result says what would be.
pub fn reap_runs(meta: &Meta, stale_after_ms: i64, actor: &str, dry_run: bool) -> Result<Reaped, AgentError> {
    if stale_after_ms < 0 {
        return Err(AgentError::Invalid("stale_after_ms must not be negative".into()));
    }
    if dry_run {
        return Ok(meta.read(|tx| {
            let (runs, leases) = plan_reap(tx, stale_after_ms)?;
            Ok(Reaped { runs, released_leases: leases.into_iter().map(|(id, _, _)| id).collect() })
        })?);
    }
    Ok(meta.write(|tx| {
        let (stale, leases) = plan_reap(tx, stale_after_ms)?;
        let mut out = Reaped::default();
        for run in stale {
            tx.run_finish(&run.run_id, "abandoned", Some("no heartbeat"))?;
            tx.audit_append(
                actor,
                "run.abandon",
                &run.run_id,
                &json!({
                    "agent": run.agent,
                    "last_heartbeat_ms": run.last_heartbeat.as_millis(),
                    "stale_after_ms": stale_after_ms,
                }),
            )?;
            out.runs.push(tx.run(&run.run_id)?.ok_or_else(|| MetaError::Corrupt("run vanished".into()))?);
        }
        for (id, holder, why) in leases {
            tx.lease_release(&id)?;
            tx.audit_append(actor, "lease.reap", &id, &json!({"holder": holder, "reason": why}))?;
            out.released_leases.push(id);
        }
        Ok(out)
    })?)
}

/// Marks runs without a heartbeat since `timeout_ms` as abandoned and releases their leases. Returns the
/// abandoned run ids. (The command-line form of [`reap_runs`].)
pub fn reap_stale_runs(meta: &Meta, timeout_ms: i64, actor: &str) -> Result<Vec<String>, AgentError> {
    Ok(reap_runs(meta, timeout_ms, actor, false)?.runs.into_iter().map(|r| r.run_id).collect())
}
