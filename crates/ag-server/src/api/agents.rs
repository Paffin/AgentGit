//! Agents: identities, runs, tasks, leases, provenance, conflict prediction.

use ag_agent::leases::{acquire_with, extend, release};
use ag_agent::predict::predict;
use ag_agent::provenance::{ProvenanceRecord, SCHEMA, attach_signed, verify};
use ag_agent::registry::{finish_run, heartbeat, register_agent_with_key, start_run_with};
use ag_agent::tasks::{add_dependency, create_task_with, finish_task, unfinished_dependencies};
use ag_crypto::{PublicKey, Signature};
use ag_protocol::v1;
use ag_storage::meta::{MetaError, TaskRow, Tx};
use ag_types::{ChangesetId, Identity, IdentityKind, Timestamp};

use super::{convert, keyed, page_size, parse_id, require};
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::{ApiError, ApiResult};

/// An agent principal may only act for itself; humans and services (an
/// orchestrator, an operator) may act for any agent.
fn check_agent_scope(actor: &Identity, agent_name: &str) -> ApiResult<()> {
    let p = actor.principal();
    if p.kind() == IdentityKind::Agent && p.name() != agent_name {
        return Err(ApiError::denied(format!("{p} may not act for agent '{agent_name}'")));
    }
    Ok(())
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
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::AgentRun, None, move |repo, actor| {
            check_agent_scope(actor, &agent)?;
            let request = format!("start_run\0{actor}\0{agent}\0{}", task.as_deref().unwrap_or(""));
            keyed(repo, "run.start", &key, &request, |claim| {
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
    ) -> ApiResult<ag_storage::meta::RunRow> {
        let run = repo
            .meta()
            .read(|tx| tx.run(run_id))?
            .ok_or_else(|| ApiError::not_found(format!("run '{run_id}' not found")))?;
        check_agent_scope(actor, &run.agent)?;
        Ok(run)
    }

    /// Records a heartbeat of a run.
    pub async fn heartbeat(&self, call: &Call, req: v1::HeartbeatRequest) -> ApiResult<v1::HeartbeatResponse> {
        let now = self.clock.now().as_millis();
        self.with_repo(call, require(&req.repo, "repo")?, Cap::AgentRun, None, move |repo, actor| {
            Self::check_run_owner(repo, &req.run_id, actor)?;
            heartbeat(repo.meta(), &req.run_id)?;
            Ok(v1::HeartbeatResponse { server_time_ms: now })
        })
        .await
    }

    /// Finishes a run and releases its leases.
    pub async fn complete_run(&self, call: &Call, req: v1::CompleteRunRequest) -> ApiResult<v1::Run> {
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::AgentRun, None, move |repo, actor| {
            Self::check_run_owner(repo, &req.run_id, actor)?;
            let summary = (!req.summary.is_empty()).then_some(req.summary.as_str());
            finish_run(repo.meta(), &req.run_id, &req.status, summary)?;
            let row = repo.meta().read(|tx| tx.run(&req.run_id))?.ok_or_else(|| ApiError::internal("run vanished"))?;
            Ok(convert::run(&row))
        })
        .await
    }

    /// Lists runs (newest first).
    pub async fn list_runs(&self, call: &Call, req: v1::ListRunsRequest) -> ApiResult<v1::ListRunsResponse> {
        let limit = if req.limit == 0 { 100 } else { req.limit.min(1000) } as usize;
        self.with_repo(call, require(&req.repo, "repo")?, Cap::AgentRead, None, move |repo, _| {
            let agent = (!req.agent.is_empty()).then_some(req.agent.as_str());
            let mut runs = repo.meta().read(|tx| tx.runs(agent, limit))?;
            if req.active_only {
                runs.retain(|r| r.status == "running");
            }
            Ok(v1::ListRunsResponse { runs: runs.iter().map(convert::run).collect() })
        })
        .await
    }

    /// Creates a task (with dependencies).
    pub async fn create_task(&self, call: &Call, req: v1::CreateTaskRequest) -> ApiResult<v1::Task> {
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::TaskWrite, None, move |repo, actor| {
            let request = format!(
                "create_task\0{actor}\0{}\0{}\0{}\0{}",
                req.title,
                req.description,
                req.scope.join(","),
                req.depends_on.join(",")
            );
            keyed(repo, "task.create", &req.idempotency_key, &request, |claim| {
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

    /// Marks a task done or cancelled.
    pub async fn finish_task(&self, call: &Call, req: v1::FinishTaskRequest) -> ApiResult<v1::Task> {
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::TaskWrite, None, move |repo, actor| {
            finish_task(repo.meta(), &req.id, &req.state, actor.as_str())?;
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
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::LeaseWrite, None, move |repo, actor| {
            let task = (!req.task_id.is_empty()).then_some(req.task_id.as_str());
            let request = format!(
                "acquire_lease\0{actor}\0{}\0{}\0{}\0{}",
                req.scope,
                req.exclusive,
                req.ttl_ms,
                task.unwrap_or("")
            );
            keyed(repo, "lease.acquire", &req.idempotency_key, &request, |claim| {
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
            release(repo.meta(), &req.lease_id, actor.as_str())?;
            Ok(v1::Empty {})
        })
        .await
    }

    /// Extends a lease (the fencing token stays the same).
    pub async fn extend_lease(&self, call: &Call, req: v1::ExtendLeaseRequest) -> ApiResult<v1::Lease> {
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::LeaseWrite, None, move |repo, actor| {
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
