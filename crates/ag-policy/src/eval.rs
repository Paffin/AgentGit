use std::collections::BTreeMap;

use ag_types::{ChangesetId, Identity, IdentityKind, Timestamp, TreeId};
use serde::Serialize;

use crate::attestation::{Attestation, CheckResult};
use crate::document::{Policy, RequiredValidation};
use crate::matcher::{IdentityMatcher, PathMatcher};

/// Maximum accepted clock skew for attestation timestamps.
const MAX_FUTURE_SKEW_MS: i64 = 5 * 60 * 1000;

/// A path changed by the candidate integration (relative to the stream head).
#[derive(Clone, Debug)]
pub struct ChangedPath {
    /// The repository path.
    pub path: String,
}

/// A review decision about a proposal head.
#[derive(Clone, Debug)]
pub struct ApprovalFact {
    /// Reviewer.
    pub reviewer: Identity,
    /// The head the reviewer looked at.
    pub head: ChangesetId,
    /// `true` for approve, `false` for request-changes.
    pub approve: bool,
}

/// Everything the evaluation depends on. Gathered by the caller (inside the
/// integration transaction) so the verdict and the commit see the same state.
#[derive(Clone, Debug)]
pub struct Facts<'a> {
    /// Target stream.
    pub stream: &'a str,
    /// Identity performing the integration.
    pub actor: &'a Identity,
    /// Author of the proposal.
    pub author: &'a Identity,
    /// Current head of the proposal.
    pub proposal_head: ChangesetId,
    /// Tree id of the exact candidate result.
    pub candidate_tree: TreeId,
    /// Version of the policy being evaluated.
    pub policy_version: u64,
    /// Paths the integration changes relative to the current stream head.
    pub changed: &'a [ChangedPath],
    /// Attestations known for the candidate tree (signatures are verified here).
    pub attestations: &'a [Attestation],
    /// Review decisions in chronological order.
    pub approvals: &'a [ApprovalFact],
    /// Whether signed provenance is attached to the proposal head.
    pub has_provenance: bool,
    /// Current time.
    pub now: Timestamp,
}

/// A requirement that is not (yet) satisfied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Unmet {
    /// The acting identity may not integrate into this stream.
    ActorNotAllowed {
        /// Why.
        reason: String,
    },
    /// The author may not propose to this stream.
    AuthorNotAllowed {
        /// The author.
        author: String,
    },
    /// A trusted attestation says the check failed on this exact tree.
    FailedValidation {
        /// Check name.
        check: String,
        /// Failing attestation.
        attestation: String,
    },
    /// No trusted passing attestation exists for this exact tree.
    MissingValidation {
        /// Check name.
        check: String,
    },
    /// Not enough eligible approvals of the current head.
    InsufficientApprovals {
        /// `default` or the protected glob.
        scope: String,
        /// Needed.
        required: u32,
        /// Present.
        have: u32,
    },
    /// An eligible reviewer asked for changes on the current head.
    ChangesRequested {
        /// The reviewer.
        reviewer: String,
    },
    /// Agent-authored change without signed provenance.
    MissingProvenance,
    /// The integration changes more files than the policy allows.
    TooManyChangedFiles {
        /// Files changed.
        changed: usize,
        /// Limit.
        max: u32,
    },
}

impl Unmet {
    /// Denials cannot be fixed by waiting; everything else can.
    pub fn is_denial(&self) -> bool {
        matches!(
            self,
            Unmet::ActorNotAllowed { .. }
                | Unmet::AuthorNotAllowed { .. }
                | Unmet::FailedValidation { .. }
                | Unmet::TooManyChangedFiles { .. }
        )
    }
}

impl std::fmt::Display for Unmet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unmet::ActorNotAllowed { reason } => write!(f, "actor not allowed to integrate: {reason}"),
            Unmet::AuthorNotAllowed { author } => write!(f, "author {author} is not allowed to propose to this stream"),
            Unmet::FailedValidation { check, attestation } => write!(f, "check '{check}' FAILED on this exact tree (attestation {attestation})"),
            Unmet::MissingValidation { check } => write!(f, "check '{check}' has no trusted passing attestation for this exact tree"),
            Unmet::InsufficientApprovals { scope, required, have } => write!(f, "approvals ({scope}): {have} of {required} required"),
            Unmet::ChangesRequested { reviewer } => write!(f, "{reviewer} requested changes"),
            Unmet::MissingProvenance => f.write_str("agent-authored change has no signed provenance"),
            Unmet::TooManyChangedFiles { changed, max } => write!(f, "{changed} files changed, policy allows at most {max}"),
        }
    }
}

