//! The integration queue: competing proposals are integrated one at a time,
//! in order, each against the head left by the previous one.

use ag_storage::meta::{IntentState, QueueRow};
use ag_types::Identity;
use serde::Serialize;

use crate::integrate::IntegrateRequest;
use crate::types::Outcome;
use crate::{Error, Repository, Result};

/// What a queue run did.
#[derive(Clone, Debug, Default, Serialize)]
pub struct QueueRunReport {
    /// Entries integrated, in order.
    pub integrated: Vec<String>,
    /// Entries that are waiting for approvals/validation (left `blocked`).
    pub blocked: Vec<String>,
    /// Entries that conflict or were denied (`rejected`).
    pub rejected: Vec<String>,
    /// Entries that errored.
    pub failed: Vec<(String, String)>,
    /// The [`Outcome::status`] of every entry that was decided, in order (errored entries have none). For
    /// in-process callers (metrics); not part of the JSON the CLI prints.
    #[serde(skip)]
    pub outcomes: Vec<&'static str>,
}

impl Repository {
    /// Appends an open proposal to its stream's integration queue.
    pub fn enqueue(&self, proposal: &str, actor: &Identity) -> Result<QueueRow> {
        let prop = self.find_proposal(proposal)?;
        let proposal = prop.id.as_str();
        if prop.state != "open" {
            return Err(Error::Conflict(format!("proposal '{proposal}' is {}", prop.state)));
        }
        Ok(self.meta.write(|tx| {
            let row = tx.queue_push(&prop.stream, proposal, actor.as_str())?;
            tx.audit_append(actor.as_str(), "queue.enqueue", proposal, &serde_json::json!({"stream": prop.stream}))?;
            Ok(row)
        })?)
    }

    /// Entries of a stream's queue.
    pub fn queue(&self, stream: &str) -> Result<Vec<QueueRow>> {
        Ok(self.meta.read(|tx| tx.queue_list(stream, None))?)
    }

    /// Processes all `queued` entries of `stream` in order. A blocked or
    /// conflicting entry does not hold up the ones behind it; it is marked and
    /// can be enqueued again once fixed.
    ///
    /// The queue carries no fencing tokens: a fencing token proves that the *lease holder* is the one
    /// integrating (`ag integrate <proposal> --fencing-tokens N`), and whoever runs the queue is not
    /// that party. A proposal that touches a path under somebody's exclusive lease is therefore left
    /// `blocked` with the lease named as the blocker; integrate it directly with the token, or wait for
    /// the lease to end, and enqueue it again. (Storing tokens with the entry would need a schema and a
    /// protocol change, which nothing asks for yet.)
    pub fn run_queue(&self, stream: &str, actor: &Identity) -> Result<QueueRunReport> {
        let mut report = QueueRunReport::default();
        let entries = self.meta.read(|tx| tx.queue_list(stream, Some("queued")))?;
        for entry in entries {
            let result = self.integrate(&IntegrateRequest {
                proposal_id: entry.proposal.clone(),
                actor: actor.clone(),
                idempotency_key: None,
                fencing_tokens: vec![],
            });
            if let Ok(r) = &result {
                report.outcomes.push(r.outcome.status());
            }
            let (state, text) = match result {
                Ok(r) => match (&r.state, &r.outcome) {
                    (IntentState::Integrated, _) => {
                        report.integrated.push(entry.proposal.clone());
                        ("done", "integrated".to_string())
                    }
                    (IntentState::Blocked, Outcome::Blocked { blockers, .. }) => {
                        report.blocked.push(entry.proposal.clone());
                        ("blocked", blockers.iter().map(|b| b.message.clone()).collect::<Vec<_>>().join("; "))
                    }
                    (_, Outcome::Conflict { conflicts, .. }) => {
                        report.rejected.push(entry.proposal.clone());
                        ("rejected", format!("{} conflicting path(s)", conflicts.len()))
                    }
                    (_, other) => {
                        report.rejected.push(entry.proposal.clone());
                        ("rejected", format!("{other:?}"))
                    }
                },
                Err(e) => {
                    report.failed.push((entry.proposal.clone(), e.to_string()));
                    ("failed", e.to_string())
                }
            };
            self.meta.write(|tx| tx.queue_finish(entry.id, state, &text))?;
        }
        Ok(report)
    }
}
