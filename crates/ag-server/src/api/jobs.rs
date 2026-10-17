//! Execution plane: validation jobs for trusted workers.
//!
//! When a stream's policy requires a check and says *how* to run it
//! (`required_validations[].run`), the server queues a **validation job** for
//! the exact candidate tree. A worker with the `runner` role claims it, checks
//! out that tree, runs the command in a sandbox, signs an attestation with its
//! own key and completes the job. Whether the attestation *counts* is decided
//! by the policy (`trusted_issuers`), as for every attestation — a job is only
//! a delivery mechanism.
//!
//! Invariants enforced here:
//! * the worker identity is bound to the credential (`claimed_by` is
//!   `<principal>|<label>`), so a runner cannot finish another runner's job;
//! * a completion must carry an attestation for *exactly* the job's tree,
//!   check and policy version, issued by the caller;
//! * the candidate handed to the worker must still hash to the job's tree id,
//!   otherwise it is `stale_candidate` and nothing is checked.

use std::time::{Duration, Instant};

use ag_integrate::Preparation;
use ag_policy::RunSpec;
use ag_protocol::v1;
use ag_storage::meta::{JobRow, JobState};
use ag_types::Timestamp;
use serde_json::json;

use super::tree::tree_page;
use super::{parse_id, require};
use crate::app::{App, Call};
use crate::authz::{Authorizer, Cap};
use crate::error::{ApiError, ApiResult};

const KIND_VALIDATE: &str = "validate";
const DEFAULT_LEASE_MS: i64 = 120_000;
const MAX_LEASE_MS: i64 = 3_600_000;
const MAX_WAIT_MS: u32 = 30_000;

fn run_to_wire(r: &RunSpec) -> v1::RunSpec {
    v1::RunSpec {
        argv: r.argv.clone(),
        timeout_secs: r.timeout_secs,
        memory_mb: r.memory_mb,
        cpu_secs: r.cpu_secs,
        max_pids: r.max_pids,
        network: r.network,
        image: r.image.clone().unwrap_or_default(),
        env: r.env.clone().into_iter().collect(),
    }
}

fn job_to_wire(repo: &str, j: &JobRow) -> v1::Job {
    let validation = (j.kind == KIND_VALIDATE).then(|| {
        let p = &j.payload;
        v1::ValidationTask {
            proposal: p["proposal"].as_str().unwrap_or_default().into(),
            check: p["check"].as_str().unwrap_or_default().into(),
            tree_id: p["tree_id"].as_str().unwrap_or_default().into(),
            policy_version: p["policy_version"].as_u64().unwrap_or(0),
            stream: p["stream"].as_str().unwrap_or_default().into(),
            run: serde_json::from_value::<RunSpec>(p["run"].clone()).ok().map(|r| run_to_wire(&r)),
            required_environment_digest: p["required_environment_digest"].as_str().unwrap_or_default().into(),
        }
    });
    v1::Job {
        id: j.id.clone(),
        repo: repo.into(),
        kind: j.kind.clone(),
        state: j.state.as_str().into(),
        validation,
        result_json: j.result.as_ref().map(|r| r.to_string()).unwrap_or_default(),
        error: j.error.clone().unwrap_or_default(),
        attempts: j.attempts,
        claimed_by: j.claimed_by.clone().unwrap_or_default(),
        claimed_until_ms: j.claimed_until.map(|t| t.as_millis()).unwrap_or(0),
        created_at_ms: j.created_at.as_millis(),
        updated_at_ms: j.updated_at.as_millis(),
    }
}

fn dedupe_key(proposal: &str, check: &str, tree: &str, policy_version: u64) -> String {
    format!("validate:{proposal}:{check}:{tree}:{policy_version}")
}

/// `<principal>|<label>`: the label is the worker's own name for this loop.
fn worker_id(call: &Call, label: &str) -> ApiResult<String> {
    let label = if label.is_empty() { "default" } else { label };
    if label.len() > 64 || !label.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
        return Err(ApiError::invalid("worker must be 1-64 characters of [A-Za-z0-9._-]"));
    }
    Ok(format!("{}|{label}", call.principal.identity.principal()))
}

