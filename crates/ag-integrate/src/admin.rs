//! Streams, proposals, reviews, policies, attestations and pins.

use ag_crypto::random_id;
use ag_policy::{Attestation, IdentityMatcher, Policy};
use ag_storage::meta::{AttestationRow, MetaError, NewProposal, PinRow, PolicyRow, ProposalRow, ReviewRow, StreamRow};
use ag_types::{ChangesetId, Identity, IdentityKind, StreamName, TreeId};
use serde_json::json;

use crate::{Error, Repository, Result};

/// Maximum attestations stored per tree (bounds spam).
const MAX_ATTESTATIONS_PER_TREE: usize = 64;

impl Repository {
    // ---- streams -----------------------------------------------------------

    /// Creates a stream, optionally starting at an existing published changeset.
    pub fn create_stream(&self, name: &StreamName, from: Option<&ChangesetId>, actor: &Identity) -> Result<StreamRow> {
        Ok(self.meta.write(|tx| {
            let row = tx.create_stream(name, actor.as_str())?;
            let row = match from {
                Some(h) => {
                    let st = tx.cas_stream(
                        name.as_str(),
                        &row.state,
                        h,
                        None,
                        actor.as_str(),
                        "stream created at a given changeset",
                    )?;
                    StreamRow { state: st, ..row }
                }
                None => row,
            };
            tx.audit_append(actor.as_str(), "stream.create", name.as_str(), &json!({"from": from}))?;
            Ok(row)
        })?)
    }

    /// Points a stream at an already published history (used by importers).
    /// Creates the stream if needed; refuses to move a stream that has a head.
    pub fn adopt_history(
        &self,
        stream: &StreamName,
        head: &ChangesetId,
        actor: &Identity,
        reason: &str,
    ) -> Result<StreamRow> {
        Ok(self.meta.write(|tx| {
            let row = match tx.stream(stream.as_str())? {
                Some(r) => r,
                None => tx.create_stream(stream, actor.as_str())?,
            };
            if row.state.head.is_some() {
                return Err(MetaError::Invalid(format!(
                    "stream '{stream}' already has history; import into a new stream"
                )));
            }
            let st = tx.cas_stream(stream.as_str(), &row.state, head, None, actor.as_str(), reason)?;
            tx.audit_append(actor.as_str(), "stream.adopt", stream.as_str(), &json!({"head": head, "reason": reason}))?;
            Ok(StreamRow { state: st, ..row })
        })?)
    }

    // ---- proposals -----------------------------------------------------------

    /// Resolves a proposal by exact id or by a unique prefix of at least 6
    /// characters (for example `prop_d73c`).
    pub fn find_proposal(&self, id_or_prefix: &str) -> Result<ProposalRow> {
        if let Some(p) = self.meta.read(|tx| tx.proposal(id_or_prefix))? {
            return Ok(p);
        }
        if id_or_prefix.len() < 6 {
            return Err(Error::NotFound(format!("proposal '{id_or_prefix}'")));
        }
        let mut found = self.meta.read(|tx| tx.proposals_by_prefix(id_or_prefix, 3))?;
        match found.len() {
            0 => Err(Error::NotFound(format!("proposal '{id_or_prefix}'"))),
            1 => Ok(found.remove(0)),
            _ => Err(Error::Invalid(format!("'{id_or_prefix}' matches more than one proposal; use more characters"))),
        }
    }

    /// Publishes `head` and opens a proposal to integrate it into `stream`.
    pub fn propose(
        &self,
        stream: &str,
        head: &ChangesetId,
        title: &str,
        description: &str,
        author: &Identity,
        task_id: Option<&str>,
    ) -> Result<ProposalRow> {
        if title.trim().is_empty() {
            return Err(Error::Invalid("a proposal needs a title".into()));
        }
        if title.len() > 300 {
            return Err(Error::Invalid("proposal title is longer than 300 bytes".into()));
        }
        if self.meta.read(|tx| tx.stream(stream))?.is_none() {
            return Err(Error::NotFound(format!("stream '{stream}'")));
        }
        let published = self.publish(head, author, None)?;
        if published.state != ag_storage::meta::IntentState::Published {
            return Err(Error::Invalid(format!("cannot publish {head}: {}", published.reason.unwrap_or_default())));
        }
        let id = random_id("prop")?;
        Ok(self.meta.write(|tx| {
            let p = tx.proposal_create(&NewProposal {
                id: &id,
                stream,
                head,
                title,
                description,
                author: author.as_str(),
                task_id,
            })?;
            tx.audit_append(author.as_str(), "proposal.create", &id, &json!({"stream": stream, "head": head}))?;
            Ok(p)
        })?)
    }

