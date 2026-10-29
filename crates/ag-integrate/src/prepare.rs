//! Read-only preparation of an integration: what would happen, and what is
//! still missing.
//!
//! [`Repository::prepare`] answers the question every client asks before it
//! integrates: *is there an unambiguous candidate, and what does policy say
//! about exactly that candidate right now?* It uses the very same evaluation
//! as the committing transaction, but changes nothing.

use ag_policy::{RequiredValidation, RunSpec, Unmet};
use ag_storage::meta::{MetaError, Tx};
use ag_types::{Identity, TreeId};
use serde::Serialize;

use crate::integrate::{MergePreview, discussion_gate_in_tx, verdict_in_tx};
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

/// What [`Repository::prepare_with`] knows when it lets the caller read more in its transaction.
#[derive(Debug)]
pub struct PrepareContext<'a> {
    /// The proposal id.
    pub proposal: &'a str,
    /// The candidate tree (`None`: no unambiguous candidate; only the discussion gate was evaluated).
    pub tree: Option<TreeId>,
    /// The policy version evaluated (0 = no policy stored).
    pub policy_version: u64,
    /// The checks the policy requires of the candidate.
    pub required: &'a [RequiredValidation],
}

impl Repository {
    /// Previews the integration of `proposal_id` as `actor` and evaluates
    /// policy against the exact candidate tree. Nothing is modified.
    pub fn prepare(&self, proposal_id: &str, actor: &Identity, fencing_tokens: &[u64]) -> Result<Preparation> {
        self.prepare_with(proposal_id, actor, fencing_tokens, |_, _| Ok(())).map(|(prep, ())| prep)
    }

    /// Like [`Repository::prepare`], and runs `observe` in the very read transaction that evaluates the policy and
    /// reads the attestations. What `observe` reads - the jobs that validate the candidate, say - is therefore
    /// from the same snapshot as the verdict: a job that completes at that moment is either seen with its
    /// attestation or seen as still running without it, never as finished while the attestation is missing.
    /// Without a candidate, only the discussion gate is evaluated; `observe` shares that read snapshot.
    pub fn prepare_with<R>(
        &self,
        proposal_id: &str,
        actor: &Identity,
        fencing_tokens: &[u64],
        observe: impl FnOnce(&Tx<'_>, &PrepareContext<'_>) -> std::result::Result<R, MetaError>,
    ) -> Result<(Preparation, R)> {
        let prop = self.find_proposal(proposal_id)?;
        let preview = self.merge_preview(&prop.id)?;
        let Some(tree_id) = preview.candidate_tree else {
            let (policy_version, blockers, seen) = self.meta.read(|tx| {
                let (policy_version, blocker) = discussion_gate_in_tx(tx, &prop)?;
                let seen =
                    observe(tx, &PrepareContext { proposal: &prop.id, tree: None, policy_version, required: &[] })?;
                Ok((policy_version, blocker.into_iter().collect(), seen))
            })?;
            return Ok((
                Preparation { proposal: prop.id, preview, policy_version, blockers, validations: Vec::new() },
                seen,
            ));
        };
        let author = Identity::new(prop.author.clone()).map_err(|e| Error::Invalid(format!("proposal author: {e}")))?;
        let changed_paths: Vec<ag_policy::ChangedPath> =
            preview.changed_files.iter().map(|p| ag_policy::ChangedPath { path: p.clone() }).collect();
        let (verdict, passing, seen) = self.meta.read(|tx| {
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
            // Tests hold the transaction open here to let a validation finish at exactly this moment.
            self.failpoints.pause("prepare.in_snapshot");
            let seen = observe(
                tx,
                &PrepareContext {
                    proposal: &prop.id,
                    tree: Some(tree_id),
                    policy_version: v.policy_version,
                    required: &v.policy.required_validations,
                },
            )?;
            Ok((v, atts, seen))
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
        Ok((
            Preparation {
                proposal: prop.id,
                preview,
                policy_version: verdict.policy_version,
                blockers: verdict.blockers,
                validations,
            },
            seen,
        ))
    }

    /// The candidate tree id for a proposal, if there is an unambiguous one.
    pub fn candidate_tree_id(&self, proposal_id: &str) -> Result<Option<TreeId>> {
        Ok(self.merge_preview(proposal_id)?.candidate_tree)
    }
}
