//! Agents: identities, runs, tasks, leases, provenance, conflict prediction.

use ag_agent::leases::{acquire_with, extend, release};
use ag_agent::predict::predict;
use ag_agent::provenance::{ProvenanceRecord, SCHEMA, attach_signed, verify};
use ag_agent::registry::{finish_run_as, heartbeat, reap_runs, register_agent_with_key, start_run_with};
use ag_agent::tasks::{
    add_dependency, create_task_with, finish_task, finish_task_as_operator, unfinished_dependencies,
};
use ag_crypto::{PublicKey, Signature};
use ag_protocol::v1;
use ag_storage::meta::{MetaError, TaskRow, Tx};
use ag_types::{ChangesetId, Identity, IdentityKind, Timestamp};

use super::{IDEM_TIME_PREFIX, Idem, convert, keyed, padded_ms, page_size, parse_id, require};
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::{ApiError, ApiResult};

/// Records one sweep deletes per repository (a bound on the length of its write transaction).
const SWEEP_BATCH: usize = 1000;
/// Marks a repository whose idempotency records written before they carried a time have been dated.
const IDEM_BACKFILL_DONE: &str = "backfill:idem-times";

/// An agent principal may only act for itself; humans and services (an
/// orchestrator, an operator) may act for any agent.
fn check_agent_scope(actor: &Identity, agent_name: &str) -> ApiResult<()> {
    let p = actor.principal();
    if p.kind() == IdentityKind::Agent && p.name() != agent_name {
        return Err(ApiError::denied(format!("{p} may not act for agent '{agent_name}'")));
    }
    Ok(())
}

/// Acting on an agent's run is the agent's own business - or an operator's. An agent principal is held to its own
/// runs by [`check_agent_scope`]; anyone else (a human, a service: a plain `writer`) acts for an agent only with
/// `agent.operate`, which the `delegator` and `admin` roles carry.
fn check_operator_for(actor: &Identity, agent_name: &str, operator: bool, what: &str) -> ApiResult<()> {
    let p = actor.principal();
    if operator || (p.kind() == IdentityKind::Agent && p.name() == agent_name) {
        return Ok(());
    }
    Err(ApiError::denied(format!(
        "{p} is not agent '{agent_name}': {what} for another agent requires the capability agent.operate"
    )))
}

/// The run an actor acts as (`agent:bot#run_...`), if it acts as one.
fn acting_run(actor: &Identity) -> Option<&str> {
    actor.as_str().split_once('#').map(|(_, run)| run)
}

/// A run keeps its own leases: another run of the same agent may neither release nor extend them. (An actor that
/// is not a run - the agent itself, an operator - is held to the engine's rule, which compares principals.)
fn check_lease_run(repo: &ag_integrate::Repository, lease_id: &str, actor: &Identity) -> ApiResult<()> {
    if acting_run(actor).is_none() {
        return Ok(());
    }
    match repo.meta().read(|tx| tx.lease(lease_id))? {
        Some(lease) if !super::owns(&lease.holder, actor) => {
            Err(ApiError::denied(format!("lease '{lease_id}' is held by {}, not by {actor}", lease.holder)))
        }
        _ => Ok(()),
    }
}

/// A task with its dependencies, read in the caller's transaction.
fn task_view_in(tx: &Tx<'_>, t: &TaskRow) -> Result<v1::Task, MetaError> {
    let (deps, ready) = (tx.task_deps(&t.id)?, unfinished_dependencies(tx, &t.id)?.is_empty());
    Ok(convert::task(t, deps, ready))
}

fn task_view(repo: &ag_integrate::Repository, t: &TaskRow) -> ApiResult<v1::Task> {
    Ok(repo.meta().read(|tx| task_view_in(tx, t))?)
}

fn record_from_wire(r: &v1::ProvenanceRecord) -> ApiResult<ProvenanceRecord> {
    let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
    Ok(ProvenanceRecord {
        schema: SCHEMA.into(),
        changeset: parse_id::<ChangesetId>(&r.changeset, "changeset id")?,
        agent: Identity::new(r.agent.clone())?,
        run_id: opt(&r.run_id),
        task_id: opt(&r.task_id),
        model: opt(&r.model),
        prompt_digest: opt(&r.prompt_digest),
        parameters: r.parameters.clone(),
        checks_run: r.checks_run.clone(),
        inputs: r.inputs.clone(),
        rationale: opt(&r.rationale),
        created_at: Timestamp::from_millis(r.created_at_ms),
    })
}

