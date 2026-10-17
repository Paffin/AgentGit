use ag_policy::Unmet;
use ag_storage::meta::IntentState;
use ag_types::{ChangesetId, TreeId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How a stream was (or would be) advanced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// The proposal already contains the stream head; the head moves to the proposal.
    FastForward,
    /// A new merge changeset with parents `[head, proposal]` becomes the head.
    Merge,
    /// The stream already contains the proposal; nothing changes.
    AlreadyIntegrated,
}

/// One conflicting path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictInfo {
    /// The path.
    pub path: String,
    /// What kind of conflict.
    pub kind: String,
}

/// A reason an integration cannot proceed (yet).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocker {
    /// Machine-readable code, e.g. `missing_validation`.
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
    /// Structured details.
    #[serde(default)]
    pub detail: Value,
}

impl Blocker {
    pub(crate) fn from_unmet(u: &Unmet) -> Self {
        let detail = serde_json::to_value(u).unwrap_or(Value::Null);
        let code = detail.get("kind").and_then(Value::as_str).unwrap_or("policy").to_string();
        Self { code, message: u.to_string(), detail }
    }
}

/// The decided outcome of an integration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    /// The stream was atomically updated.
    Integrated {
        /// The new stream head.
        new_head: ChangesetId,
        /// The head before the update.
        previous_head: Option<ChangesetId>,
        /// Stream version after the update.
        version: u64,
        /// How the head moved.
        strategy: Strategy,
        /// Tree id of the new state.
        tree_id: TreeId,
        /// Files changed relative to the previous head.
        changed_files: usize,
    },
    /// Something is missing; retry after it is provided.
    Blocked {
        /// The candidate changeset (what a validator should check out).
        candidate_changeset: ChangesetId,
        /// Exact tree id of the candidate; attestations must name this.
        candidate_tree: TreeId,
        /// Policy version evaluated.
        policy_version: u64,
        /// What is missing.
        blockers: Vec<Blocker>,
    },
    /// The merge cannot be completed automatically.
    Conflict {
        /// Conflicting paths.
        conflicts: Vec<ConflictInfo>,
        /// The merge base used.
        base: Option<ChangesetId>,
    },
    /// Policy refuses the change as it stands.
    Denied {
        /// Why.
        blockers: Vec<Blocker>,
    },
    /// The request itself is unusable (bad objects, unrelated history, stale proposal).
    Invalid {
        /// Why.
        reason: String,
    },
}

/// What a client gets back (and can ask for again by operation id).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationResult {
    /// Operation identifier.
    pub operation_id: String,
    /// Final or current intent state.
    pub state: IntentState,
    /// The decided outcome.
    pub outcome: Outcome,
}
