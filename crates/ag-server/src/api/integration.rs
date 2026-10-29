//! Integration: prepare, attestations, integrate, and the ordered queue.

use ag_crypto::{PublicKey, Signature};
use ag_integrate::{IntegrateRequest, Preparation, Strategy};
use ag_policy::{Attestation, CheckResult};
use ag_protocol::v1;
use ag_types::{Identity, Timestamp, TreeId};

use super::{convert, parse_id, require};
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::{ApiError, ApiResult};

pub(crate) fn preparation_to_wire(p: &Preparation, jobs: Vec<v1::Job>) -> v1::Preparation {
    let pv = &p.preview;
    let no_candidate = pv.candidate_tree.is_none();
    let strategy = match (pv.strategy, no_candidate, pv.conflicts.is_empty() && pv.problem.is_none()) {
        (Strategy::AlreadyIntegrated, _, _) => "already_integrated",
        (_, true, _) => "",
        (s, false, _) => convert::strategy(s),
    };
    let strategy = if no_candidate && !pv.conflicts.is_empty() { "merge" } else { strategy };
    v1::Preparation {
        proposal: p.proposal.clone(),
        strategy: strategy.into(),
        candidate_tree: pv.candidate_tree.map(|t| t.to_string()).unwrap_or_default(),
        conflicts: pv.conflicts.iter().map(convert::conflict).collect(),
        base: pv.base.map(|b| b.to_string()).unwrap_or_default(),
        changed_files: pv.changed_files.clone(),
        problem: pv.problem.clone().unwrap_or_default(),
        algorithm: pv.algorithm.to_string(),
        policy_version: p.policy_version,
        blockers: p.blockers.iter().map(convert::blocker).collect(),
        validations: p.validations.iter().map(|v| v10(v, &jobs)).collect(),
        jobs,
    }
}

fn v10(v: &ag_integrate::ValidationState, jobs: &[v1::Job]) -> v1::ValidationStatus {
    let job = jobs.iter().find(|j| {
        j.validation.as_ref().is_some_and(|t| t.check == v.check) && matches!(j.state.as_str(), "queued" | "claimed")
    });
    let state = if v.state == "missing" && job.is_some() { "pending" } else { v.state };
    v1::ValidationStatus {
        check: v.check.clone(),
        state: state.into(),
        attestation_id: v.attestation_id.clone().unwrap_or_default(),
        job_id: job.map(|j| j.id.clone()).unwrap_or_default(),
    }
}

pub(crate) fn attestation_from_wire(a: &v1::Attestation) -> ApiResult<Attestation> {
    let bad = |what: &str, e: String| ApiError::invalid(format!("attestation {what}: {e}"));
    Ok(Attestation {
        id: require(&a.id, "attestation.id")?.to_string(),
        tree_id: parse_id::<TreeId>(&a.tree_id, "tree id")?,
        check: require(&a.check, "attestation.check")?.to_string(),
        result: match a.result.as_str() {
            "pass" => CheckResult::Pass,
            "fail" => CheckResult::Fail,
            other => {
                return Err(ApiError::invalid(format!("attestation result must be 'pass' or 'fail', not '{other}'")));
            }
        },
        policy_version: a.policy_version,
        environment_digest: a.environment_digest.clone(),
        issuer: Identity::new(a.issuer.clone()).map_err(|e| bad("issuer", e.to_string()))?,
        issued_at: Timestamp::from_millis(a.issued_at_ms),
        public_key: PublicKey::from_hex(&a.public_key).map_err(|e| bad("public_key", e.to_string()))?,
        signature: Signature::from_hex(&a.signature).map_err(|e| bad("signature", e.to_string()))?,
    })
}

impl App {
    /// Previews an integration and evaluates policy against the exact
    /// candidate tree. With `enqueue_validations`, also queues the checks the
    /// policy knows how to run.
    ///
    /// The verdict and the jobs that validate the candidate are read in one transaction
    /// ([`ag_integrate::Repository::prepare_with`]), so a validation that finishes at that moment is reported
    /// either as still running or as passed/failed with its attestation - never as missing with nothing in flight.
    pub async fn prepare(&self, call: &Call, req: v1::PrepareRequest) -> ApiResult<v1::Preparation> {
        let repo_name = require(&req.repo, "repo")?.to_string();
        let proposal = require(&req.proposal, "proposal")?.to_string();
        if req.enqueue_validations {
            self.authorize(call, &repo_name, Cap::IntegrateValidate, None).await?;
        }
        // The jobs are part of the answer.
        self.authorize(call, &repo_name, Cap::JobsRead, None).await?;
        let (mut prep, mut jobs) = self.prepare_snapshot(call, &repo_name, &proposal).await?;
        if req.enqueue_validations && prep.validations.iter().any(|v| v.state == "missing" && v.run.is_some()) {
            self.enqueue_validation_jobs(call, &repo_name, &prep, &[]).await?;
            // What the answer shows is read again, as a whole: the jobs just queued - or finished by a worker
            // since the first read - and the attestations they produced belong together.
            (prep, jobs) = self.prepare_snapshot(call, &repo_name, &proposal).await?;
        }
        Ok(preparation_to_wire(&prep, jobs))
    }

    /// One consistent reading of a proposal's preparation and its validation jobs.
    async fn prepare_snapshot(
        &self,
        call: &Call,
        repo: &str,
        proposal: &str,
    ) -> ApiResult<(Preparation, Vec<v1::Job>)> {
        let (name, p) = (repo.to_string(), proposal.to_string());
        self.with_proposal(call, repo, Cap::IntegratePrepare, p.clone(), move |r, actor| {
            let (prep, rows) = r.prepare_with(&p, actor, &[], super::jobs::validation_jobs_in)?;
            Ok((prep, rows.iter().map(|j| super::jobs::job_to_wire(&name, j)).collect()))
        })
        .await
    }

