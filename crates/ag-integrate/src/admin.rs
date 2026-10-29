//! Streams, proposals, reviews, policies, attestations and pins.

use ag_crypto::random_id;
use ag_policy::{Attestation, IdentityMatcher, Policy};
use ag_storage::meta::{
    AttestationRow, MetaError, NewProposal, PinRow, PolicyRow, ProposalRow, ReviewRow, StreamRow, Tx,
};
use ag_types::{ChangesetId, Identity, IdentityKind, StreamName, TreeId};
use serde_json::json;

use crate::{Error, Repository, Result};

/// Maximum attestations stored per tree (bounds spam).
const MAX_ATTESTATIONS_PER_TREE: usize = 64;

impl Repository {
    // ---- streams -----------------------------------------------------------

    /// Creates a stream, optionally starting at an existing published changeset.
    pub fn create_stream(&self, name: &StreamName, from: Option<&ChangesetId>, actor: &Identity) -> Result<StreamRow> {
        self.create_stream_capped(name, from, actor, None)
    }

    /// Like [`Repository::create_stream`], with an optional limit on the number of streams. The count and the
    /// insert are one write transaction (which never overlaps another on the same repository, on any backend), so
    /// concurrent creates cannot pass the limit between them: the one that would be over it gets
    /// [`MetaError::QuotaExceeded`] and nothing is written.
    pub fn create_stream_capped(
        &self,
        name: &StreamName,
        from: Option<&ChangesetId>,
        actor: &Identity,
        max_streams: Option<u64>,
    ) -> Result<StreamRow> {
        Ok(self.meta.write(|tx| {
            if let Some(max) = max_streams
                && tx.count_streams()? >= max
            {
                return Err(MetaError::QuotaExceeded(format!("this repository is limited to {max} streams")));
            }
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

    /// Atomically adopts a complete imported branch/tag set into an empty repository.
    /// Objects must already be published. An occupied branch rolls back every ref.
    pub fn adopt_imported_refs(
        &self,
        branches: &[(StreamName, ChangesetId)],
        tags: &[(String, ChangesetId)],
        actor: &Identity,
    ) -> Result<()> {
        self.meta.write(|tx| {
            if tx.list_streams()?.iter().any(|s| s.state.head.is_some()) || !tx.pins()?.is_empty() {
                return Err(MetaError::Invalid("Git migration requires an empty repository".into()));
            }
            for (name, head) in branches {
                if !tx.is_ready(head)? {
                    return Err(MetaError::Invalid(format!("imported head {head} is not published")));
                }
                let row = match tx.stream(name.as_str())? {
                    Some(row) => row,
                    None => tx.create_stream(name, actor.as_str())?,
                };
                if row.state.head.is_some() {
                    return Err(MetaError::Invalid(format!("stream '{name}' already has history")));
                }
                tx.cas_stream(name.as_str(), &row.state, head, None, actor.as_str(), "Git repository migration")?;
            }
            for (name, head) in tags {
                if !tx.is_ready(head)? {
                    return Err(MetaError::Invalid(format!("imported tag {head} is not published")));
                }
                tx.pin_put(name, head, "Imported Git tag")?;
            }
            tx.audit_append(
                actor.as_str(),
                "repository.import",
                "git",
                &json!({"branches":branches.len(), "tags":tags.len()}),
            )?;
            Ok(())
        })?;
        Ok(())
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
        Ok(self.propose_idempotent(stream, head, title, description, author, task_id, None)?.0)
    }

    /// Like [`Repository::propose`], but safe to repeat: with an idempotency
    /// key, the same request returns the proposal it created the first time
    /// (second element `false`), and the same key with a different request is
    /// refused. This is what makes a retried network request harmless.
    #[allow(clippy::too_many_arguments)]
    pub fn propose_idempotent(
        &self,
        stream: &str,
        head: &ChangesetId,
        title: &str,
        description: &str,
        author: &Identity,
        task_id: Option<&str>,
        idempotency_key: Option<&str>,
    ) -> Result<(ProposalRow, bool)> {
        self.propose_capped(stream, head, title, description, author, task_id, idempotency_key, None)
    }

    /// Like [`Repository::propose_idempotent`], with an optional limit on the number of open proposals. The count
    /// and the insert are one write transaction, so concurrent proposals cannot pass the limit between them
    /// ([`MetaError::QuotaExceeded`] for the one that would). A replay of a request that was already served
    /// returns its proposal whatever the count.
    #[allow(clippy::too_many_arguments)]
    pub fn propose_capped(
        &self,
        stream: &str,
        head: &ChangesetId,
        title: &str,
        description: &str,
        author: &Identity,
        task_id: Option<&str>,
        idempotency_key: Option<&str>,
        max_open: Option<u64>,
    ) -> Result<(ProposalRow, bool)> {
        if title.trim().is_empty() {
            return Err(Error::Invalid("a proposal needs a title".into()));
        }
        if title.len() > 300 {
            return Err(Error::Invalid("proposal title is longer than 300 bytes".into()));
        }
        if self.meta.read(|tx| tx.stream(stream))?.is_none() {
            return Err(Error::NotFound(format!("stream '{stream}'")));
        }
        let request_hash = crate::publish::hash_request(&[
            "propose",
            stream,
            &head.to_hex(),
            title,
            description,
            task_id.unwrap_or(""),
            author.as_str(),
        ]);
        if let Some(key) = idempotency_key
            && let Some(existing) = self.meta.read(|tx| tx.intent_by_key(&format!("propose:{key}")))?
        {
            return self.replay_propose(&existing, &request_hash).map(|p| (p, false));
        }
        let published = self.publish(head, author, None)?;
        if published.state != ag_storage::meta::IntentState::Published {
            return Err(Error::Invalid(format!("cannot publish {head}: {}", published.reason.unwrap_or_default())));
        }
        let id = random_id("prop")?;
        let op = random_id("op")?;
        let outcome = self.meta.write(|tx| {
            if let Some(key) = idempotency_key {
                let payload = json!({"stream": stream, "head": head});
                let (intent, created) = tx.intent_create(&ag_storage::meta::NewIntent {
                    operation_id: &op,
                    idempotency_key: &format!("propose:{key}"),
                    request_hash: &request_hash,
                    kind: "propose",
                    actor: author.as_str(),
                    stream: Some(stream),
                    payload: &payload,
                })?;
                if !created {
                    return Ok(Err(intent));
                }
            }
            if let Some(max) = max_open
                && tx.proposals_count("open")? >= max
            {
                return Err(MetaError::QuotaExceeded(format!("this repository is limited to {max} open proposals")));
            }
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
            if idempotency_key.is_some() {
                tx.intent_transition(
                    &op,
                    &[ag_storage::meta::IntentState::Prepared],
                    ag_storage::meta::IntentState::Published,
                    Some(&json!({"proposal": id})),
                    None,
                )?;
            }
            Ok(Ok(p))
        })?;
        match outcome {
            Ok(p) => Ok((p, true)),
            Err(existing) => self.replay_propose(&existing, &request_hash).map(|p| (p, false)),
        }
    }

    fn replay_propose(&self, intent: &ag_storage::meta::IntentRow, request_hash: &str) -> Result<ProposalRow> {
        if intent.request_hash != request_hash {
            return Err(Error::Meta(MetaError::IdempotencyKeyReused(intent.idempotency_key.clone())));
        }
        let id = intent
            .result
            .as_ref()
            .and_then(|r| r["proposal"].as_str())
            .ok_or_else(|| Error::Conflict("the original request is still in progress".into()))?;
        self.meta.read(|tx| tx.proposal(id))?.ok_or_else(|| Error::NotFound(format!("proposal '{id}'")))
    }

    /// Moves an open proposal to a new head. Earlier approvals stay attached
    /// to the head they reviewed and no longer count.
    pub fn update_proposal(&self, id: &str, new_head: &ChangesetId, actor: &Identity) -> Result<ProposalRow> {
        self.update_proposal_at_head(id, new_head, actor, None)
    }

    /// Moves a proposal only while its head matches the caller's inspected revision.
    pub fn update_proposal_at_head(
        &self,
        id: &str,
        new_head: &ChangesetId,
        actor: &Identity,
        expected_head: Option<&ChangesetId>,
    ) -> Result<ProposalRow> {
        let prop = self.find_proposal(id)?;
        let id = prop.id.as_str();
        if prop.author.split('#').next() != Some(actor.principal().as_str()) {
            return Err(Error::Conflict(format!("only the author ({}) may update proposal {id}", prop.author)));
        }
        let expected = expected_head.copied().unwrap_or(prop.head);
        if prop.head != expected {
            return Err(Error::Conflict(format!("proposal head changed: expected {expected}, found {}", prop.head)));
        }
        let published = self.publish(new_head, actor, None)?;
        if published.state != ag_storage::meta::IntentState::Published {
            return Err(Error::Invalid(format!("cannot publish {new_head}: {}", published.reason.unwrap_or_default())));
        }
        self.meta.write(|tx| replace_proposal_head(tx, id, new_head, actor, &expected))?
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
        self.review_at_head(proposal, reviewer, decision, body, None)
    }

    /// Records a review only if the proposal still has the caller's inspected head.
    /// The comparison, review and audit insertion share one metadata write transaction.
    pub fn review_at_head(
        &self,
        proposal: &str,
        reviewer: &Identity,
        decision: &str,
        body: &str,
        expected_head: Option<&ChangesetId>,
    ) -> Result<ReviewRow> {
        if !["approve", "request_changes", "comment"].contains(&decision) {
            return Err(Error::Invalid(format!("unknown decision '{decision}' (approve | request_changes | comment)")));
        }
        let proposal = self.find_proposal(proposal)?.id;
        let proposal = proposal.as_str();
        self.meta.write(|tx| {
            let p = tx.proposal(proposal)?.ok_or_else(|| MetaError::NotFound(format!("proposal '{proposal}'")))?;
            if p.state != "open" {
                return Err(MetaError::Invalid(format!("proposal '{proposal}' is {}", p.state)));
            }
            if let Some(expected) = expected_head
                && p.head != *expected
            {
                return Ok(Err(Error::Conflict(format!(
                    "proposal head changed: expected {expected}, found {}; inspect the updated changes before reviewing",
                    p.head
                ))));
            }
            Ok(Ok(store_review(tx, &p, reviewer, decision, body)?))
        })?
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
    ///
    /// Setting a document that is identical (after canonicalisation) to the current one changes nothing: the
    /// current row is returned, no version is added and nothing is audited. A new version invalidates every
    /// attestation made under the old one, so a retried request after a lost response, or a policy-as-code
    /// loop that re-applies the same file, must not do that.
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
            if let Some(current) = tx.policy_current(stream)?
                && current.doc == canonical
            {
                return Ok(current);
            }
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

/// Keep the review row and its audit event in the caller's metadata transaction.
fn store_review(
    tx: &Tx<'_>,
    proposal: &ProposalRow,
    reviewer: &Identity,
    decision: &str,
    body: &str,
) -> std::result::Result<ReviewRow, MetaError> {
    let id = tx.review_add(&proposal.id, reviewer.as_str(), decision, body, &proposal.head)?;
    tx.audit_append(
        reviewer.as_str(),
        "proposal.review",
        &proposal.id,
        &json!({"decision": decision, "head": proposal.head}),
    )?;
    tx.review(id)?.ok_or_else(|| MetaError::Corrupt("review vanished".into()))
}

fn replace_proposal_head(
    tx: &Tx<'_>,
    id: &str,
    new_head: &ChangesetId,
    actor: &Identity,
    expected: &ChangesetId,
) -> std::result::Result<Result<ProposalRow>, MetaError> {
    let current = tx.proposal(id)?.ok_or_else(|| MetaError::NotFound(format!("proposal '{id}'")))?;
    if current.head != *expected {
        return Ok(Err(Error::Conflict(format!(
            "proposal head changed: expected {expected}, found {}; inspect the updated changes before replacing the head",
            current.head
        ))));
    }
    tx.proposal_set_head(id, new_head)?;
    tx.audit_append(actor.as_str(), "proposal.update", id, &json!({"head": new_head}))?;
    let updated = tx.proposal(id)?.ok_or_else(|| MetaError::Corrupt("proposal vanished".into()))?;
    Ok(Ok(updated))
}