impl App {
    /// Queues one job per required check that has a runner configuration and
    /// no passing attestation yet (idempotent per candidate tree).
    pub(crate) async fn enqueue_validation_jobs(
        &self,
        call: &Call,
        repo: &str,
        prep: &Preparation,
        only: &[String],
    ) -> ApiResult<Vec<v1::Job>> {
        let Some(tree) = prep.preview.candidate_tree else { return Ok(Vec::new()) };
        let todo: Vec<_> = prep
            .validations
            .iter()
            .filter(|v| v.state == "missing" && v.run.is_some() && (only.is_empty() || only.contains(&v.check)))
            .cloned()
            .collect();
        if todo.is_empty() {
            return self.existing_validation_jobs(call, repo, prep).await;
        }
        let (proposal, policy_version, stream_hint) = (prep.proposal.clone(), prep.policy_version, repo.to_string());
        let name = repo.to_string();
        let events = self.metrics.job_events.clone();
        let jobs = self
            .with_repo_mut(call, &stream_hint, Cap::IntegrateValidate, None, move |r, actor| {
                let stream = r.find_proposal(&proposal)?.stream;
                let mut out = Vec::new();
                for v in &todo {
                    let payload = json!({
                        "proposal": proposal,
                        "check": v.check,
                        "tree_id": tree.to_string(),
                        "policy_version": policy_version,
                        "stream": stream,
                        "run": v.run,
                        "required_environment_digest": v.required_environment_digest,
                    });
                    let key = dedupe_key(&proposal, &v.check, &tree.to_string(), policy_version);
                    let job = r.meta().write(|tx| {
                        let existing = tx.job_list(None, 1000)?.into_iter().find(|j| j.dedupe_key.as_deref() == Some(&key));
                        if let Some(j) = existing {
                            return Ok(j);
                        }
                        let j = tx.job_enqueue(KIND_VALIDATE, &payload, Some(&key), Timestamp::from_millis(0))?;
                        events.with_label_values(&["enqueued"]).inc();
                        tx.audit_append(
                            actor.as_str(),
                            "job.enqueue",
                            &j.id,
                            &json!({"kind": KIND_VALIDATE, "proposal": proposal, "check": v.check, "tree": tree.to_string()}),
                        )?;
                        Ok(j)
                    })?;
                    out.push(job_to_wire(&name, &job));
                }
                Ok(out)
            })
            .await?;
        self.note_jobs_queued(repo);
        Ok(jobs)
    }

    /// Jobs already queued or running for the candidate (no side effects).
    pub(crate) async fn existing_validation_jobs(
        &self,
        call: &Call,
        repo: &str,
        prep: &Preparation,
    ) -> ApiResult<Vec<v1::Job>> {
        let Some(tree) = prep.preview.candidate_tree else { return Ok(Vec::new()) };
        let (proposal, tree, pv) = (prep.proposal.clone(), tree.to_string(), prep.policy_version);
        let name = repo.to_string();
        self.with_repo(call, repo, Cap::JobsRead, None, move |r, _| {
            let jobs = r.meta().read(|tx| tx.job_list(None, 500))?;
            Ok(jobs
                .iter()
                .filter(|j| {
                    j.kind == KIND_VALIDATE
                        && j.payload["proposal"] == proposal.as_str()
                        && j.payload["tree_id"] == tree.as_str()
                        && j.payload["policy_version"] == pv
                })
                .map(|j| job_to_wire(&name, j))
                .collect())
        })
        .await
    }

    /// Queues validation jobs for a proposal's required checks.
    pub async fn request_validation(
        &self,
        call: &Call,
        req: v1::RequestValidationRequest,
    ) -> ApiResult<v1::RequestValidationResponse> {
        let repo = require(&req.repo, "repo")?.to_string();
        let proposal = require(&req.proposal, "proposal")?.to_string();
        self.authorize(call, &repo, Cap::IntegrateValidate, None).await?;
        let p = proposal.clone();
        let prep = self
            .with_repo(call, &repo, Cap::IntegratePrepare, None, move |r, actor| Ok(r.prepare(&p, actor, &[])?))
            .await?;
        if prep.preview.candidate_tree.is_none() {
            return Err(ApiError::new(
                ag_protocol::ErrorCode::FailedPrecondition,
                "this proposal has no unambiguous candidate to validate (conflict or unrelated history)",
            ));
        }
        let jobs = self.enqueue_validation_jobs(call, &repo, &prep, &req.checks).await?;
        Ok(v1::RequestValidationResponse { jobs })
    }