impl App {
    /// The lifetime of idempotency records and the time to judge them by.
    pub(super) fn idem(&self) -> Idem {
        let ttl_ms = self.cfg.repos.idempotency_ttl_secs.saturating_mul(1000);
        Idem { ttl_ms: i64::try_from(ttl_ms).unwrap_or(i64::MAX), now_ms: self.clock.now().as_millis() }
    }

    /// Deletes the idempotency records older than `repos.idempotency_ttl_secs` from every repository (at most
    /// `SWEEP_BATCH` per repository and pass; the next pass takes the rest). Returns how many were removed. A
    /// repository that cannot be read is logged and skipped.
    pub async fn sweep_idempotency_records(&self) -> usize {
        let idem = self.idem();
        if idem.ttl_ms <= 0 {
            return 0;
        }
        let control = self.control.clone();
        let names = match tokio::task::spawn_blocking(move || control.all_repos()).await {
            Ok(Ok(rows)) => rows.into_iter().map(|r| r.name).collect::<Vec<_>>(),
            Ok(Err(e)) => {
                tracing::warn!(error = %e.0.message, "idempotency sweep: cannot list the repositories");
                return 0;
            }
            Err(e) => {
                tracing::error!(error = %e, "idempotency sweep: the listing task failed");
                return 0;
            }
        };
        let mut removed = 0;
        for name in names {
            let app = self.0.clone();
            let repo = name.clone();
            let swept = tokio::task::spawn_blocking(move || -> ApiResult<usize> {
                let handle = app.repos.handle(&repo)?;
                let bound = padded_ms(idem.now_ms.saturating_sub(idem.ttl_ms));
                Ok(handle.meta().write(|tx| {
                    // Records written before they carried a time can never be judged; once per repository they get
                    // the time of this first sweep, so they live one more lifetime and are then removed.
                    if tx.meta_get(IDEM_BACKFILL_DONE)?.is_none() {
                        let n = tx.meta_fill_companions("idem:", IDEM_TIME_PREFIX, &padded_ms(idem.now_ms))?;
                        tx.meta_set(IDEM_BACKFILL_DONE, "1")?;
                        if n > 0 {
                            tracing::info!(repo = %repo, records = n, "idempotency sweep: dated records that had no time");
                        }
                    }
                    let stale = tx.meta_keys_with_value_below(IDEM_TIME_PREFIX, &bound, SWEEP_BATCH)?;
                    for time_key in &stale {
                        tx.meta_delete(time_key)?;
                        tx.meta_delete(&format!("idem:{}", &time_key[IDEM_TIME_PREFIX.len()..]))?;
                    }
                    Ok(stale.len())
                })?)
            })
            .await;
            match swept {
                Ok(Ok(n)) => removed += n,
                Ok(Err(e)) => tracing::warn!(repo = %name, error = %e.0.message, "idempotency sweep: skipped"),
                Err(e) => tracing::error!(repo = %name, error = %e, "idempotency sweep: the task failed"),
            }
        }
        if removed > 0 {
            tracing::info!(records = removed, "idempotency sweep: expired records removed");
        }
        removed
    }