    /// Moves an open proposal to a new head. Earlier approvals stay attached
    /// to the head they reviewed and no longer count.
    pub fn update_proposal(&self, id: &str, new_head: &ChangesetId, actor: &Identity) -> Result<ProposalRow> {
        let prop = self.find_proposal(id)?;
        let id = prop.id.as_str();
        if prop.author.split('#').next() != Some(actor.principal().as_str()) {
            return Err(Error::Conflict(format!("only the author ({}) may update proposal {id}", prop.author)));
        }
        let published = self.publish(new_head, actor, None)?;
        if published.state != ag_storage::meta::IntentState::Published {
            return Err(Error::Invalid(format!("cannot publish {new_head}: {}", published.reason.unwrap_or_default())));
        }
        Ok(self.meta.write(|tx| {
            tx.proposal_set_head(id, new_head)?;
            tx.audit_append(actor.as_str(), "proposal.update", id, &json!({"head": new_head}))?;
            tx.proposal(id)?.ok_or_else(|| MetaError::Corrupt("proposal vanished".into()))
        })?)
    }

    /// Closes an open proposal without integrating it (author or admin).
    pub fn close_proposal(&self, id: &str, actor: &Identity) -> Result<()> {
        let prop = self.find_proposal(id)?;
        let id = prop.id.as_str();
        if prop.state != "open" {
            return Err(Error::Conflict(format!("proposal '{id}' is already {}", prop.state)));
        }
        let is_author = prop.author.split('#').next() == Some(actor.principal().as_str());
        if !is_author && !self.is_admin(&prop.stream, actor)? {
            return Err(Error::Conflict("only the author or an admin may close a proposal".into()));
        }
        self.meta.write(|tx| {
            tx.proposal_set_state(id, "closed", None)?;
            tx.audit_append(actor.as_str(), "proposal.close", id, &json!({}))?;
            Ok(())
        })?;
        Ok(())
    }

    /// Records a review of the proposal's **current head**.
    pub fn review(&self, proposal: &str, reviewer: &Identity, decision: &str, body: &str) -> Result<ReviewRow> {
        if !["approve", "request_changes", "comment"].contains(&decision) {
            return Err(Error::Invalid(format!("unknown decision '{decision}' (approve | request_changes | comment)")));
        }
        let proposal = self.find_proposal(proposal)?.id;
        let proposal = proposal.as_str();
        Ok(self.meta.write(|tx| {
            let p = tx.proposal(proposal)?.ok_or_else(|| MetaError::NotFound(format!("proposal '{proposal}'")))?;
            if p.state != "open" {
                return Err(MetaError::Invalid(format!("proposal '{proposal}' is {}", p.state)));
            }
            let id = tx.review_add(proposal, reviewer.as_str(), decision, body, &p.head)?;
            tx.audit_append(
                reviewer.as_str(),
                "proposal.review",
                proposal,
                &json!({"decision": decision, "head": p.head}),
            )?;
            tx.reviews(proposal)?
                .into_iter()
                .find(|r| r.id == id)
                .ok_or_else(|| MetaError::Corrupt("review vanished".into()))
        })?)
    }

    // ---- policy --------------------------------------------------------------

    /// The policy in force for `stream` and its version (0 = built-in default).
    pub fn policy(&self, stream: &str) -> Result<(Policy, u64)> {
        Ok(match self.meta.read(|tx| tx.policy_current(stream))? {
            Some(row) => (Policy::parse(&row.doc)?, row.version),
            None => (Policy::default(), 0),
        })
    }

    /// All stored versions of a stream's policy.
    pub fn policy_history(&self, stream: &str) -> Result<Vec<PolicyRow>> {
        Ok(self.meta.read(|tx| tx.policy_history(stream))?)
    }