    // ---- bookkeeping: which repositories may have queued work ----------------------

    pub(crate) fn note_jobs_queued(&self, repo: &str) {
        self.job_repos.lock().unwrap_or_else(|e| e.into_inner()).insert(repo.to_string());
        self.job_notify.notify_waiters();
    }

    /// Finds repositories with queued jobs (called at startup; a job queued
    /// by another server process is picked up by the periodic rescan).
    pub fn rebuild_job_index(&self) {
        let Ok(repos) = self.control.repos("", "", 100_000) else { return };
        for row in repos {
            if let Ok(h) = self.repos.handle(&row.name)
                && h.meta().read(|tx| tx.job_list(Some(JobState::Queued), 1)).map(|v| !v.is_empty()).unwrap_or(false)
            {
                self.note_jobs_queued(&row.name);
            }
        }
    }

    /// Queued jobs across the repositories known to have work (blocking).
    pub fn count_queued_jobs(&self) -> i64 {
        let repos: Vec<String> = self.job_repos.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect();
        repos
            .iter()
            .filter_map(|name| self.repos.handle(name).ok())
            .map(|h| h.meta().read(|tx| tx.job_list(Some(JobState::Queued), 1000)).map(|v| v.len() as i64).unwrap_or(0))
            .sum()
    }

    // ---- worker API ----------------------------------------------------------------------

    async fn try_claim(
        &self,
        call: &Call,
        repo: &str,
        kinds: Vec<String>,
        worker: String,
        lease_ms: i64,
    ) -> ApiResult<Option<v1::Job>> {
        let name = repo.to_string();
        let events = self.metrics.job_events.clone();
        self.with_repo_mut(call, repo, Cap::JobsClaim, None, move |r, actor| {
            let kinds: Vec<&str> = kinds.iter().map(String::as_str).collect();
            let job = r.meta().write(|tx| {
                let j = tx.job_claim(&kinds, &worker, lease_ms)?;
                if let Some(j) = &j {
                    events.with_label_values(&["claimed"]).inc();
                    tx.audit_append(
                        actor.as_str(),
                        "job.claim",
                        &j.id,
                        &json!({"worker": worker, "attempt": j.attempts}),
                    )?;
                }
                Ok(j)
            })?;
            Ok(job.map(|j| job_to_wire(&name, &j)))
        })
        .await
    }

