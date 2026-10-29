//! Engine rows → protocol messages.

use ag_agent::provenance::ProvenanceRecord;
use ag_integrate::{Blocker, IntegrationResult, Outcome, Strategy};
use ag_protocol::v1;
use ag_storage::meta::{
    AgentRow, AttestationRow, AuditEntry, IntentRow, LeaseRow, PinRow, PolicyRow, ProposalRow, ProvenanceRow, QueueRow,
    ReflogEntry, ReviewRow, RunRow, StreamRow, TaskRow, WorkspaceRow,
};
use ag_types::Timestamp;

use crate::error::{ApiError, ApiResult};

fn ms(t: Timestamp) -> i64 {
    t.as_millis()
}

fn id_or_empty<T: std::fmt::Display>(o: &Option<T>) -> String {
    o.as_ref().map(|v| v.to_string()).unwrap_or_default()
}

pub fn stream(r: &StreamRow, policy_version: u64) -> v1::Stream {
    v1::Stream {
        name: r.name.to_string(),
        head: id_or_empty(&r.state.head),
        version: r.state.version,
        created_at_ms: ms(r.created_at),
        created_by: r.created_by.clone(),
        policy_version,
    }
}

pub fn reflog(e: &ReflogEntry) -> v1::ReflogEntry {
    v1::ReflogEntry {
        seq: e.seq,
        old_head: id_or_empty(&e.old_head),
        new_head: id_or_empty(&e.new_head),
        version: e.version,
        operation_id: e.operation_id.clone().unwrap_or_default(),
        actor: e.actor.clone(),
        timestamp_ms: ms(e.timestamp),
        reason: e.reason.clone(),
    }
}

pub fn workspace(w: &WorkspaceRow) -> v1::Workspace {
    v1::Workspace {
        name: w.name.clone(),
        stream: w.stream.clone(),
        base: id_or_empty(&w.base),
        head: id_or_empty(&w.head),
        owner: w.owner.clone(),
        created_at_ms: ms(w.created_at),
        updated_at_ms: ms(w.updated_at),
    }
}

pub fn proposal(p: &ProposalRow) -> v1::Proposal {
    v1::Proposal {
        id: p.id.clone(),
        stream: p.stream.clone(),
        head: p.head.to_string(),
        title: p.title.clone(),
        description: p.description.clone(),
        author: p.author.clone(),
        state: p.state.clone(),
        task_id: p.task_id.clone().unwrap_or_default(),
        created_at_ms: ms(p.created_at),
        updated_at_ms: ms(p.updated_at),
        integrated_as: id_or_empty(&p.integrated_as),
    }
}

pub fn review(r: &ReviewRow) -> v1::Review {
    v1::Review {
        id: r.id,
        proposal: r.proposal.clone(),
        reviewer: r.reviewer.clone(),
        decision: r.decision.clone(),
        body: r.body.clone(),
        head: r.head.to_string(),
        timestamp_ms: ms(r.timestamp),
    }
}

pub fn attestation_row(a: &AttestationRow) -> v1::Attestation {
    v1::Attestation {
        id: a.id.clone(),
        tree_id: a.tree_id.to_string(),
        check: a.check_name.clone(),
        result: a.result.clone(),
        policy_version: a.policy_version,
        environment_digest: a.environment_digest.clone(),
        issuer: a.issuer.clone(),
        issued_at_ms: ms(a.issued_at),
        public_key: a.public_key.clone(),
        signature: a.signature.clone(),
    }
}

pub fn policy(p: &PolicyRow) -> v1::PolicyVersion {
    v1::PolicyVersion {
        stream: p.stream.clone(),
        version: p.version,
        document: p.doc.clone(),
        doc_hash: p.doc_hash.clone(),
        created_at_ms: ms(p.created_at),
        created_by: p.created_by.clone(),
    }
}

pub fn pin(p: &PinRow) -> v1::Pin {
    v1::Pin {
        name: p.name.clone(),
        changeset: p.changeset.to_string(),
        reason: p.reason.clone(),
        created_at_ms: ms(p.created_at),
    }
}

