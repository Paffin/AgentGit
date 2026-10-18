//! Safe publication: making a head and its history visible to readers.
//!
//! ```text
//! Prepare  intent recorded, idempotency key reserved
//! Upload   closure of unpublished changesets/blobs enumerated, presence checked,
//!          objects registered with the intent (protects them from GC)
//! Verify   every blob and changeset re-read and hashed; shapes, namespace and
//!          limits checked; every changeset's tree derived to prove the history
//!          is structurally valid
//! Ready    ONE metadata transaction registers the changesets as published
//! ```
//!
//! Objects are durable and verified **before** the transaction that makes
//! them reachable, because the object store and the metadata store cannot
//! share a transaction. Each step is idempotent, so an interrupted
//! publication resumes from its recorded state.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use ag_changeset::{Changeset, Op};
use ag_crypto::random_id;
use ag_object::{ObjectKind, canon::Writer};
use ag_storage::meta::{IntentRow, IntentState, NewIntent};
use ag_tree::Tree;
use ag_types::{BlobId, ChangesetId, Identity, ObjectId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::derive::DERIVE_BATCH_AFTER_PUBLISH;
use crate::{Error, Repository, Result};

/// Outcome of a publication.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishResult {
    /// Operation id (ask for it again after a lost response).
    pub operation_id: String,
    /// `published` on success, `rejected` if the objects are unusable.
    pub state: IntentState,
    /// The published head.
    pub head: ChangesetId,
    /// Changesets that became visible with this publication.
    pub new_changesets: usize,
    /// Blobs verified for this publication.
    pub new_blobs: usize,
    /// Why the publication was rejected.
    pub reason: Option<String>,
}

pub(crate) fn hash_request(parts: &[&str]) -> String {
    let mut w = Writer::new();
    for p in parts {
        w.str(p);
    }
    hex::encode(Sha256::digest(w.finish()))
}

struct Closure {
    order: Vec<(ChangesetId, Arc<Changeset>)>,
    blobs: BTreeSet<BlobId>,
}

impl Repository {
    /// Publishes `head` and all of its not-yet-published ancestors.
    ///
    /// Idempotent: the default idempotency key is derived from the head, so
    /// repeating the call returns the recorded result.
    pub fn publish(
        &self,
        head: &ChangesetId,
        actor: &Identity,
        idempotency_key: Option<&str>,
    ) -> Result<PublishResult> {
        let key = idempotency_key.map(str::to_string).unwrap_or_else(|| format!("publish:{head}"));
        let request_hash = hash_request(&["publish", &head.to_hex()]);
        let op = random_id("op")?;
        let payload = json!({ "head": head });
        let (intent, _) = self.meta.write(|tx| {
            tx.intent_create(&NewIntent {
                operation_id: &op,
                idempotency_key: &key,
                request_hash: &request_hash,
                kind: "publish",
                actor: actor.as_str(),
                stream: None,
                payload: &payload,
            })
        })?;
        self.advance_publish(&intent.operation_id, actor)
    }

    /// Records an object a client uploaded for an in-flight publication, so
    /// it is protected from garbage collection until the publication ends.
    pub fn stage_object(&self, operation_id: &str, raw: &ag_object::RawObject) -> Result<ObjectId> {
        let id = self.objects.put(raw)?;
        self.meta.write(|tx| tx.intent_add_object(operation_id, &id))?;
        Ok(id)
    }

    /// Resumes (or reports) a publication by operation id.
    pub fn resume_publish(&self, operation_id: &str, actor: &Identity) -> Result<PublishResult> {
        self.advance_publish(operation_id, actor)
    }

    fn publish_result(&self, intent: &IntentRow) -> Result<PublishResult> {
        let head: ChangesetId = serde_json::from_value(intent.payload["head"].clone())?;
        let r = intent.result.clone().unwrap_or(Value::Null);
        Ok(PublishResult {
            operation_id: intent.operation_id.clone(),
            state: intent.state,
            head,
            new_changesets: r["changesets"].as_u64().unwrap_or(0) as usize,
            new_blobs: r["blobs"].as_u64().unwrap_or(0) as usize,
            reason: r["reason"].as_str().map(str::to_string),
        })
    }