/// Overall verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// All requirements are met.
    Allow,
    /// Something is missing; the integration can be retried later.
    Blocked,
    /// The change is refused as it stands.
    Denied,
}

/// Result of [`evaluate`].
#[derive(Clone, Debug, Serialize)]
pub struct Evaluation {
    /// The verdict.
    pub decision: Decision,
    /// Unmet requirements (empty when allowed).
    pub unmet: Vec<Unmet>,
    /// Human-readable list of requirements that were satisfied.
    pub satisfied: Vec<String>,
}

impl Evaluation {
    /// True if the change may be integrated.
    pub fn is_allowed(&self) -> bool {
        self.decision == Decision::Allow
    }
}

/// Evaluates `policy` against `facts`. Pure and deterministic.
///
/// The policy is assumed valid (see [`Policy::validate`]); matcher
/// compilation failures are treated as "nothing matches".
pub fn evaluate(policy: &Policy, facts: &Facts<'_>) -> Evaluation {
    let mut unmet: Vec<Unmet> = Vec::new();
    let mut satisfied: Vec<String> = Vec::new();

    let integrators = IdentityMatcher::new(&policy.integrators).expect("validated policy");
    let allowed_authors = IdentityMatcher::new(&policy.allowed_authors).expect("validated policy");
    let approvers = IdentityMatcher::new(&policy.approvals.approvers).expect("validated policy");
    let autonomy_paths = PathMatcher::new(&policy.agent_autonomy.allowed_paths).expect("validated policy");

    let protected: Vec<(&crate::document::ProtectedPath, PathMatcher, IdentityMatcher)> = policy
        .protected_paths
        .iter()
        .map(|p| {
            (
                p,
                PathMatcher::new(std::slice::from_ref(&p.glob)).expect("validated policy"),
                IdentityMatcher::new(&p.approvers).expect("validated policy"),
            )
        })
        .collect();

    let touches_protected = facts.changed.iter().any(|c| protected.iter().any(|(_, m, _)| m.is_match(&c.path)));
    let within_autonomy = policy.agent_autonomy.max_files > 0
        && facts.changed.len() <= policy.agent_autonomy.max_files as usize
        && !autonomy_paths.is_empty()
        && facts.changed.iter().all(|c| autonomy_paths.is_match(&c.path))
        && !touches_protected;

    // --- who may do this -------------------------------------------------
    let actor_is_agent = facts.actor.kind() == IdentityKind::Agent;
    let listed = if integrators.is_empty() { !actor_is_agent } else { integrators.is_match(facts.actor) };
    let autonomous_agent = actor_is_agent && policy.agent_autonomy.auto_integrate && within_autonomy;
    if listed || autonomous_agent {
        satisfied.push(format!("actor {} may integrate", facts.actor));
    } else {
        let reason = if actor_is_agent {
            "agents may not integrate unless agent_autonomy.auto_integrate is on and the change is within its limits".to_string()
        } else {
            format!("{} is not in the policy's integrators", facts.actor)
        };
        unmet.push(Unmet::ActorNotAllowed { reason });
    }
    if !allowed_authors.is_empty() && !allowed_authors.is_match(facts.author) {
        unmet.push(Unmet::AuthorNotAllowed { author: facts.author.to_string() });
    }

    // --- change size -----------------------------------------------------
    if policy.max_changed_files > 0 && facts.changed.len() > policy.max_changed_files as usize {
        unmet.push(Unmet::TooManyChangedFiles { changed: facts.changed.len(), max: policy.max_changed_files });
    }

    // --- validations -------------------------------------------------------
    for v in &policy.required_validations {
        if let Some(bad) = facts.attestations.iter().find(|a| {
            a.result == CheckResult::Fail && a.check == v.check && trusted(policy, a, facts) && a.verify_signature().is_ok()
        }) {
            unmet.push(Unmet::FailedValidation { check: v.check.clone(), attestation: bad.id.clone() });
        } else if facts.attestations.iter().any(|a| passes(policy, v, a, facts)) {
            satisfied.push(format!("check '{}' passed on tree {}", v.check, facts.candidate_tree.short()));
        } else {
            unmet.push(Unmet::MissingValidation { check: v.check.clone() });
        }
    }

    // --- approvals ---------------------------------------------------------
    let author_is_agent = facts.author.kind() == IdentityKind::Agent;
    let agent_needs_humans = author_is_agent && !within_autonomy;
    let global_required = if agent_needs_humans {
        policy.approvals.min.max(policy.approvals.min_for_agents)
    } else {
        policy.approvals.min
    };
    let latest = latest_decisions(facts);
    let eligible = |id: &Identity, extra: &IdentityMatcher, require_human: bool| -> bool {
        if require_human && id.kind() != IdentityKind::User {
            return false;
        }
        if policy.approvals.forbid_self_approval && id.principal() == facts.author.principal() {
            return false;
        }
        if !extra.is_empty() {
            extra.is_match(id)
        } else if approvers.is_empty() {
            id.kind() == IdentityKind::User
        } else {
            approvers.is_match(id)
        }
    };
    let count = |extra: &IdentityMatcher, require_human: bool| -> u32 {
        latest.iter().filter(|(_, (id, ok))| *ok && eligible(id, extra, require_human)).count() as u32
    };
    let none = IdentityMatcher::new(&[]).expect("empty matcher");

    if global_required > 0 {
        let have = count(&none, agent_needs_humans);
        if have >= global_required {
            satisfied.push(format!("{have} approval(s) of {global_required} required"));
        } else {
            unmet.push(Unmet::InsufficientApprovals { scope: "default".into(), required: global_required, have });
        }
    }
    for (rule, paths, ids) in &protected {
        if facts.changed.iter().any(|c| paths.is_match(&c.path)) {
            let have = count(ids, false);
            if have >= rule.min_approvals {
                satisfied.push(format!("protected '{}': {have} approval(s) of {}", rule.glob, rule.min_approvals));
            } else {
                unmet.push(Unmet::InsufficientApprovals { scope: rule.glob.clone(), required: rule.min_approvals, have });
            }
        }
    }
    for (_, (id, ok)) in &latest {
        if !*ok && eligible(id, &none, false) {
            unmet.push(Unmet::ChangesRequested { reviewer: id.to_string() });
        }
    }

    // --- provenance ----------------------------------------------------------
    if author_is_agent && policy.require_provenance_for_agents {
        if facts.has_provenance {
            satisfied.push("signed provenance attached".into());
        } else {
            unmet.push(Unmet::MissingProvenance);
        }
    }

    let decision = if unmet.iter().any(Unmet::is_denial) {
        Decision::Denied
    } else if unmet.is_empty() {
        Decision::Allow
    } else {
        Decision::Blocked
    };
    Evaluation { decision, unmet, satisfied }
}