    /// Registers an agent and its public key.
    pub async fn register_agent(&self, call: &Call, req: v1::RegisterAgentRequest) -> ApiResult<v1::Agent> {
        let name = require(&req.name, "name")?.to_string();
        let key = PublicKey::from_hex(&req.public_key).map_err(|e| ApiError::invalid(format!("public_key: {e}")))?;
        let role = if req.role.is_empty() { "agent".to_string() } else { req.role.clone() };
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::AgentRegister, None, move |repo, actor| {
            check_agent_scope(actor, &name)?;
            let row =
                register_agent_with_key(repo.meta(), &name, &key, &role, actor.as_str()).map_err(|e| match e {
                    ag_agent::AgentError::Meta(ag_storage::meta::MetaError::AlreadyExists(m)) => {
                        ApiError::new(ag_protocol::ErrorCode::AlreadyExists, m)
                    }
                    other => other.into(),
                })?;
            Ok(convert::agent(&row))
        })
        .await
    }

    /// All registered agents.
    pub async fn list_agents(&self, call: &Call, req: v1::ListAgentsRequest) -> ApiResult<v1::ListAgentsResponse> {
        self.with_repo(call, require(&req.repo, "repo")?, Cap::AgentRead, None, |repo, _| {
            Ok(v1::ListAgentsResponse {
                agents: repo.meta().read(|tx| tx.agents())?.iter().map(convert::agent).collect(),
            })
        })
        .await
    }

    /// Starts a run; the response carries the identity to act as.
    pub async fn start_run(&self, call: &Call, req: v1::StartRunRequest) -> ApiResult<v1::Run> {
        let agent = require(&req.agent, "agent")?.to_string();
        let task = (!req.task_id.is_empty()).then(|| req.task_id.clone());
        let key = req.idempotency_key.clone();
        let idem = self.idem();
        let repo_name = require(&req.repo, "repo")?.to_string();
        let operator = self.authorize(call, &repo_name, Cap::AgentOperate, None).await.is_ok();
        self.with_repo_mut(call, &repo_name, Cap::AgentRun, None, move |repo, actor| {
            check_agent_scope(actor, &agent)?;
            check_operator_for(actor, &agent, operator, "starting a run")?;
            // A run does not start runs: one acting as a run - a run-bound credential, or `ag-run` - would be
            // making a second identity for its agent. Runs are started with the agent's own credential, or by
            // an operator.
            if let Some(run) = acting_run(actor) {
                return Err(ApiError::denied(format!("{actor} acts as run '{run}' and may not start another run")));
            }
            let request = format!("start_run\0{actor}\0{agent}\0{}", task.as_deref().unwrap_or(""));
            keyed(repo, idem, "run.start", &key, &request, |claim| {
                let (_, response) = start_run_with(repo.meta(), &agent, task.as_deref(), |tx, run| {
                    let row = tx.run(&run.run_id)?.ok_or_else(|| MetaError::Corrupt("run vanished".into()))?;
                    let response = convert::run(&row);
                    claim.record(tx, &response)?;
                    Ok(response)
                })?;
                Ok(response)
            })
        })
        .await
    }

    fn check_run_owner(
        repo: &ag_integrate::Repository,
        run_id: &str,
        actor: &Identity,
        operator: bool,
        what: &str,
    ) -> ApiResult<ag_storage::meta::RunRow> {
        let run = repo
            .meta()
            .read(|tx| tx.run(run_id))?
            .ok_or_else(|| ApiError::not_found(format!("run '{run_id}' not found")))?;
        check_agent_scope(actor, &run.agent)?;
        check_operator_for(actor, &run.agent, operator, what)?;
        // Acting as one run is not acting as another: the heartbeat and the end of a run are its own.
        if let Some(acting) = acting_run(actor)
            && acting != run_id
        {
            return Err(ApiError::denied(format!("{actor} acts as run '{acting}'; it may not act on run '{run_id}'")));
        }
        Ok(run)
    }

    /// Records a heartbeat of a run.
    pub async fn heartbeat(&self, call: &Call, req: v1::HeartbeatRequest) -> ApiResult<v1::HeartbeatResponse> {
        let now = self.clock.now().as_millis();
        let repo_name = require(&req.repo, "repo")?.to_string();
        let operator = self.authorize(call, &repo_name, Cap::AgentOperate, None).await.is_ok();
        self.with_repo(call, &repo_name, Cap::AgentRun, None, move |repo, actor| {
            Self::check_run_owner(repo, &req.run_id, actor, operator, "a heartbeat")?;
            heartbeat(repo.meta(), &req.run_id)?;
            Ok(v1::HeartbeatResponse { server_time_ms: now })
        })
        .await
    }

    /// Finishes a run, releases its leases and revokes the credentials bound to it. The audit entry is the
    /// caller's own: the agent finishing its run, or an operator finishing it for the agent (flagged as such).
    pub async fn complete_run(&self, call: &Call, req: v1::CompleteRunRequest) -> ApiResult<v1::Run> {
        let repo_name = require(&req.repo, "repo")?.to_string();
        let run_id = req.run_id.clone();
        let operator = self.authorize(call, &repo_name, Cap::AgentOperate, None).await.is_ok();
        let out = self
            .with_repo_mut(call, &repo_name, Cap::AgentRun, None, move |repo, actor| {
                Self::check_run_owner(repo, &req.run_id, actor, operator, "finishing a run")?;
                let summary = (!req.summary.is_empty()).then_some(req.summary.as_str());
                finish_run_as(repo.meta(), &req.run_id, &req.status, summary, Some(actor.as_str()))?;
                let row =
                    repo.meta().read(|tx| tx.run(&req.run_id))?.ok_or_else(|| ApiError::internal("run vanished"))?;
                Ok(convert::run(&row))
            })
            .await?;
        // The run is over, and so is every credential bound to it (they stop working at once either way).
        self.revoke_run_credentials(&repo_name, &run_id).await;
        Ok(out)
    }

    /// The threshold of a reap: the request's, else `agents.run_stale_after_secs`; never below
    /// `agents.min_stale_after_secs`.
    fn reap_threshold_ms(&self, requested_ms: u64) -> ApiResult<i64> {
        let cfg = &self.cfg.agents;
        let ms = if requested_ms == 0 { cfg.run_stale_after_secs.saturating_mul(1000) } else { requested_ms };
        let min = cfg.min_stale_after_secs.saturating_mul(1000);
        if ms < min {
            return Err(ApiError::invalid(format!(
                "stale_after_ms must be at least {min} (agents.min_stale_after_secs); it was {ms}"
            )));
        }
        i64::try_from(ms).map_err(|_| ApiError::invalid("stale_after_ms is too large"))
    }

    /// Ends the runs of a repository that stopped heartbeating, releases the leases of ended runs and revokes
    /// the credentials bound to the reaped runs. An operator action, audited under the caller's identity.
    pub async fn reap_runs(&self, call: &Call, req: v1::ReapRunsRequest) -> ApiResult<v1::ReapRunsResponse> {
        let repo_name = require(&req.repo, "repo")?.to_string();
        let stale_after = self.reap_threshold_ms(req.stale_after_ms)?;
        let dry_run = req.dry_run;
        let reaped = self
            .with_repo_mut(call, &repo_name, Cap::AgentOperate, None, move |repo, actor| {
                Ok(reap_runs(repo.meta(), stale_after, actor.as_str(), dry_run)?)
            })
            .await?;
        self.finish_reap(&repo_name, reaped, dry_run).await
    }

    /// The part of a reap that comes after the repository's own transaction: the credentials of the reaped runs,
    /// the counter, the response.
    async fn finish_reap(
        &self,
        repo: &str,
        reaped: ag_agent::registry::Reaped,
        dry_run: bool,
    ) -> ApiResult<v1::ReapRunsResponse> {
        let mut revoked = 0u64;
        if !dry_run {
            for run in &reaped.runs {
                revoked += self.revoke_run_credentials(repo, &run.run_id).await;
            }
            self.metrics.runs_reaped.inc_by(reaped.runs.len() as u64);
        }
        Ok(v1::ReapRunsResponse {
            runs: reaped.runs.iter().map(convert::run).collect(),
            released_leases: reaped.released_leases,
            revoked_credentials: u32::try_from(revoked).unwrap_or(u32::MAX),
            dry_run,
        })
    }

    /// Reaps every registered repository on behalf of the server itself (`agents.reap_interval_secs`). A
    /// repository that cannot be read is logged and skipped; the next pass tries again.
    pub async fn reap_all_repos(&self) {
        // One replica at a time: the lease is renewed by its holder on every pass and expires after two intervals if
        // the holder dies. A control database that cannot be asked skips the pass (the next one tries again).
        let (control, holder) = (self.control.clone(), self.lease_holder.clone());
        let ttl = std::time::Duration::from_secs(self.cfg.agents.reap_interval_secs.saturating_mul(2).max(2));
        match tokio::task::spawn_blocking(move || control.try_lease(crate::app::RUN_REAPER_LEASE, &holder, ttl)).await {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => return,
            Ok(Err(e)) => {
                tracing::warn!(error = %e.0.message, "run reaper: cannot take the lease");
                return;
            }
            Err(e) => {
                tracing::error!(error = %e, "run reaper: the lease task failed");
                return;
            }
        }
        let Ok(stale_after) = i64::try_from(self.cfg.agents.run_stale_after_secs.saturating_mul(1000)) else {
            return;
        };
        let control = self.control.clone();
        let names = match tokio::task::spawn_blocking(move || control.all_repos()).await {
            Ok(Ok(rows)) => rows.into_iter().map(|r| r.name).collect::<Vec<_>>(),
            Ok(Err(e)) => {
                tracing::warn!(error = %e.0.message, "run reaper: cannot list the repositories");
                return;
            }
            Err(e) => {
                tracing::error!(error = %e, "run reaper: the listing task failed");
                return;
            }
        };
        for name in names {
            let app = self.0.clone();
            let repo = name.clone();
            let reaped = tokio::task::spawn_blocking(move || -> ApiResult<_> {
                let handle = app.repos.handle(&repo)?;
                Ok(reap_runs(handle.meta(), stale_after, crate::app::REAPER_IDENTITY, false)?)
            })
            .await;
            match reaped {
                Ok(Ok(r)) if !r.runs.is_empty() || !r.released_leases.is_empty() => {
                    tracing::info!(repo = %name, runs = r.runs.len(), leases = r.released_leases.len(), "run reaper: ended stale runs");
                    self.notify(&name);
                    let _ = self.finish_reap(&name, r, false).await;
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::warn!(repo = %name, error = %e.0.message, "run reaper: skipped"),
                Err(e) => tracing::error!(repo = %name, error = %e, "run reaper: the task failed"),
            }
        }
    }

    /// Lists runs (newest first).
    pub async fn list_runs(&self, call: &Call, req: v1::ListRunsRequest) -> ApiResult<v1::ListRunsResponse> {
        let limit = if req.limit == 0 { 100 } else { req.limit.min(1000) } as usize;
        self.with_repo(call, require(&req.repo, "repo")?, Cap::AgentRead, None, move |repo, _| {
            let agent = (!req.agent.is_empty()).then_some(req.agent.as_str());
            let runs = repo
                .meta()
                .read(|tx| if req.active_only { tx.runs_running(agent, limit) } else { tx.runs(agent, limit) })?;
            Ok(v1::ListRunsResponse { runs: runs.iter().map(convert::run).collect() })
        })
        .await
    }

    /// Creates a task (with dependencies).
    pub async fn create_task(&self, call: &Call, req: v1::CreateTaskRequest) -> ApiResult<v1::Task> {
        let idem = self.idem();
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::TaskWrite, None, move |repo, actor| {
            let request = format!(
                "create_task\0{actor}\0{}\0{}\0{}\0{}",
                req.title,
                req.description,
                req.scope.join(","),
                req.depends_on.join(",")
            );
            keyed(repo, idem, "task.create", &req.idempotency_key, &request, |claim| {
                // The task, its dependencies and the idempotency record are one transaction.
                let (_, response) = create_task_with(
                    repo.meta(),
                    &req.title,
                    &req.description,
                    &req.scope,
                    &req.depends_on,
                    actor.as_str(),
                    |tx, t| {
                        let response = task_view_in(tx, t)?;
                        claim.record(tx, &response)?;
                        Ok(response)
                    },
                )?;
                Ok(response)
            })
        })
        .await
    }

    /// One task.
    pub async fn get_task(&self, call: &Call, req: v1::GetTaskRequest) -> ApiResult<v1::Task> {
        self.with_repo(call, require(&req.repo, "repo")?, Cap::TaskRead, None, move |repo, _| {
            let t = repo
                .meta()
                .read(|tx| tx.task(&req.id))?
                .ok_or_else(|| ApiError::not_found(format!("task '{}' not found", req.id)))?;
            task_view(repo, &t)
        })
        .await
    }

    /// Lists tasks, optionally only those that are ready to start.
    pub async fn list_tasks(&self, call: &Call, req: v1::ListTasksRequest) -> ApiResult<v1::ListTasksResponse> {
        self.with_repo(call, require(&req.repo, "repo")?, Cap::TaskRead, None, move |repo, _| {
            let state = (!req.state.is_empty()).then_some(req.state.as_str());
            let tasks = repo.meta().read(|tx| tx.tasks(state))?;
            let mut out = Vec::new();
            for t in &tasks {
                let v = task_view(repo, t)?;
                if !req.ready_only || (v.ready && v.state == "open") {
                    out.push(v);
                }
            }
            Ok(v1::ListTasksResponse { tasks: out })
        })
        .await
    }

    /// Adds a dependency (cycles are refused).
    pub async fn add_dependency(&self, call: &Call, req: v1::AddDependencyRequest) -> ApiResult<v1::Task> {
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::TaskWrite, None, move |repo, actor| {
            add_dependency(repo.meta(), &req.task, &req.depends_on, actor.as_str())?;
            let t = repo.meta().read(|tx| tx.task(&req.task))?.ok_or_else(|| ApiError::not_found("task not found"))?;
            task_view(repo, &t)
        })
        .await
    }

    /// Marks a task done or cancelled: by its owner, or - with `agent.operate` - by an operator, whatever the
    /// owner (the audit entry records the override).
    pub async fn finish_task(&self, call: &Call, req: v1::FinishTaskRequest) -> ApiResult<v1::Task> {
        let repo_name = require(&req.repo, "repo")?.to_string();
        let operator = self.authorize(call, &repo_name, Cap::AgentOperate, None).await.is_ok();
        self.with_repo_mut(call, &repo_name, Cap::TaskWrite, None, move |repo, actor| {
            if operator {
                finish_task_as_operator(repo.meta(), &req.id, &req.state, actor.as_str())?;
            } else {
                finish_task(repo.meta(), &req.id, &req.state, actor.as_str())?;
            }
            let t = repo.meta().read(|tx| tx.task(&req.id))?.ok_or_else(|| ApiError::not_found("task not found"))?;
            task_view(repo, &t)
        })
        .await
    }

    /// Acquires a soft or exclusive lease.
    pub async fn acquire_lease(
        &self,
        call: &Call,
        req: v1::AcquireLeaseRequest,
    ) -> ApiResult<v1::AcquireLeaseResponse> {
        let idem = self.idem();
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::LeaseWrite, None, move |repo, actor| {
            let task = (!req.task_id.is_empty()).then_some(req.task_id.as_str());
            let request = format!(
                "acquire_lease\0{actor}\0{}\0{}\0{}\0{}",
                req.scope,
                req.exclusive,
                req.ttl_ms,
                task.unwrap_or("")
            );
            keyed(repo, idem, "lease.acquire", &req.idempotency_key, &request, |claim| {
                let (_, response) = acquire_with(
                    repo.meta(),
                    &req.scope,
                    actor.as_str(),
                    req.exclusive,
                    req.ttl_ms,
                    task,
                    |tx, got| {
                        let response = v1::AcquireLeaseResponse {
                            lease: Some(convert::lease(&got.lease)),
                            overlapping: got.overlaps.iter().map(convert::lease).collect(),
                        };
                        claim.record(tx, &response)?;
                        Ok(response)
                    },
                )
                .map_err(|e| match e {
                    ag_agent::AgentError::Conflict(m) => ApiError::new(ag_protocol::ErrorCode::LeaseConflict, m),
                    other => other.into(),
                })?;
                Ok(response)
            })
        })
        .await
    }

    /// Releases a lease.
    pub async fn release_lease(&self, call: &Call, req: v1::LeaseRef) -> ApiResult<v1::Empty> {
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::LeaseWrite, None, move |repo, actor| {
            check_lease_run(repo, &req.lease_id, actor)?;
            release(repo.meta(), &req.lease_id, actor.as_str())?;
            Ok(v1::Empty {})
        })
        .await
    }

    /// Extends a lease (the fencing token stays the same).
    pub async fn extend_lease(&self, call: &Call, req: v1::ExtendLeaseRequest) -> ApiResult<v1::Lease> {
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::LeaseWrite, None, move |repo, actor| {
            check_lease_run(repo, &req.lease_id, actor)?;
            Ok(convert::lease(&extend(repo.meta(), &req.lease_id, actor.as_str(), req.ttl_ms)?))
        })
        .await
    }

    /// Lists leases.
    pub async fn list_leases(&self, call: &Call, req: v1::ListLeasesRequest) -> ApiResult<v1::ListLeasesResponse> {
        let limit = page_size(req.limit);
        self.with_repo(call, require(&req.repo, "repo")?, Cap::LeaseRead, None, move |repo, _| {
            let mut truncated = false;
            let leases = if req.active_only {
                repo.meta().read(|tx| tx.leases_active())?
            } else {
                // One more than asked for, to learn whether the history goes on.
                let mut all = repo.meta().read(|tx| tx.leases_all(limit + 1))?;
                if all.len() > limit {
                    all.truncate(limit);
                    truncated = true;
                }
                all
            };
            Ok(v1::ListLeasesResponse { leases: leases.iter().map(convert::lease).collect(), truncated })
        })
        .await
    }

    /// Stores a provenance record signed by the agent itself.
    pub async fn submit_provenance(
        &self,
        call: &Call,
        req: v1::SubmitProvenanceRequest,
    ) -> ApiResult<v1::ProvenanceEntry> {
        let wire = req.record.ok_or_else(|| ApiError::invalid("'record' is required"))?;
        let record = record_from_wire(&wire)?;
        let sig = Signature::from_hex(&req.signature).map_err(|e| ApiError::invalid(format!("signature: {e}")))?;
        // The caller must be the agent the record speaks for.
        if record.agent.principal() != call.principal.identity.principal() && !call.principal.bootstrap {
            return Err(ApiError::denied(format!(
                "this credential authenticates as {}; it may not submit provenance for {}",
                call.principal.identity, record.agent
            )));
        }
        // A credential bound to a run speaks for that run only.
        if let Some((_, bound)) = call.principal.scope.bound_run()
            && record.run_id.as_deref() != Some(bound)
        {
            return Err(ApiError::denied(format!(
                "this credential is bound to run '{bound}'; it may only submit provenance recorded for that run"
            )));
        }
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::ProvenanceWrite, None, move |repo, _| {
            let id = attach_signed(repo.meta(), &record, &sig)?;
            let row = repo
                .meta()
                .read(|tx| Ok(tx.provenance_for(&record.changeset)?.into_iter().find(|r| r.id == id)))?
                .ok_or_else(|| ApiError::internal("provenance vanished"))?;
            Ok(convert::provenance_entry(&row, Some(&record)))
        })
        .await
    }

    /// Provenance records of a changeset, each flagged verified or not.
    pub async fn list_provenance(
        &self,
        call: &Call,
        req: v1::ListProvenanceRequest,
    ) -> ApiResult<v1::ListProvenanceResponse> {
        let cs: ChangesetId = parse_id(&req.changeset, "changeset id")?;
        self.with_repo(call, require(&req.repo, "repo")?, Cap::ProvenanceRead, None, move |repo, _| {
            let checked = repo.meta().read(|tx| {
                let mut out = Vec::new();
                for row in tx.provenance_for(&cs)? {
                    let verdict = verify(tx, &row);
                    out.push((row, verdict));
                }
                Ok(out)
            })?;
            let mut entries = Vec::new();
            for (row, verdict) in checked {
                let verified = match verdict {
                    Ok(record) => Some(record),
                    // A record that fails verification (bad signature, unknown key, ...) is listed as unverified.
                    Err(ag_agent::AgentError::Verification(_)) => None,
                    // A fault of the store is not a verdict about the record: say so.
                    Err(other) => return Err(other.into()),
                };
                entries.push(convert::provenance_entry(&row, verified.as_ref()));
            }
            Ok(v1::ListProvenanceResponse { entries })
        })
        .await
    }

    /// Path-level conflict prediction for a stream.
    pub async fn predict(&self, call: &Call, req: v1::PredictRequest) -> ApiResult<v1::PredictResponse> {
        let stream = require(&req.stream, "stream")?.to_string();
        let s = stream.clone();
        self.with_repo(call, require(&req.repo, "repo")?, Cap::Predict, Some(&stream), move |repo, _| {
            let p = predict(repo.meta(), &s)?;
            Ok(v1::PredictResponse {
                overlaps: p
                    .overlaps
                    .into_iter()
                    .map(|o| v1::Overlap { a: o.a, b: o.b, kind: o.kind, paths: o.paths })
                    .collect(),
            })
        })
        .await
    }
}