    pub(crate) fn advance_publish(&self, operation_id: &str, actor: &Identity) -> Result<PublishResult> {
        loop {
            let intent = self
                .meta
                .read(|tx| tx.intent(operation_id))?
                .ok_or_else(|| Error::NotFound(format!("operation '{operation_id}'")))?;
            if intent.kind != "publish" {
                return Err(Error::Invalid(format!("operation '{operation_id}' is a {} operation", intent.kind)));
            }
            match intent.state {
                IntentState::Prepared => self.publish_upload(&intent)?,
                IntentState::Uploaded => self.publish_verify(&intent)?,
                IntentState::Verified => self.publish_ready(&intent, actor)?,
                IntentState::Published | IntentState::Rejected | IntentState::Failed | IntentState::Aborted => {
                    return self.publish_result(&intent);
                }
                other => return Err(Error::Invalid(format!("unexpected state '{other}' for a publish operation"))),
            }
        }
    }

    fn head_of(intent: &IntentRow) -> Result<ChangesetId> {
        Ok(serde_json::from_value(intent.payload["head"].clone())?)
    }

    fn reject_publish(&self, intent: &IntentRow, from: &[IntentState], reason: String) -> Result<()> {
        self.meta.write(|tx| {
            tx.intent_transition(
                &intent.operation_id,
                from,
                IntentState::Rejected,
                Some(&json!({ "reason": reason })),
                None,
            )?;
            tx.audit_append(&intent.actor, "publish.reject", &intent.operation_id, &json!({ "reason": reason }))?;
            Ok(())
        })?;
        Ok(())
    }

    /// Enumerates the unpublished part of the history below `head`, parents first.
    fn collect_unpublished(&self, head: &ChangesetId) -> Result<std::result::Result<Closure, String>> {
        let mut order: Vec<(ChangesetId, Arc<Changeset>)> = Vec::new();
        let mut done: HashSet<ChangesetId> = HashSet::new();
        let mut stack: Vec<(ChangesetId, bool)> = vec![(*head, false)];
        while let Some((id, expanded)) = stack.pop() {
            if done.contains(&id) {
                continue;
            }
            if expanded {
                let cs = self.changeset(&id)?;
                order.push((id, cs));
                done.insert(id);
                if order.len() > self.limits.max_changesets_per_publication {
                    return Ok(Err(format!(
                        "publication introduces more than {} changesets",
                        self.limits.max_changesets_per_publication
                    )));
                }
                continue;
            }
            if self.meta.read(|tx| tx.is_ready(&id))? {
                done.insert(id);
                continue;
            }
            let cs = match self.changeset(&id) {
                Ok(c) => c,
                Err(Error::NotFound(_)) => return Ok(Err(format!("changeset {id} is missing from the object store"))),
                Err(Error::Invalid(m)) => return Ok(Err(format!("changeset {id} is unusable: {m}"))),
                Err(e) => return Err(e),
            };
            if cs.namespace != self.config.namespace {
                return Ok(Err(format!(
                    "changeset {id} belongs to namespace '{}', not '{}'",
                    cs.namespace, self.config.namespace
                )));
            }
            stack.push((id, true));
            for p in cs.parents.iter().rev() {
                if !done.contains(p) {
                    stack.push((*p, false));
                }
            }
        }
        let blobs = order.iter().flat_map(|(_, cs)| cs.blobs()).collect();
        Ok(Ok(Closure { order, blobs }))
    }

    fn publish_upload(&self, intent: &IntentRow) -> Result<()> {
        let head = Self::head_of(intent)?;
        let closure = match self.collect_unpublished(&head)? {
            Ok(c) => c,
            Err(reason) => return self.reject_publish(intent, &[IntentState::Prepared], reason),
        };
        if closure.order.is_empty() {
            // Already published: nothing to do.
            self.meta.write(|tx| {
                tx.intent_transition(
                    &intent.operation_id,
                    &[IntentState::Prepared],
                    IntentState::Published,
                    Some(&json!({"changesets": 0, "blobs": 0})),
                    None,
                )?;
                Ok(())
            })?;
            return Ok(());
        }
        for b in &closure.blobs {
            if !self.objects.contains(&b.object_id())? {
                return self.reject_publish(
                    intent,
                    &[IntentState::Prepared],
                    format!("blob {b} is missing from the object store"),
                );
            }
        }
        self.meta.write(|tx| {
            for (id, _) in &closure.order {
                tx.intent_add_object(&intent.operation_id, &id.object_id())?;
            }
            for b in &closure.blobs {
                tx.intent_add_object(&intent.operation_id, &b.object_id())?;
            }
            tx.intent_transition(
                &intent.operation_id,
                &[IntentState::Prepared],
                IntentState::Uploaded,
                Some(&json!({"changesets": closure.order.len(), "blobs": closure.blobs.len()})),
                None,
            )?;
            Ok(())
        })?;
        self.failpoints.hit("publish.after_upload")
    }

