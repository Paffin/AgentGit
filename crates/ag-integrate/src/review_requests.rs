//! Explicit review routing, independent of approval and authorization.
use crate::{Error, Repository, Result};
use ag_storage::meta::MetaError;
use ag_types::Identity;
use serde_json::json;
use std::collections::BTreeSet;

fn canonical(identities: &[Identity]) -> Vec<String> {
    identities.iter().map(|identity| identity.principal().to_string()).collect::<BTreeSet<_>>().into_iter().collect()
}

fn owns(owner: &str, actor: &Identity) -> bool {
    match (owner.contains('#'), actor.as_str().contains('#')) {
        (true, true) => owner == actor.as_str(),
        _ => owner.split('#').next() == Some(actor.principal().as_str()),
    }
}

impl Repository {
    /// Requested reviewers of the proposal; this does not grant access or record a review.
    pub fn requested_reviewers(&self, proposal: &str) -> Result<Vec<String>> {
        let proposal = self.find_proposal(proposal)?;
        Ok(self.meta.read(|tx| tx.proposal_reviewers(&proposal.id))?)
    }

    /// The author replaces the inspected reviewer set atomically. Repeating the desired set is a no-op.
    pub fn set_requested_reviewers(
        &self,
        proposal: &str,
        reviewers: &[Identity],
        expected: &[Identity],
        actor: &Identity,
    ) -> Result<Vec<String>> {
        if reviewers.len() > 20 || expected.len() > 20 {
            return Err(Error::Invalid("request at most 20 reviewers".into()));
        }
        let proposal = self.find_proposal(proposal)?.id;
        let (reviewers, expected) = (canonical(reviewers), canonical(expected));
        self.meta.write(|tx| {
            let p = tx.proposal(&proposal)?.ok_or_else(|| MetaError::NotFound(format!("proposal {proposal}")))?;
            if !owns(&p.author, actor) || p.state != "open" {
                return Ok(Err(Error::Invalid("only the author of an open proposal may request reviewers".into())));
            }
            if reviewers.contains(&actor.principal().to_string()) {
                return Ok(Err(Error::Invalid("request a reviewer other than the author".into())));
            }
            let current = tx.proposal_reviewers(&proposal)?;
            if current == reviewers {
                return Ok(Ok(current));
            }
            if current != expected {
                return Ok(Err(Error::Conflict("requested reviewers changed; inspect the current list".into())));
            }
            tx.proposal_set_reviewers(&proposal, &reviewers)?;
            tx.audit_append(actor.as_str(), "proposal.reviewers", &proposal, &json!({"reviewers": reviewers}))?;
            Ok(Ok(reviewers))
        })?
    }
}
