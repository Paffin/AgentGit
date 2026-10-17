//! Read-only preparation of an integration: what would happen, and what is
//! still missing.
//!
//! [`Repository::prepare`] answers the question every client asks before it
//! integrates: *is there an unambiguous candidate, and what does policy say
//! about exactly that candidate right now?* It uses the very same evaluation
//! as the committing transaction, but changes nothing.

use ag_policy::{RunSpec, Unmet};
use ag_types::{Identity, TreeId};
use serde::Serialize;

use crate::integrate::{MergePreview, verdict_in_tx};
use crate::types::Blocker;
use crate::{Error, Repository, Result};

/// Where one required check stands for the current candidate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ValidationState {
    /// Check name.
    pub check: String,
    /// `passed`, `failed` or `missing` (no trusted passing attestation yet).
    pub state: &'static str,
    /// The attestation that decided it (passed/failed).
    pub attestation_id: Option<String>,
    /// How a worker should run it, when the policy says so.
    pub run: Option<RunSpec>,
    /// Environment digest the attestation must carry, if the policy fixes one.
    pub required_environment_digest: Option<String>,
}

/// The result of [`Repository::prepare`].
#[derive(Clone, Debug, Serialize)]
pub struct Preparation {
    /// The proposal.
    pub proposal: String,
    /// The merge preview (strategy, conflicts, candidate tree, changed files).
    pub preview: MergePreview,
    /// Policy version evaluated (0 = no policy stored).
    pub policy_version: u64,
    /// What policy would say right now; empty means it would allow it.
    pub blockers: Vec<Blocker>,
    /// Status of each required check on the candidate tree.
    pub validations: Vec<ValidationState>,
}

impl Repository {
    /// Previews the integration of `proposal_id` as `actor` and evaluates
    /// policy against the exact candidate tree. Nothing is modified.
    pub fn prepare(&self, proposal_id: &str, actor: &Identity, fencing_tokens: &[u64]) -> Result<Preparation> {
        let prop = self.find_proposal(proposal_id)?;
        let preview = self.merge_preview(&prop.id)?;
        let Some(tree_id) = preview.candidate_tree else {
            return Ok(Preparation {
                proposal: prop.id,
                preview,
                policy_version: 0,
                blockers: Vec::new(),
                validations: Vec::new(),
            });
        };
        let author = Identity::new(prop.author.clone()).map_err(|e| Error::Invalid(format!("proposal author: {e}")))?;
        let changed_paths: Vec<ag_policy::ChangedPath> =
            preview.changed_files.iter().map(|p| ag_policy::ChangedPath { path: p.clone() }).collect();
        let (verdict, passing) = self.meta.read(|tx| {
            let v = verdict_in_tx(
                tx,
                &prop,
                actor,
                &author,
                tree_id,
                &changed_paths,
                &preview.changed_files,
                fencing_tokens,
            )?;
            let atts = tx.attestations_for_tree(&tree_id)?;
            Ok((v, atts))
        })?;

        let mut validations = Vec::new();
        for rv in &verdict.policy.required_validations {
            let (state, attestation_id) = match verdict.evaluation.unmet.iter().find_map(|u| match u {
                Unmet::FailedValidation { check, attestation } if check == &rv.check => {
                    Some(("failed", Some(attestation.clone())))
                }
                Unmet::MissingValidation { check } if check == &rv.check => Some(("missing", None)),
                _ => None,
            }) {
                Some(found) => found,
                None => (
                    "passed",
                    passing
                        .iter()
                        .filter(|a| a.check_name == rv.check && a.result == "pass")
                        .max_by_key(|a| a.issued_at)
                        .map(|a| a.id.clone()),
                ),
            };
            validations.push(ValidationState {
                check: rv.check.clone(),
                state,
                attestation_id,
                run: rv.run.clone(),
                required_environment_digest: rv.environment_digest.clone(),
            });
        }
        Ok(Preparation {
            proposal: prop.id,
            preview,
            policy_version: verdict.policy_version,
            blockers: verdict.blockers,
            validations,
        })
    }

    /// The candidate tree id for a proposal, if there is an unambiguous one.
    pub fn candidate_tree_id(&self, proposal_id: &str) -> Result<Option<TreeId>> {
        Ok(self.merge_preview(proposal_id)?.candidate_tree)
    }
}