    fn is_admin(&self, stream: &str, actor: &Identity) -> Result<bool> {
        let (policy, _) = self.policy(stream)?;
        let admins = IdentityMatcher::new(&policy.admins).map_err(Error::Invalid)?;
        Ok(if admins.is_empty() { actor.kind() == IdentityKind::User } else { admins.is_match(actor) })
    }

    /// Stores a new policy version for a stream. Requires an admin of the
    /// *current* policy (any user while no admins are configured).
    pub fn set_policy(&self, stream: &str, json_doc: &str, actor: &Identity) -> Result<PolicyRow> {
        if self.meta.read(|tx| tx.stream(stream))?.is_none() {
            return Err(Error::NotFound(format!("stream '{stream}'")));
        }
        if !self.is_admin(stream, actor)? {
            return Err(Error::Conflict(format!("{actor} is not allowed to change the policy of '{stream}'")));
        }
        let policy = Policy::parse(json_doc)?;
        let canonical = policy.to_json();
        Ok(self.meta.write(|tx| {
            let row = tx.policy_put(stream, &canonical, actor.as_str())?;
            tx.audit_append(
                actor.as_str(),
                "policy.set",
                stream,
                &json!({"version": row.version, "hash": row.doc_hash}),
            )?;
            Ok(row)
        })?)
    }

    // ---- attestations ---------------------------------------------------------

    /// Stores a signed attestation after verifying its signature. Whether it
    /// *counts* is decided by the policy's `trusted_issuers` at integration time.
    pub fn submit_attestation(&self, att: &Attestation) -> Result<()> {
        att.verify_signature().map_err(|e| Error::Invalid(e.to_string()))?;
        let row = AttestationRow {
            id: att.id.clone(),
            tree_id: att.tree_id,
            check_name: att.check.clone(),
            result: att.result.as_str().into(),
            policy_version: att.policy_version,
            environment_digest: att.environment_digest.clone(),
            issuer: att.issuer.to_string(),
            issued_at: att.issued_at,
            public_key: att.public_key.to_hex(),
            signature: att.signature.to_hex(),
            payload: serde_json::to_value(att)?,
        };
        self.meta.write(|tx| {
            if tx.attestations_for_tree(&att.tree_id)?.len() >= MAX_ATTESTATIONS_PER_TREE {
                return Err(MetaError::Invalid(format!(
                    "tree {} already has {MAX_ATTESTATIONS_PER_TREE} attestations",
                    att.tree_id
                )));
            }
            tx.attestation_insert(&row)?;
            tx.audit_append(
                att.issuer.as_str(),
                "attestation.submit",
                &att.tree_id.to_hex(),
                &json!({"check": att.check, "result": att.result.as_str(), "id": att.id}),
            )?;
            Ok(())
        })?;
        Ok(())
    }

    /// Attestations recorded for an exact tree.
    pub fn attestations(&self, tree: &TreeId) -> Result<Vec<AttestationRow>> {
        Ok(self.meta.read(|tx| tx.attestations_for_tree(tree))?)
    }

    // ---- pins ----------------------------------------------------------------

    /// Pins a changeset as a retention root (keeps it from garbage collection).
    pub fn pin(&self, name: &str, id: &ChangesetId, reason: &str, actor: &Identity) -> Result<()> {
        if !self.objects.contains(&id.object_id())? {
            return Err(Error::NotFound(format!("changeset {id}")));
        }
        self.meta.write(|tx| {
            tx.pin_put(name, id, reason)?;
            tx.audit_append(actor.as_str(), "pin.set", name, &json!({"changeset": id}))?;
            Ok(())
        })?;
        Ok(())
    }

    /// Removes a pin.
    pub fn unpin(&self, name: &str, actor: &Identity) -> Result<bool> {
        Ok(self.meta.write(|tx| {
            let removed = tx.pin_delete(name)?;
            if removed {
                tx.audit_append(actor.as_str(), "pin.remove", name, &json!({}))?;
            }
            Ok(removed)
        })?)
    }

    /// All pins.
    pub fn pins(&self) -> Result<Vec<PinRow>> {
        Ok(self.meta.read(|tx| tx.pins())?)
    }
}