    /// Stores a signed attestation (whether it *counts* is up to the policy).
    pub async fn submit_attestation(
        &self,
        call: &Call,
        req: v1::SubmitAttestationRequest,
    ) -> ApiResult<v1::SubmitAttestationResponse> {
        let wire = req.attestation.ok_or_else(|| ApiError::invalid("'attestation' is required"))?;
        let att = attestation_from_wire(&wire)?;
        if att.issuer.principal() != call.principal.identity.principal() && !call.principal.bootstrap {
            return Err(ApiError::denied(format!(
                "this credential authenticates as {}, it may not submit attestations issued by {}",
                call.principal.identity, att.issuer
            )));
        }
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::AttestationSubmit, None, move |repo, _| {
            repo.submit_attestation(&att)?;
            Ok(v1::SubmitAttestationResponse { accepted: true })
        })
        .await
    }

    /// Attestations recorded for a tree.
    pub async fn list_attestations(
        &self,
        call: &Call,
        req: v1::ListAttestationsRequest,
    ) -> ApiResult<v1::ListAttestationsResponse> {
        let tree: TreeId = parse_id(&req.tree_id, "tree id")?;
        self.with_repo(call, require(&req.repo, "repo")?, Cap::AttestationRead, None, move |repo, _| {
            Ok(v1::ListAttestationsResponse {
                attestations: repo.attestations(&tree)?.iter().map(convert::attestation_row).collect(),
            })
        })
        .await
    }

    /// Integrates a proposal into its stream.
    pub async fn integrate(&self, call: &Call, req: v1::IntegrateRequest) -> ApiResult<v1::IntegrationResult> {
        let repo_name = require(&req.repo, "repo")?.to_string();
        let proposal = require(&req.proposal, "proposal")?.to_string();
        // Find the stream first so the credential's stream scope can be enforced.
        let p = proposal.clone();
        let stream = self
            .with_repo(call, &repo_name, Cap::ProposalRead, None, move |repo, _| Ok(repo.find_proposal(&p)?.stream))
            .await?;
        let key = (!req.idempotency_key.is_empty()).then(|| req.idempotency_key.clone());
        let tokens = req.fencing_tokens.clone();
        let result = self
            .with_repo_mut(call, &repo_name, Cap::IntegrateRun, Some(&stream), move |repo, actor| {
                let res = repo.integrate(&IntegrateRequest {
                    proposal_id: proposal,
                    actor: actor.clone(),
                    idempotency_key: key,
                    fencing_tokens: tokens,
                })?;
                Ok(convert::integration_result(&res))
            })
            .await?;
        let status = result.outcome.as_ref().map(|o| o.status.clone()).unwrap_or_default();
        self.metrics.integrations.with_label_values(&[&status]).inc();
        Ok(result)
    }

    /// Appends a proposal to its stream's integration queue.
    pub async fn enqueue(&self, call: &Call, req: v1::EnqueueRequest) -> ApiResult<v1::QueueEntry> {
        self.with_proposal_mut(
            call,
            require(&req.repo, "repo")?,
            Cap::QueueWrite,
            req.proposal.clone(),
            move |repo, actor| Ok(convert::queue_entry(&repo.enqueue(&req.proposal, actor)?)),
        )
        .await
    }

    /// The queue of a stream.
    pub async fn list_queue(&self, call: &Call, req: v1::ListQueueRequest) -> ApiResult<v1::ListQueueResponse> {
        let stream = require(&req.stream, "stream")?.to_string();
        let scope = stream.clone();
        self.with_repo(call, require(&req.repo, "repo")?, Cap::QueueRead, Some(&scope), move |repo, _| {
            Ok(v1::ListQueueResponse { entries: repo.queue(&stream)?.iter().map(convert::queue_entry).collect() })
        })
        .await
    }

    /// Processes the queued proposals of a stream in order.
    pub async fn run_queue(&self, call: &Call, req: v1::RunQueueRequest) -> ApiResult<v1::RunQueueResponse> {
        let stream = require(&req.stream, "stream")?.to_string();
        let s = stream.clone();
        let out = self
            .with_repo_mut(call, require(&req.repo, "repo")?, Cap::QueueRun, Some(&stream), move |repo, actor| {
                let report = repo.run_queue(&s, actor)?;
                let outcomes = report.outcomes.clone();
                let queue = repo.queue(&s)?;
                let mut processed = Vec::new();
                let all: Vec<&String> = report
                    .integrated
                    .iter()
                    .chain(&report.blocked)
                    .chain(&report.rejected)
                    .chain(report.failed.iter().map(|(p, _)| p))
                    .collect();
                for id in all {
                    if let Some(e) = queue.iter().rev().find(|e| &e.proposal == id) {
                        processed.push(convert::queue_entry(e));
                    }
                }
                let stopped_at = report
                    .blocked
                    .first()
                    .or(report.rejected.first())
                    .or(report.failed.first().map(|(p, _)| p))
                    .cloned()
                    .unwrap_or_default();
                Ok((v1::RunQueueResponse { processed, stopped_at }, outcomes))
            })
            .await?;
        // Every entry the run decided counts under its own outcome, like an integrate call does.
        let (out, outcomes) = out;
        for status in outcomes {
            self.metrics.integrations.with_label_values(&[status]).inc();
        }
        Ok(out)
    }
}
