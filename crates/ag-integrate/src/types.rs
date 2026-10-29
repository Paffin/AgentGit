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

/// Source locations for a conflict whose versions moved to different paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictOriginInfo {
    /// Path in the common merge base.
    pub base: String,
    /// Path in the first parent (workspace for sync, target stream for integration).
    pub ours: String,
    /// Path in the second parent (stream for sync, proposal for integration).
    pub theirs: String,
}

/// Bounded complete candidate paths for one explicit structural choice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionScopeInfo {
    /// Exact candidate entry paths at or beneath the conflicting root, in path order.
    pub paths: Vec<String>,
    /// False when either preview bound omitted any candidate path.
    pub complete: bool,
}

impl From<ag_merge::ResolutionScope> for ResolutionScopeInfo {
    fn from(scope: ag_merge::ResolutionScope) -> Self {
        Self { paths: scope.paths.into_iter().map(|path| path.to_string()).collect(), complete: scope.complete }
    }
}

/// One conflicting path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictInfo {
    /// The path.
    pub path: String,
    /// The kind of conflict as a stable code (`content`, `both_added`, `delete_modify`, `binary`, `type_change`,
    /// `too_large`, `file_directory`, `rename_rename`, `rename_delete`,
    /// `directory_rename_ambiguous`, `directory_rename_collision`): see `ag_merge::ConflictKind::code`.
    pub kind: String,
    /// The same, in words for a person (`content conflict (2 region(s))`). Outcomes stored before this field
    /// existed have none.
    #[serde(default)]
    pub message: String,
    /// Exact per-version paths when a rename moved the source; absent for ordinary conflicts and legacy outcomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<ConflictOriginInfo>,
    /// Present only for file/directory conflicts in a fresh virtual sync preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_scope: Option<ResolutionScopeInfo>,
}

impl ConflictInfo {
    /// The wire description of a merge conflict.
    pub fn from_conflict(c: &ag_merge::Conflict) -> Self {
        ConflictInfo {
            path: c.path.to_string(),
            kind: c.kind.code().to_string(),
            message: c.kind.to_string(),
            resolution_scope: None,
            origin: c.origin.as_ref().map(|o| ConflictOriginInfo {
                base: o.base.to_string(),
                ours: o.ours.to_string(),
                theirs: o.theirs.to_string(),
            }),
        }
    }

    /// What to show a person: the message, or the code for an outcome stored before messages existed.
    pub fn describe(&self) -> &str {
        if self.message.is_empty() { &self.kind } else { &self.message }
    }
}

/// A note of the merge: something it decided on its own and a person may want to check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteInfo {
    /// A stable code (`directory_rename`): see `ag_merge::MergeNote::code`.
    pub code: String,
    /// The note in words for a person.
    pub message: String,
    /// The paths involved, as `[from, to]` pairs (files moved along by a directory rename), possibly cut.
    #[serde(default)]
    pub moves: Vec<[String; 2]>,
}

impl NoteInfo {
    /// The wire description of a merge note.
    pub fn from_note(n: &ag_merge::MergeNote) -> Self {
        let moves = match n {
            ag_merge::MergeNote::DirectoryRename { relocated, .. } => {
                relocated.iter().map(|(a, b)| [a.to_string(), b.to_string()]).collect()
            }
        };
        NoteInfo { code: n.code().to_string(), message: n.to_string(), moves }
    }
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
        Self { code: u.code().to_string(), message: u.to_string(), detail }
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

impl Outcome {
    /// The outcome's name on the wire and in metrics: `integrated`, `blocked`, `conflict`, `denied` or `invalid`.
    pub fn status(&self) -> &'static str {
        match self {
            Outcome::Integrated { .. } => "integrated",
            Outcome::Blocked { .. } => "blocked",
            Outcome::Conflict { .. } => "conflict",
            Outcome::Denied { .. } => "denied",
            Outcome::Invalid { .. } => "invalid",
        }
    }
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