    fn publish_verify(&self, intent: &IntentRow) -> Result<()> {
        let head = Self::head_of(intent)?;
        let from = [IntentState::Uploaded];
        let closure = match self.collect_unpublished(&head)? {
            Ok(c) => c,
            Err(reason) => return self.reject_publish(intent, &from, reason),
        };

        // Blobs: re-read and hash every one (durability, not just existence).
        for b in &closure.blobs {
            match self.objects.get(&b.object_id()) {
                Ok(Some(raw)) if raw.kind == ObjectKind::Blob => {}
                Ok(Some(_)) => return self.reject_publish(intent, &from, format!("object {b} is not a blob")),
                Ok(None) => return self.reject_publish(intent, &from, format!("blob {b} is missing")),
                Err(ag_storage::StoreError::Corrupt { reason, .. }) => {
                    return self.reject_publish(intent, &from, format!("blob {b} is corrupt: {reason}"));
                }
                Err(e) => return Err(e.into()),
            }
        }

        // History: derive every tree, proving all operation lists apply cleanly.
        let mut remaining: HashMap<ChangesetId, usize> = HashMap::new();
        for (_, cs) in &closure.order {
            if let Some(p) = cs.first_parent() {
                *remaining.entry(p).or_default() += 1;
            }
        }
        let mut trees: HashMap<ChangesetId, Tree> = HashMap::new();
        for (id, cs) in &closure.order {
            let base = match cs.first_parent() {
                None => Tree::empty(),
                Some(p) => match trees.get(&p) {
                    Some(t) => t.clone(),
                    None => self.tree_of(&p)?,
                },
            };
            let tree = match base.apply(&cs.ops, &self.limits) {
                Ok(t) => t,
                Err(e) => {
                    return self.reject_publish(
                        intent,
                        &from,
                        format!("changeset {id} does not apply to its parent: {e}"),
                    );
                }
            };
            // Every blob a symlink points to must be a valid, safe target string.
            for op in &cs.ops {
                if let Op::Put { path, content, file_type: ag_types::FileType::Symlink, .. } = op {
                    let target = self.blob(content)?;
                    let ok = std::str::from_utf8(&target)
                        .ok()
                        .is_some_and(|t| crate::workdir::check_link_target(path, t).is_ok());
                    if !ok {
                        return self.reject_publish(
                            intent,
                            &from,
                            format!("changeset {id}: symlink '{path}' has an unsafe target"),
                        );
                    }
                }
            }
            if let Some(p) = cs.first_parent()
                && let Some(n) = remaining.get_mut(&p)
            {
                *n -= 1;
                if *n == 0 {
                    trees.remove(&p);
                }
            }
            if remaining.get(id).copied().unwrap_or(0) > 0 {
                trees.insert(*id, tree);
            }
        }

        self.meta.write(|tx| {
            tx.intent_transition(
                &intent.operation_id,
                &from,
                IntentState::Verified,
                Some(&json!({"changesets": closure.order.len(), "blobs": closure.blobs.len()})),
                None,
            )?;
            Ok(())
        })?;
        self.failpoints.hit("publish.after_verify")
    }

    fn publish_ready(&self, intent: &IntentRow, actor: &Identity) -> Result<()> {
        let head = Self::head_of(intent)?;
        let from = [IntentState::Verified];
        let closure = match self.collect_unpublished(&head)? {
            Ok(c) => c,
            Err(reason) => return self.reject_publish(intent, &from, reason),
        };
        self.failpoints.hit("publish.before_commit")?;
        self.meta.write(|tx| {
            for (id, cs) in &closure.order {
                tx.register_changeset(id, cs)?;
                tx.outbox_push("changeset.published", &json!({"id": id, "operation_id": intent.operation_id}))?;
            }
            tx.audit_append(
                actor.as_str(),
                "changeset.publish",
                &head.to_hex(),
                &json!({"operation_id": intent.operation_id, "changesets": closure.order.len()}),
            )?;
            tx.intent_transition(
                &intent.operation_id,
                &from,
                IntentState::Published,
                Some(&json!({"changesets": closure.order.len(), "blobs": closure.blobs.len()})),
                None,
            )?;
            Ok(())
        })?;
        // The commit succeeded; a failure here models a lost response.
        self.failpoints.hit("publish.after_commit")?;
        self.derive_best_effort(None, DERIVE_BATCH_AFTER_PUBLISH);
        Ok(())
    }
}