pub fn audit(a: &AuditEntry) -> v1::AuditEntry {
    v1::AuditEntry {
        seq: a.seq,
        timestamp_ms: ms(a.timestamp),
        actor: a.actor.clone(),
        action: a.action.clone(),
        target: a.target.clone(),
        details_json: a.details.clone(),
        hash: a.hash.clone(),
    }
}

pub fn agent(a: &AgentRow) -> v1::Agent {
    v1::Agent {
        name: a.name.clone(),
        identity: a.identity.clone(),
        public_key: a.public_key.clone(),
        role: a.role.clone(),
        created_at_ms: ms(a.created_at),
        disabled: a.disabled,
        executor: None,
    }
}

pub fn run(r: &RunRow) -> v1::Run {
    v1::Run {
        run_id: r.run_id.clone(),
        agent: r.agent.clone(),
        identity: format!("agent:{}#{}", r.agent, r.run_id),
        task_id: r.task_id.clone().unwrap_or_default(),
        started_at_ms: ms(r.started_at),
        finished_at_ms: r.finished_at.map(ms).unwrap_or(0),
        status: r.status.clone(),
        last_heartbeat_ms: ms(r.last_heartbeat),
        summary: r.summary.clone().unwrap_or_default(),
    }
}

pub fn task(t: &TaskRow, depends_on: Vec<String>, ready: bool) -> v1::Task {
    v1::Task {
        id: t.id.clone(),
        title: t.title.clone(),
        description: t.description.clone(),
        owner: t.owner.clone().unwrap_or_default(),
        state: t.state.clone(),
        scope: t.scope.clone(),
        created_by: t.created_by.clone(),
        created_at_ms: ms(t.created_at),
        updated_at_ms: ms(t.updated_at),
        depends_on,
        ready,
        source_task: t.source_task.clone().unwrap_or_default(),
        source_run: t.source_run.clone().unwrap_or_default(),
        max_attempts: t.max_attempts,
        attempts_started: t.attempts_started,
    }
}

pub fn lease(l: &LeaseRow) -> v1::Lease {
    v1::Lease {
        id: l.id.clone(),
        scope: l.scope.clone(),
        holder: l.holder.clone(),
        exclusive: l.exclusive,
        fencing_token: l.token,
        acquired_at_ms: ms(l.acquired_at),
        expires_at_ms: ms(l.expires_at),
        released_at_ms: l.released_at.map(ms).unwrap_or(0),
        task_id: l.task_id.clone().unwrap_or_default(),
    }
}

pub fn queue_entry(q: &QueueRow) -> v1::QueueEntry {
    v1::QueueEntry {
        id: q.id,
        stream: q.stream.clone(),
        proposal: q.proposal.clone(),
        state: q.state.clone(),
        enqueued_by: q.enqueued_by.clone(),
        enqueued_at_ms: ms(q.enqueued_at),
        processed_at_ms: q.processed_at.map(ms).unwrap_or(0),
        outcome: q.outcome.clone().unwrap_or_default(),
    }
}

pub fn provenance_record(r: &ProvenanceRecord) -> v1::ProvenanceRecord {
    v1::ProvenanceRecord {
        changeset: r.changeset.to_string(),
        agent: r.agent.to_string(),
        run_id: r.run_id.clone().unwrap_or_default(),
        task_id: r.task_id.clone().unwrap_or_default(),
        model: r.model.clone().unwrap_or_default(),
        prompt_digest: r.prompt_digest.clone().unwrap_or_default(),
        parameters: r.parameters.clone(),
        checks_run: r.checks_run.clone(),
        inputs: r.inputs.clone(),
        rationale: r.rationale.clone().unwrap_or_default(),
        created_at_ms: ms(r.created_at),
    }
}

pub fn provenance_entry(row: &ProvenanceRow, record: Option<&ProvenanceRecord>) -> v1::ProvenanceEntry {
    v1::ProvenanceEntry {
        id: row.id,
        record: record.map(provenance_record),
        public_key: row.public_key.clone(),
        signature: row.signature.clone(),
        verified: record.is_some(),
        created_at_ms: ms(row.created_at),
    }
}