/// Latest review decision per reviewer principal, for the current head only.
/// Earlier reviews of other heads are stale and ignored.
fn latest_decisions(facts: &Facts<'_>) -> BTreeMap<Identity, (Identity, bool)> {
    let mut m = BTreeMap::new();
    for a in facts.approvals.iter().filter(|a| a.head == facts.proposal_head) {
        m.insert(a.reviewer.principal(), (a.reviewer.clone(), a.approve));
    }
    m
}

/// Issuer identity and key match a `trusted_issuers` entry, and the
/// attestation is about this tree and policy version.
fn trusted(policy: &Policy, a: &Attestation, facts: &Facts<'_>) -> bool {
    a.tree_id == facts.candidate_tree
        && a.policy_version == facts.policy_version
        && policy.trusted_issuers.iter().any(|t| t.identity == a.issuer && t.public_key == a.public_key)
}

fn passes(policy: &Policy, v: &RequiredValidation, a: &Attestation, facts: &Facts<'_>) -> bool {
    if a.result != CheckResult::Pass || a.check != v.check || !trusted(policy, a, facts) {
        return false;
    }
    if !v.issuers.is_empty() && !v.issuers.contains(&a.issuer) {
        return false;
    }
    if let Some(d) = &v.environment_digest {
        if d != &a.environment_digest {
            return false;
        }
    }
    let age_ms = facts.now.as_millis() - a.issued_at.as_millis();
    if age_ms < -MAX_FUTURE_SKEW_MS {
        return false;
    }
    if let Some(max) = v.max_age_secs {
        if age_ms > (max as i64).saturating_mul(1000) {
            return false;
        }
    }
    a.verify_signature().is_ok()
}

#[cfg(test)]
#[path = "eval_tests.rs"]
mod tests;