    /// Claims the next queued job (long-polls up to `wait_ms`). With no
    /// `repo`, every repository the caller may run jobs for is considered.
    pub async fn claim_job(&self, call: &Call, req: v1::ClaimJobRequest) -> ApiResult<v1::ClaimJobResponse> {
        let worker = worker_id(call, &req.worker)?;
        let lease_ms = if req.lease_ms <= 0 { DEFAULT_LEASE_MS } else { req.lease_ms.min(MAX_LEASE_MS) };
        for k in &req.kinds {
            if k != KIND_VALIDATE {
                return Err(ApiError::invalid(format!("unknown job kind '{k}'")));
            }
        }
        let wait = Duration::from_millis(u64::from(req.wait_ms.min(MAX_WAIT_MS)));
        let deadline = Instant::now() + wait;
        loop {
            let notified = self.job_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let candidates: Vec<String> = if req.repo.is_empty() {
                let bindings = self.bindings().await?;
                let names: Vec<String> =
                    self.job_repos.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect();
                names
                    .into_iter()
                    .filter(|n| Authorizer::check(&call.principal, &bindings, n, Cap::JobsClaim, None).is_ok())
                    .collect()
            } else {
                vec![req.repo.clone()]
            };
            for repo in candidates {
                if let Some(job) = self.try_claim(call, &repo, req.kinds.clone(), worker.clone(), lease_ms).await? {
                    return Ok(v1::ClaimJobResponse { job: Some(job) });
                }
                if req.repo.is_empty() {
                    self.job_repos.lock().unwrap_or_else(|e| e.into_inner()).remove(&repo);
                }
            }
            if Instant::now() >= deadline {
                return Ok(v1::ClaimJobResponse { job: None });
            }
            let remaining = deadline.saturating_duration_since(Instant::now()).min(Duration::from_secs(1));
            let _ = tokio::time::timeout(remaining, notified).await;
            if req.repo.is_empty() && self.job_repos.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
                // Another server process may have queued work: look again now and then.
                self.rescan_jobs_if_due();
            }
        }
    }

    fn rescan_jobs_if_due(&self) {
        let now = Instant::now();
        let mut last = self.job_rescan.lock().unwrap_or_else(|e| e.into_inner());
        if now.duration_since(*last) >= Duration::from_secs(5) {
            *last = now;
            let app = self.clone();
            tokio::task::spawn_blocking(move || app.rebuild_job_index());
        }
    }

    /// Extends the lease on a claimed job.
    pub async fn heartbeat_job(&self, call: &Call, req: v1::HeartbeatJobRequest) -> ApiResult<v1::Job> {
        let worker = worker_id(call, &req.worker)?;
        let lease_ms = if req.lease_ms <= 0 { DEFAULT_LEASE_MS } else { req.lease_ms.min(MAX_LEASE_MS) };
        let repo = require(&req.repo, "repo")?.to_string();
        let name = repo.clone();
        self.with_repo_mut(call, &repo, Cap::JobsClaim, None, move |r, _| {
            let j = r.meta().write(|tx| tx.job_heartbeat(&req.job_id, &worker, lease_ms))?;
            Ok(job_to_wire(&name, &j))
        })
        .await
    }

    /// Completes a job. For validation jobs the signed attestation is stored first.
    pub async fn complete_job(&self, call: &Call, req: v1::CompleteJobRequest) -> ApiResult<v1::Job> {
        let worker = worker_id(call, &req.worker)?;
        let repo = require(&req.repo, "repo")?.to_string();
        let issuer_ok = call.principal.identity.principal();
        let name = repo.clone();
        let events = self.metrics.job_events.clone();
        let result_json = if req.result_json.is_empty() {
            json!({})
        } else {
            serde_json::from_str(&req.result_json).map_err(|e| ApiError::invalid(format!("result: {e}")))?
        };
        self.with_repo_mut(call, &repo, Cap::AttestationSubmit, None, move |r, actor| {
            let job = r
                .meta()
                .read(|tx| tx.job_get(&req.job_id))?
                .ok_or_else(|| ApiError::not_found(format!("job '{}' not found", req.job_id)))?;
            if job.state != JobState::Claimed || job.claimed_by.as_deref() != Some(worker.as_str()) {
                return Err(ApiError::new(
                    ag_protocol::ErrorCode::FailedPrecondition,
                    "this worker does not hold the job (it finished, was cancelled, or its lease was taken over)",
                ));
            }
            let mut result = result_json;
            if job.kind == KIND_VALIDATE {
                if req.stale {
                    result["stale"] = json!(true);
                } else {
                    let wire = req.attestation.as_ref().ok_or_else(|| {
                        ApiError::invalid("a validation job is completed with a signed attestation (or stale=true)")
                    })?;
                    let p = &job.payload;
                    let expect = |field: &str, got: &str, want: &str| -> ApiResult<()> {
                        if got == want {
                            Ok(())
                        } else {
                            Err(ApiError::invalid(format!(
                                "attestation {field} '{got}' does not match the job's '{want}'"
                            )))
                        }
                    };
                    expect("tree_id", &wire.tree_id, p["tree_id"].as_str().unwrap_or_default())?;
                    expect("check", &wire.check, p["check"].as_str().unwrap_or_default())?;
                    if wire.policy_version != p["policy_version"].as_u64().unwrap_or(0) {
                        return Err(ApiError::invalid("attestation policy_version does not match the job"));
                    }
                    let wanted_env = p["required_environment_digest"].as_str().unwrap_or_default();
                    if !wanted_env.is_empty() {
                        expect("environment_digest", &wire.environment_digest, wanted_env)?;
                    }
                    let issuer = ag_types::Identity::new(wire.issuer.clone())?;
                    if issuer.principal() != issuer_ok {
                        return Err(ApiError::denied("a worker may only submit attestations it issued itself"));
                    }
                    let att = super::integration::attestation_from_wire(wire)?;
                    let tree: ag_types::TreeId = parse_id(&wire.tree_id, "tree id")?;
                    // Re-delivery after a crash between the two steps must not fail.
                    if !r.attestations(&tree)?.iter().any(|a| a.id == att.id) {
                        r.submit_attestation(&att)?;
                    }
                    result["attestation"] = json!(wire.id);
                    result["outcome"] = json!(wire.result);
                }
            }
            let done = r.meta().write(|tx| {
                let j = tx.job_complete(&job.id, &worker, &result)?;
                events.with_label_values(&["succeeded"]).inc();
                tx.audit_append(actor.as_str(), "job.complete", &j.id, &json!({"result": result}))?;
                Ok(j)
            })?;
            Ok(job_to_wire(&name, &done))
        })
        .await
    }

    /// Fails a job (permanently, or with a retry delay).
    pub async fn fail_job(&self, call: &Call, req: v1::FailJobRequest) -> ApiResult<v1::Job> {
        let worker = worker_id(call, &req.worker)?;
        let repo = require(&req.repo, "repo")?.to_string();
        let name = repo.clone();
        let events = self.metrics.job_events.clone();
        let out = self
            .with_repo_mut(call, &repo, Cap::JobsClaim, None, move |r, actor| {
                let retry = (req.retry_after_ms > 0).then_some(req.retry_after_ms);
                let j = r.meta().write(|tx| {
                    let j =
                        tx.job_fail(&req.job_id, &worker, &req.error.chars().take(2000).collect::<String>(), retry)?;
                    events.with_label_values(&[if j.state == JobState::Queued { "retried" } else { "failed" }]).inc();
                    tx.audit_append(
                        actor.as_str(),
                        "job.fail",
                        &j.id,
                        &json!({"error": req.error, "state": j.state.as_str()}),
                    )?;
                    Ok(j)
                })?;
                Ok(job_to_wire(&name, &j))
            })
            .await?;
        if out.state == "queued" {
            self.note_jobs_queued(&repo);
        }
        Ok(out)
    }

    /// One job.
    pub async fn get_job(&self, call: &Call, req: v1::GetJobRequest) -> ApiResult<v1::Job> {
        let name = require(&req.repo, "repo")?.to_string();
        let n = name.clone();
        self.with_repo(call, &name, Cap::JobsRead, None, move |r, _| {
            let j = r
                .meta()
                .read(|tx| tx.job_get(&req.job_id))?
                .ok_or_else(|| ApiError::not_found(format!("job '{}' not found", req.job_id)))?;
            Ok(job_to_wire(&n, &j))
        })
        .await
    }

    /// Lists jobs (newest first).
    pub async fn list_jobs(&self, call: &Call, req: v1::ListJobsRequest) -> ApiResult<v1::ListJobsResponse> {
        let name = require(&req.repo, "repo")?.to_string();
        let n = name.clone();
        let limit = if req.limit == 0 { 100 } else { req.limit.min(1000) } as usize;
        self.with_repo(call, &name, Cap::JobsRead, None, move |r, _| {
            let state = if req.state.is_empty() {
                None
            } else {
                Some(
                    req.state
                        .parse::<JobState>()
                        .map_err(|_| ApiError::invalid(format!("unknown job state '{}'", req.state)))?,
                )
            };
            let jobs = r.meta().read(|tx| tx.job_list(state, limit))?;
            Ok(v1::ListJobsResponse { jobs: jobs.iter().map(|j| job_to_wire(&n, j)).collect() })
        })
        .await
    }

    /// The candidate tree of a proposal, for a worker to check out. Refuses if
    /// it is not the tree the job was created for.
    pub async fn get_candidate(&self, call: &Call, req: v1::GetCandidateRequest) -> ApiResult<v1::GetTreeResponse> {
        let (size, token) = (req.page_size, req.page_token.clone());
        self.with_repo(call, require(&req.repo, "repo")?, Cap::TreeRead, None, move |r, _| {
            let preview = r.merge_preview(&req.proposal)?;
            let (Some(id), Some(tree)) = (preview.candidate_tree, preview.tree) else {
                return Err(ApiError::new(
                    ag_protocol::ErrorCode::StaleCandidate,
                    "the proposal has no unambiguous candidate any more",
                ));
            };
            if !req.expected_tree_id.is_empty() && id.to_string() != req.expected_tree_id {
                return Err(ApiError::new(
                    ag_protocol::ErrorCode::StaleCandidate,
                    format!("the candidate is now tree {id}, not {}", req.expected_tree_id),
                ));
            }
            tree_page(r, &tree, size, &token)
        })
        .await
    }
}