pub fn blocker(b: &Blocker) -> v1::Blocker {
    v1::Blocker { code: b.code.clone(), message: b.message.clone(), detail_json: b.detail.to_string() }
}

pub fn strategy(s: Strategy) -> &'static str {
    match s {
        Strategy::FastForward => "fast_forward",
        Strategy::Merge => "merge",
        Strategy::AlreadyIntegrated => "already_integrated",
    }
}

pub fn conflict(c: &ag_integrate::ConflictInfo) -> v1::Conflict {
    v1::Conflict { path: c.path.clone(), kind: c.kind.clone(), message: c.message.clone() }
}

pub fn outcome(o: &Outcome) -> v1::IntegrationOutcome {
    let mut out = v1::IntegrationOutcome::default();
    match o {
        Outcome::Integrated { new_head, previous_head, version, strategy: s, tree_id, changed_files } => {
            out.status = "integrated".into();
            out.new_head = new_head.to_string();
            out.previous_head = id_or_empty(previous_head);
            out.version = *version;
            out.strategy = strategy(*s).into();
            out.tree_id = tree_id.to_string();
            out.changed_files = *changed_files as u32;
        }
        Outcome::Blocked { candidate_changeset, candidate_tree, policy_version, blockers } => {
            out.status = "blocked".into();
            out.candidate_changeset = candidate_changeset.to_string();
            out.candidate_tree = candidate_tree.to_string();
            out.policy_version = *policy_version;
            out.blockers = blockers.iter().map(blocker).collect();
        }
        Outcome::Conflict { conflicts, base } => {
            out.status = "conflict".into();
            out.conflicts = conflicts.iter().map(conflict).collect();
            out.base = id_or_empty(base);
        }
        Outcome::Denied { blockers } => {
            out.status = "denied".into();
            out.blockers = blockers.iter().map(blocker).collect();
        }
        Outcome::Invalid { reason } => {
            out.status = "invalid".into();
            out.reason = reason.clone();
        }
    }
    out
}

pub fn integration_result(r: &IntegrationResult) -> v1::IntegrationResult {
    v1::IntegrationResult {
        operation_id: r.operation_id.clone(),
        state: r.state.to_string(),
        outcome: Some(outcome(&r.outcome)),
    }
}

/// The recorded operation. An integrate operation that has not finished has no outcome yet — the outcome stays
/// unset and `state` says where it is, so a client that asks after a `deadline_exceeded` is never told "invalid"
/// about something still in flight. A stored result that cannot be read is an `internal` error, not an outcome.
pub fn operation(i: &IntentRow) -> ApiResult<v1::Operation> {
    let mut op = v1::Operation {
        operation_id: i.operation_id.clone(),
        kind: i.kind.clone(),
        state: i.state.to_string(),
        actor: i.actor.clone(),
        stream: i.stream.clone().unwrap_or_default(),
        created_at_ms: ms(i.created_at),
        updated_at_ms: ms(i.updated_at),
        ..Default::default()
    };
    match i.kind.as_str() {
        "integrate" => {
            let outcome = i
                .result
                .as_ref()
                .map(|v| {
                    serde_json::from_value::<Outcome>(v.clone()).map_err(|e| {
                        ApiError::internal(format!("operation {} has an unreadable stored result: {e}", i.operation_id))
                    })
                })
                .transpose()?;
            op.integration = Some(v1::IntegrationResult {
                operation_id: i.operation_id.clone(),
                state: i.state.to_string(),
                outcome: outcome.as_ref().map(self::outcome),
            });
        }
        "publish" => {
            let r = i.result.clone().unwrap_or(serde_json::Value::Null);
            op.publication = Some(v1::Publication {
                operation_id: i.operation_id.clone(),
                state: i.state.to_string(),
                head: i.payload["head"].as_str().unwrap_or_default().to_string(),
                new_changesets: r["changesets"].as_u64().unwrap_or(0) as u32,
                new_blobs: r["blobs"].as_u64().unwrap_or(0) as u32,
                reason: r["reason"].as_str().unwrap_or_default().to_string(),
            });
        }
        _ => {}
    }
    Ok(op)
}
