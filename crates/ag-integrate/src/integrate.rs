//! Integration: moving a stream head to include a proposal.
//!
//! ```text
//! read stream state (head H, version V)
//! candidate  = fast-forward | deterministic merge(base, H, proposal)   <- outside the lock
//! BEGIN IMMEDIATE
//!    assert stream == (H, V) and proposal head/state unchanged; otherwise retry
//!    read facts (policy, attestations for THIS tree, reviews of THIS head, leases)
//!    evaluate policy                     -> allow | blocked | denied (recorded on the intent)
//!    register merge changeset, CAS head, reflog, proposal state, intent state,
//!    audit entry and outbox events       <- all or nothing
//! COMMIT
//! ```
//!
//! A candidate is never reused after the stream moved: the merge is
//! recomputed against the new head, and a validation attestation only counts
//! if it names the *exact* tree of the new candidate.

use ag_agent::leases::lease_blockers;
use ag_agent::provenance;
use ag_changeset::Changeset;
use ag_crypto::{PublicKey, Signature, random_id};
use ag_dag::{is_ancestor, merge_bases};
use ag_merge::{ALGORITHM_VERSION, merge_trees};
use ag_policy::{ApprovalFact, Attestation, ChangedPath, CheckResult, Decision, Evaluation, Facts, Policy, evaluate};
use ag_storage::meta::{IntentRow, IntentState, NewIntent, ProposalRow, StreamState};
use ag_tree::Tree;
use ag_types::{ChangesetId, Identity, IdentityKind, Timestamp, TreeId};
use serde::Serialize;
use serde_json::{Value, json};

use crate::derive::DERIVE_BATCH_AFTER_INTEGRATE;
use crate::publish::hash_request;
use crate::repo::RepoBlobs;
use crate::types::{Blocker, ConflictInfo, IntegrationResult, NoteInfo, Outcome, Strategy};
use crate::{Error, Repository, Result};

/// How an integration behaves when other writers keep moving its stream's head under it.
///
/// Each attempt reads the stream, computes the candidate against that head and tries to commit; a writer that
/// committed in between makes the attempt lose (the compare-and-swap), and the candidate is recomputed against the
/// new head. Under many writers on one stream an attempt can lose several times in a row. After `attempts` losses the
/// integration gives up with [`Error::Contention`] (`aborted` on the wire), which the caller may retry; between
/// attempts it waits a short, spread-out time so that the writers stop colliding in lockstep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentionPolicy {
    /// Attempts before giving up (at least 1).
    pub attempts: usize,
    /// The longest wait between two attempts, in milliseconds. The window starts at 1 ms after the second lost
    /// attempt and doubles per loss up to this; 0 retries at once.
    pub max_backoff_ms: u64,
}

impl Default for ContentionPolicy {
    fn default() -> Self {
        Self { attempts: 24, max_backoff_ms: 32 }
    }
}

impl ContentionPolicy {
    /// How long operation `op` waits before attempt number `attempt` (1-based; the first attempt never waits).
    /// Spread by a hash of the operation id, not by a random generator: the same operation waits the same time, and
    /// different operations (the colliding writers) wait different times.
    pub fn backoff(&self, op: &str, attempt: usize) -> std::time::Duration {
        use sha2::{Digest, Sha256};
        if attempt < 3 || self.max_backoff_ms == 0 {
            return std::time::Duration::ZERO;
        }
        let window = (1u64 << (attempt - 3).min(20)).min(self.max_backoff_ms);
        let h = Sha256::digest(format!("{op}/{attempt}").as_bytes());
        let spread = u64::from_be_bytes(h[..8].try_into().expect("8 bytes")) % (window + 1);
        std::time::Duration::from_millis(spread)
    }
}
/// Most unfinished operations one `recover` pass looks at; the next pass takes the rest.
const RECOVER_BATCH: usize = 10_000;

/// Parameters of an integration request.
#[derive(Clone, Debug)]
pub struct IntegrateRequest {
    /// Proposal to integrate.
    pub proposal_id: String,
    /// Who performs the integration.
    pub actor: Identity,
    /// Explicit idempotency key. Without one the engine derives a key from
    /// the proposal and its head, so repeating the call resumes or replays.
    pub idempotency_key: Option<String>,
    /// Fencing tokens of exclusive leases the author holds.
    pub fencing_tokens: Vec<u64>,
}

/// The merge notes as message lines (`Directory-Rename: ...`), each ending in a newline.
pub(crate) fn note_trailers(notes: &[ag_merge::MergeNote]) -> String {
    notes.iter().map(|n| format!("{}\n", n.trailer())).collect()
}

/// What integrating a proposal *would* do right now. No stream is touched.
#[derive(Clone, Debug, Serialize)]
pub struct MergePreview {
    /// How the head would move.
    pub strategy: Strategy,
    /// Exact tree of the candidate state (what validators must check).
    pub candidate_tree: Option<TreeId>,
    /// Conflicting paths, if any.
    pub conflicts: Vec<ConflictInfo>,
    /// Merge base used.
    pub base: Option<ChangesetId>,
    /// Paths the integration would change in the stream.
    pub changed_files: Vec<String>,
    /// Why no candidate exists (unrelated histories, ...).
    pub problem: Option<String>,
    /// Merge algorithm version.
    pub algorithm: &'static str,
    /// What the merge decided on its own and a person may want to check (a directory rename followed, ...).
    pub notes: Vec<NoteInfo>,
    /// The candidate tree itself (for checkout by validators).
    #[serde(skip)]
    pub tree: Option<Tree>,
}

/// Result of [`Repository::recover`].
#[derive(Clone, Debug, Default, Serialize)]
pub struct RecoveryReport {
    /// Operations that were resumed (with their new state).
    pub resumed: Vec<(String, String)>,
    /// Operations abandoned because they were too old.
    pub aborted: Vec<String>,
    /// Operations left alone (still waiting for something).
    pub waiting: Vec<String>,
    /// Operations that failed to resume, with the error.
    pub errors: Vec<(String, String)>,
}

enum Candidate {
    AlreadyIntegrated,
    FastForward {
        new_head: ChangesetId,
        new_tree: Tree,
        head_tree: Tree,
    },
    Merge {
        id: ChangesetId,
        cs: Box<Changeset>,
        new_tree: Tree,
        head_tree: Tree,
        base: ChangesetId,
        notes: Vec<NoteInfo>,
    },
    Conflict {
        conflicts: Vec<ConflictInfo>,
        base: ChangesetId,
        notes: Vec<NoteInfo>,
    },
    Invalid(String),
}

enum Step {
    Retry,
    Done(Box<IntegrationResult>),
}

fn outcome_json(o: &Outcome) -> Value {
    serde_json::to_value(o).expect("outcome serializes")
}

impl Repository {
    /// Integrates a proposal into its stream.
    ///
    /// Resumable and idempotent: see [`IntegrateRequest::idempotency_key`].
    pub fn integrate(&self, req: &IntegrateRequest) -> Result<IntegrationResult> {
        let prop = self.find_proposal(&req.proposal_id)?;
        let key = match &req.idempotency_key {
            Some(k) => k.clone(),
            None => self.default_integrate_key(&prop)?,
        };
        let request_hash = hash_request(&["integrate", &prop.id, &prop.stream, &prop.head.to_hex()]);
        if let Some(existing) = self.meta.read(|tx| tx.intent_by_key(&key))? {
            // A key may only ever mean one request, finished or not.
            if existing.request_hash != request_hash {
                return Err(Error::Meta(ag_storage::meta::MetaError::IdempotencyKeyReused(key)));
            }
            // A finished operation replays its recorded result, even if the proposal moved on.
            if existing.state.is_terminal() {
                return self.integration_result(&existing);
            }
        }
        if prop.state != "open" {
            return Err(Error::Conflict(format!("proposal '{}' is {}", prop.id, prop.state)));
        }
        let op = random_id("op")?;
        let payload = json!({
            "proposal": prop.id,
            "stream": prop.stream,
            "head": prop.head,
            "tokens": req.fencing_tokens,
        });
        // The record of a new operation whose head is already published is written in the state `ready`: the two
        // steps (`prepared`, then `ready`) were two durable commits for what is one fact, and no crash window
        // between them has a different recovery than the one after them.
        let (intent, created) = self.meta.write(|tx| {
            let (intent, created) = tx.intent_create(&NewIntent {
                operation_id: &op,
                idempotency_key: &key,
                request_hash: &request_hash,
                kind: "integrate",
                actor: req.actor.as_str(),
                stream: Some(&prop.stream),
                payload: &payload,
            })?;
            if created && tx.is_ready(&prop.head)? {
                let ready = tx.intent_transition(
                    &intent.operation_id,
                    &[IntentState::Prepared],
                    IntentState::Ready,
                    None,
                    None,
                )?;
                return Ok((ready, created));
            }
            Ok((intent, created))
        })?;
        if created && intent.state == IntentState::Ready {
            self.failpoints.hit("integrate.after_ready")?;
        }
        self.advance_integrate(&intent.operation_id, &req.actor, Some(&req.fencing_tokens))
    }

    /// Continues an integration by operation id (after a crash, or once the
    /// missing approval/validation exists).
    pub fn resume_integrate(
        &self,
        operation_id: &str,
        actor: &Identity,
        fencing_tokens: Option<&[u64]>,
    ) -> Result<IntegrationResult> {
        self.advance_integrate(operation_id, actor, fencing_tokens)
    }

    /// Looks up an operation's recorded result without changing anything.
    ///
    /// A client that lost the response to `integrate` calls this instead of
    /// blindly retrying.
    pub fn operation(&self, operation_id: &str) -> Result<IntegrationResult> {
        let intent = self
            .meta
            .read(|tx| tx.intent(operation_id))?
            .ok_or_else(|| Error::NotFound(format!("operation '{operation_id}'")))?;
        self.integration_result(&intent)
    }

    /// Like [`Repository::operation`] but looks the operation up by the
    /// idempotency key it was created with.
    pub fn operation_by_key(&self, key: &str) -> Result<IntegrationResult> {
        let intent = self
            .meta
            .read(|tx| tx.intent_by_key(key))?
            .ok_or_else(|| Error::NotFound(format!("no operation with idempotency key '{key}'")))?;
        self.integration_result(&intent)
    }

    fn integration_result(&self, intent: &IntentRow) -> Result<IntegrationResult> {
        let outcome = match &intent.result {
            Some(v) => serde_json::from_value(v.clone())
                .unwrap_or_else(|_| Outcome::Invalid { reason: "unreadable result".into() }),
            None => Outcome::Invalid { reason: format!("operation is {} and has no result yet", intent.state) },
        };
        Ok(IntegrationResult { operation_id: intent.operation_id.clone(), state: intent.state, outcome })
    }

    fn default_integrate_key(&self, prop: &ProposalRow) -> Result<String> {
        let prefix = format!("integrate:{}:{}:", prop.id, prop.head);
        let existing = self.meta.read(|tx| tx.intents_with_key_prefix(&prefix))?;
        Ok(match existing.last() {
            Some(last) if !matches!(last.state, IntentState::Rejected | IntentState::Failed | IntentState::Aborted) => {
                last.idempotency_key.clone()
            }
            Some(_) => format!("{prefix}{}", existing.len()),
            None => format!("{prefix}0"),
        })
    }

    pub(crate) fn advance_integrate(
        &self,
        op: &str,
        actor: &Identity,
        tokens: Option<&[u64]>,
    ) -> Result<IntegrationResult> {
        if let Some(t) = tokens {
            // Only a change is written: a new operation already carries these tokens in its payload.
            let wanted = json!(t);
            let current = self.meta.read(|tx| tx.intent(op))?;
            if let Some(i) = current
                && !i.state.is_terminal()
                && i.payload["tokens"] != wanted
            {
                self.meta.write(|tx| {
                    if let Some(i) = tx.intent(op)?
                        && !i.state.is_terminal()
                    {
                        let mut p = i.payload.clone();
                        p["tokens"] = wanted.clone();
                        tx.intent_set_payload(op, &p)?;
                    }
                    Ok(())
                })?;
            }
        }
        let policy = self.contention;
        for attempt in 1..=policy.attempts.max(1) {
            let wait = policy.backoff(op, attempt);
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
            let intent =
                self.meta.read(|tx| tx.intent(op))?.ok_or_else(|| Error::NotFound(format!("operation '{op}'")))?;
            if intent.kind != "integrate" {
                return Err(Error::Invalid(format!("operation '{op}' is a {} operation", intent.kind)));
            }
            if intent.state.is_terminal() {
                return self.integration_result(&intent);
            }
            // The payload was written by `integrate` itself; if a field is missing or unreadable the record is
            // damaged, and carrying on with an empty proposal id, stream or token list would answer a different
            // question than the one that was asked.
            let proposal_id = payload_str(&intent, "proposal")?;
            let stream = payload_str(&intent, "stream")?;
            let head: ChangesetId = serde_json::from_value(intent.payload["head"].clone())?;
            let tokens: Vec<u64> = serde_json::from_value(intent.payload["tokens"].clone()).map_err(|e| {
                Error::Invalid(format!(
                    "operation '{}' has an unreadable 'tokens' in its payload: {e}",
                    intent.operation_id
                ))
            })?;

            let Some(prop) = self.meta.read(|tx| tx.proposal(&proposal_id))? else {
                return self.abort_integrate(op, "the proposal no longer exists");
            };
            if prop.state != "open" {
                return self.abort_integrate(op, &format!("the proposal is {}", prop.state));
            }
            if prop.head != head {
                return self
                    .abort_integrate(op, "the proposal was updated after this integration started; start a new one");
            }
            if intent.state == IntentState::Prepared {
                if !self.meta.read(|tx| tx.is_ready(&head))? {
                    return self.abort_integrate(op, "the proposal head is not published; publish it first");
                }
                self.meta.write(|tx| {
                    tx.intent_transition(op, &[IntentState::Prepared], IntentState::Ready, None, None)?;
                    Ok(())
                })?;
                self.failpoints.hit("integrate.after_ready")?;
            }

            let st = self
                .meta
                .read(|tx| tx.stream(&stream))?
                .ok_or_else(|| Error::NotFound(format!("stream '{stream}'")))?
                .state;
            let meta_for_merge =
                MergeMeta { author: actor.clone(), timestamp: intent.created_at, operation_id: op.to_string() };
            let candidate = self.compute_candidate(&prop, &st, Some(&meta_for_merge))?;
            self.failpoints.pause("integrate.before_commit");
            self.failpoints.hit("integrate.before_commit")?;
            match &candidate {
                Candidate::AlreadyIntegrated => match self.finish_already_integrated(op, actor, &prop, &st)? {
                    Some(result) => return Ok(result),
                    None => continue,
                },
                Candidate::Conflict { conflicts, base, .. } => {
                    let outcome = Outcome::Conflict { conflicts: conflicts.clone(), base: Some(*base) };
                    return self.reject(op, actor, &outcome, "integrate.conflict");
                }
                Candidate::Invalid(reason) => {
                    let outcome = Outcome::Invalid { reason: reason.clone() };
                    return self.reject(op, actor, &outcome, "integrate.invalid");
                }
                _ => {}
            }
            match self.commit_integration(&intent, &prop, actor, &st, &tokens, candidate)? {
                Step::Retry => continue,
                Step::Done(result) => {
                    self.failpoints.hit("integrate.after_commit")?;
                    let new_head = match &result.outcome {
                        Outcome::Integrated { new_head, .. } => Some(new_head),
                        _ => None,
                    };
                    self.derive_best_effort(new_head, DERIVE_BATCH_AFTER_INTEGRATE);
                    return Ok(*result);
                }
            }
        }
        Err(Error::Contention(format!("stream changed {} times while integrating; retry", policy.attempts.max(1))))
    }

    fn abort_integrate(&self, op: &str, reason: &str) -> Result<IntegrationResult> {
        let outcome = Outcome::Invalid { reason: reason.to_string() };
        let intent = self.meta.write(|tx| {
            let i = tx.intent_transition(
                op,
                &[IntentState::Prepared, IntentState::Ready, IntentState::Blocked],
                IntentState::Aborted,
                Some(&outcome_json(&outcome)),
                None,
            )?;
            tx.audit_append(&i.actor, "integrate.abort", op, &json!({"reason": reason}))?;
            Ok(i)
        })?;
        self.integration_result(&intent)
    }

    fn reject(&self, op: &str, actor: &Identity, outcome: &Outcome, action: &str) -> Result<IntegrationResult> {
        let intent = self.meta.write(|tx| {
            let i = tx.intent_transition(
                op,
                &[IntentState::Prepared, IntentState::Ready, IntentState::Blocked],
                IntentState::Rejected,
                Some(&outcome_json(outcome)),
                None,
            )?;
            tx.audit_append(actor.as_str(), action, op, &outcome_json(outcome))?;
            Ok(i)
        })?;
        self.integration_result(&intent)
    }

    fn finish_already_integrated(
        &self,
        op: &str,
        actor: &Identity,
        prop: &ProposalRow,
        st: &StreamState,
    ) -> Result<Option<IntegrationResult>> {
        let head = st.head.expect("already integrated implies a head");
        let tree = self.tree_of(&head)?;
        let outcome = Outcome::Integrated {
            new_head: head,
            previous_head: Some(head),
            version: st.version,
            strategy: Strategy::AlreadyIntegrated,
            tree_id: tree.id(),
            changed_files: 0,
        };
        let intent = self.meta.write(|tx| {
            if !integration_snapshot_matches(tx, prop, st)? {
                return Ok(None);
            }
            if let Some(blocked) = contained_discussion_blocker(tx, op, actor, prop, &head, tree.id())? {
                return Ok(Some(blocked));
            }
            tx.proposal_set_state(&prop.id, "integrated", Some(&head))?;
            let i = tx.intent_transition(
                op,
                &[IntentState::Prepared, IntentState::Ready, IntentState::Blocked],
                IntentState::Integrated,
                Some(&outcome_json(&outcome)),
                None,
            )?;
            tx.audit_append(
                actor.as_str(),
                "stream.integrate",
                &prop.stream,
                &json!({"proposal": prop.id, "strategy": "already_integrated"}),
            )?;
            Ok(Some(i))
        })?;
        intent.map(|intent| self.integration_result(&intent)).transpose()
    }

    /// Computes what integration would produce against stream state `st`.
    fn compute_candidate(
        &self,
        prop: &ProposalRow,
        st: &StreamState,
        merge_meta: Option<&MergeMeta>,
    ) -> Result<Candidate> {
        let p = prop.head;
        let Some(h) = st.head else {
            let new_tree = self.tree_of(&p)?;
            return Ok(Candidate::FastForward { new_head: p, new_tree, head_tree: Tree::empty() });
        };
        let g = self.graph();
        if is_ancestor(&g, &p, &h)? {
            return Ok(Candidate::AlreadyIntegrated);
        }
        let head_tree = self.tree_of(&h)?;
        if is_ancestor(&g, &h, &p)? {
            let new_tree = self.tree_of(&p)?;
            return Ok(Candidate::FastForward { new_head: p, new_tree, head_tree });
        }
        let bases = merge_bases(&g, &h, &p)?;
        let base = match bases.as_slice() {
            [] => return Ok(Candidate::Invalid("the proposal's history is unrelated to the stream's history".into())),
            [one] => *one,
            many => {
                let list = many.iter().map(|b| b.short()).collect::<Vec<_>>().join(", ");
                return Ok(Candidate::Invalid(format!(
                    "criss-cross history: {} equally good merge bases ({list}); merge the stream into the proposal manually first",
                    many.len()
                )));
            }
        };
        let base_tree = self.tree_of(&base)?;
        let theirs = self.tree_of(&p)?;
        let merged = merge_trees(&base_tree, &head_tree, &theirs, &RepoBlobs(self), &self.limits)
            .map_err(|e| Error::Invalid(format!("merge failed: {e}")))?;
        if !merged.is_clean() {
            let conflicts = merged.conflicts.iter().map(ConflictInfo::from_conflict).collect();
            let notes = merged.notes.iter().map(NoteInfo::from_note).collect();
            return Ok(Candidate::Conflict { conflicts, base, notes });
        }
        let ops = head_tree.diff(&merged.tree);
        let default_meta;
        let mm = match merge_meta {
            Some(m) => m,
            None => {
                default_meta = MergeMeta {
                    author: Identity::new("system:preview").expect("valid"),
                    timestamp: Timestamp::from_millis(0),
                    operation_id: "preview".into(),
                };
                &default_meta
            }
        };
        let message = format!(
            "Integrate {}: {}\n\nProposal-Head: {}\nMerge-Base: {}\nMerge-Algorithm: {}\n{}Operation: {}\n",
            prop.id,
            prop.title,
            p,
            base,
            ALGORITHM_VERSION,
            note_trailers(&merged.notes),
            mm.operation_id
        );
        let cs = Changeset::build(
            self.config.namespace.clone(),
            vec![h, p],
            mm.author.clone(),
            mm.timestamp,
            message,
            ops,
            &self.limits,
        )?;
        let id = cs.id();
        if merge_meta.is_some() {
            self.put_changeset(&cs)?;
        }
        let notes = merged.notes.iter().map(NoteInfo::from_note).collect();
        Ok(Candidate::Merge { id, cs: Box::new(cs), new_tree: merged.tree, head_tree, base, notes })
    }

    /// Dry run: reports what integrating `proposal_id` would do now.
    pub fn merge_preview(&self, proposal_id: &str) -> Result<MergePreview> {
        let prop = self.find_proposal(proposal_id)?;
        let st = self
            .meta
            .read(|tx| tx.stream(&prop.stream))?
            .ok_or_else(|| Error::NotFound(format!("stream '{}'", prop.stream)))?
            .state;
        let mut preview = MergePreview {
            strategy: Strategy::FastForward,
            candidate_tree: None,
            conflicts: vec![],
            base: None,
            changed_files: vec![],
            problem: None,
            algorithm: ALGORITHM_VERSION,
            notes: vec![],
            tree: None,
        };
        match self.compute_candidate(&prop, &st, None)? {
            Candidate::AlreadyIntegrated => {
                preview.strategy = Strategy::AlreadyIntegrated;
            }
            Candidate::FastForward { new_tree, head_tree, .. } => {
                preview.changed_files = head_tree.diff(&new_tree).iter().map(|o| o.path().to_string()).collect();
                preview.candidate_tree = Some(new_tree.id());
                preview.tree = Some(new_tree);
            }
            Candidate::Merge { new_tree, head_tree, base, notes, .. } => {
                preview.strategy = Strategy::Merge;
                preview.notes = notes;
                preview.base = Some(base);
                preview.changed_files = head_tree.diff(&new_tree).iter().map(|o| o.path().to_string()).collect();
                preview.candidate_tree = Some(new_tree.id());
                preview.tree = Some(new_tree);
            }
            Candidate::Conflict { conflicts, base, notes } => {
                preview.strategy = Strategy::Merge;
                preview.notes = notes;
                preview.base = Some(base);
                preview.conflicts = conflicts;
            }
            Candidate::Invalid(reason) => preview.problem = Some(reason),
        }
        Ok(preview)
    }

    fn commit_integration(
        &self,
        intent: &IntentRow,
        prop: &ProposalRow,
        actor: &Identity,
        st: &StreamState,
        tokens: &[u64],
        candidate: Candidate,
    ) -> Result<Step> {
        let op = intent.operation_id.as_str();
        let (new_head, new_tree, head_tree, strategy, merge_cs) = match candidate {
            Candidate::FastForward { new_head, new_tree, head_tree } => {
                (new_head, new_tree, head_tree, Strategy::FastForward, None)
            }
            Candidate::Merge { id, cs, new_tree, head_tree, .. } => {
                (id, new_tree, head_tree, Strategy::Merge, Some((id, cs)))
            }
            _ => unreachable!("only mergeable candidates reach the commit step"),
        };
        let tree_id = new_tree.id();
        let changed: Vec<String> = head_tree.diff(&new_tree).iter().map(|o| o.path().to_string()).collect();
        let changed_paths: Vec<ChangedPath> = changed.iter().map(|p| ChangedPath { path: p.clone() }).collect();
        let author = Identity::new(prop.author.clone()).map_err(|e| Error::Invalid(format!("proposal author: {e}")))?;

        let step = self.meta.write(|tx| {
            // 1. Stream and proposal must still match the candidate snapshot.
            if !integration_snapshot_matches(tx, prop, st)? {
                return Ok(Step::Retry);
            }

            // 2. Facts, read in the same transaction that will commit the change.
            let Verdict { evaluation, blockers, policy_version, .. } =
                verdict_in_tx(tx, prop, actor, &author, tree_id, &changed_paths, &changed, tokens)?;

            // 3. Decide.
            if evaluation.decision == Decision::Denied {
                let outcome = Outcome::Denied { blockers };
                let i = tx.intent_transition(op, &[IntentState::Prepared, IntentState::Ready, IntentState::Blocked], IntentState::Rejected, Some(&outcome_json(&outcome)), None)?;
                tx.audit_append(actor.as_str(), "integrate.denied", op, &outcome_json(&outcome))?;
                return Ok(Step::Done(Box::new(IntegrationResult { operation_id: i.operation_id, state: i.state, outcome })));
            }
            if !blockers.is_empty() {
                let outcome = Outcome::Blocked { candidate_changeset: new_head, candidate_tree: tree_id, policy_version, blockers };
                let i = tx.intent_transition(op, &[IntentState::Prepared, IntentState::Ready, IntentState::Blocked], IntentState::Blocked, Some(&outcome_json(&outcome)), None)?;
                tx.audit_append(actor.as_str(), "integrate.blocked", op, &outcome_json(&outcome))?;
                return Ok(Step::Done(Box::new(IntegrationResult { operation_id: i.operation_id, state: i.state, outcome })));
            }

            // 4. Allowed: publish the merge changeset, CAS the head, record everything.
            if let Some((id, cs)) = &merge_cs {
                tx.register_changeset(id, cs)?;
                tx.outbox_push("changeset.published", &json!({"id": id, "operation_id": op}))?;
            }
            let reason = format!("integrate {} ({})", prop.id, match strategy { Strategy::Merge => "merge", _ => "fast-forward" });
            let new_state = tx.cas_stream(&prop.stream, st, &new_head, Some(op), actor.as_str(), &reason)?;
            tx.proposal_set_state(&prop.id, "integrated", Some(&new_head))?;
            let outcome = Outcome::Integrated {
                new_head,
                previous_head: st.head,
                version: new_state.version,
                strategy,
                tree_id,
                changed_files: changed.len(),
            };
            let i = tx.intent_transition(op, &[IntentState::Prepared, IntentState::Ready, IntentState::Blocked], IntentState::Integrated, Some(&outcome_json(&outcome)), None)?;
            tx.audit_append(
                actor.as_str(),
                "stream.integrate",
                &prop.stream,
                &json!({"proposal": prop.id, "operation_id": op, "old_head": st.head, "new_head": new_head, "version": new_state.version, "policy_version": policy_version, "tree": tree_id}),
            )?;
            tx.outbox_push(
                "stream.updated",
                &json!({"stream": prop.stream, "old_head": st.head, "new_head": new_head, "version": new_state.version, "operation_id": op}),
            )?;
            Ok(Step::Done(Box::new(IntegrationResult { operation_id: i.operation_id, state: i.state, outcome })))
        })?;
        Ok(step)
    }

    /// Resumes interrupted operations and abandons stale ones.
    ///
    /// * Non-terminal publish operations and integrations that were interrupted
    ///   mid-flight are resumed (steps are idempotent).
    /// * Operations untouched for longer than `max_age_ms` are aborted.
    /// * Integrations waiting for approvals/validation are left alone until
    ///   they are older than `max_age_ms`.
    pub fn recover(&self, max_age_ms: i64) -> Result<RecoveryReport> {
        let mut report = RecoveryReport::default();
        let now = self.clock().now();
        let pending = self.meta.read(|tx| {
            tx.intents_in_states(
                &[
                    IntentState::Prepared,
                    IntentState::Uploaded,
                    IntentState::Verified,
                    IntentState::Ready,
                    IntentState::Blocked,
                ],
                RECOVER_BATCH,
            )
        })?;
        for intent in pending {
            let op = intent.operation_id.clone();
            if now.as_millis() - intent.updated_at.as_millis() > max_age_ms {
                self.meta.write(|tx| {
                    tx.intent_transition(
                        &op,
                        &[
                            IntentState::Prepared,
                            IntentState::Uploaded,
                            IntentState::Verified,
                            IntentState::Ready,
                            IntentState::Blocked,
                        ],
                        IntentState::Aborted,
                        Some(&outcome_json(&Outcome::Invalid { reason: "expired".into() })),
                        None,
                    )?;
                    tx.audit_append("system:recovery", "operation.expire", &op, &json!({}))?;
                    Ok(())
                })?;
                report.aborted.push(op);
                continue;
            }
            if intent.state == IntentState::Blocked {
                report.waiting.push(op);
                continue;
            }
            let actor = Identity::new(intent.actor.clone())
                .unwrap_or_else(|_| Identity::new("system:recovery").expect("valid"));
            let result: Result<String> = match intent.kind.as_str() {
                "publish" => self.advance_publish(&op, &actor).map(|r| r.state.to_string()),
                "integrate" => self.advance_integrate(&op, &actor, None).map(|r| r.state.to_string()),
                other => Err(Error::Invalid(format!("unknown operation kind '{other}'"))),
            };
            match result {
                Ok(state) => report.resumed.push((op, state)),
                Err(e) => report.errors.push((op, e.to_string())),
            }
        }
        Ok(report)
    }
}

/// A string field of an operation's payload, or an error naming the damaged record.
fn payload_str(intent: &IntentRow, field: &str) -> Result<String> {
    intent.payload[field]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| Error::Invalid(format!("operation '{}' has no '{field}' in its payload", intent.operation_id)))
}

/// Everything policy has to say about a candidate, evaluated against facts
/// read through `tx`. Used by the committing transaction (so verdict and
/// commit see one state) and by the read-only [`Repository::prepare`].
pub(crate) struct Verdict {
    pub(crate) evaluation: Evaluation,
    pub(crate) blockers: Vec<Blocker>,
    pub(crate) policy: Policy,
    pub(crate) policy_version: u64,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn verdict_in_tx(
    tx: &ag_storage::meta::Tx<'_>,
    prop: &ProposalRow,
    actor: &Identity,
    author: &Identity,
    tree_id: TreeId,
    changed_paths: &[ChangedPath],
    changed: &[String],
    tokens: &[u64],
) -> std::result::Result<Verdict, ag_storage::meta::MetaError> {
    let (policy, policy_version) = policy_in_tx(tx, &prop.stream)?;
    let attestations: Vec<Attestation> =
        tx.attestations_for_tree(&tree_id)?.iter().filter_map(attestation_from_row).collect();
    let approvals: Vec<ApprovalFact> = tx
        .reviews(&prop.id)?
        .into_iter()
        .filter(|r| r.decision == "approve" || r.decision == "request_changes")
        .filter_map(|r| {
            Some(ApprovalFact {
                reviewer: Identity::new(r.reviewer).ok()?,
                head: r.head,
                approve: r.decision == "approve",
            })
        })
        .collect();
    let has_provenance =
        author.kind() == IdentityKind::Agent && !provenance::verified_for(tx, &prop.head, author)?.is_empty();
    let facts = Facts {
        actor,
        author,
        proposal_head: prop.head,
        candidate_tree: tree_id,
        policy_version,
        changed: changed_paths,
        attestations: &attestations,
        approvals: &approvals,
        has_provenance,
        unresolved_threads: if policy.require_resolved_threads { tx.unresolved_review_threads(&prop.id)? } else { 0 },
        now: tx.now(),
    };
    let evaluation: Evaluation = evaluate(&policy, &facts);
    let mut blockers: Vec<Blocker> = evaluation.unmet.iter().map(Blocker::from_unmet).collect();
    for lb in lease_blockers(tx, changed, author.as_str(), tokens)? {
        blockers.push(Blocker { code: "lease".into(), message: lb.to_string(), detail: serde_json::to_value(&lb)? });
    }
    Ok(Verdict { evaluation, blockers, policy, policy_version })
}

struct MergeMeta {
    author: Identity,
    timestamp: Timestamp,
    operation_id: String,
}

/// Converts a stored attestation row back into a verifiable [`Attestation`].
/// Rows that do not parse are ignored: stored attestations are untrusted
/// input until the policy engine verifies their signatures.
fn attestation_from_row(r: &ag_storage::meta::AttestationRow) -> Option<Attestation> {
    Some(Attestation {
        id: r.id.clone(),
        tree_id: r.tree_id,
        check: r.check_name.clone(),
        result: match r.result.as_str() {
            "pass" => CheckResult::Pass,
            "fail" => CheckResult::Fail,
            _ => return None,
        },
        policy_version: r.policy_version,
        environment_digest: r.environment_digest.clone(),
        issuer: Identity::new(r.issuer.clone()).ok()?,
        issued_at: r.issued_at,
        public_key: PublicKey::from_hex(&r.public_key).ok()?,
        signature: Signature::from_hex(&r.signature).ok()?,
    })
}

/// Candidate computation happens outside the write lock; both mutable inputs must still match.
fn integration_snapshot_matches(
    tx: &ag_storage::meta::Tx<'_>,
    proposal: &ProposalRow,
    stream: &StreamState,
) -> std::result::Result<bool, ag_storage::meta::MetaError> {
    let Some(current) = tx.proposal(&proposal.id)? else { return Ok(false) };
    if current.state != "open" || current.head != proposal.head {
        return Ok(false);
    }
    Ok(tx.stream(&proposal.stream)?.is_some_and(|row| row.state == *stream))
}

fn policy_in_tx(
    tx: &ag_storage::meta::Tx<'_>,
    stream: &str,
) -> std::result::Result<(Policy, u64), ag_storage::meta::MetaError> {
    use ag_storage::meta::MetaError;
    Ok(match tx.policy_current(stream)? {
        Some(row) => (
            Policy::parse(&row.doc)
                .map_err(|e| MetaError::Invalid(format!("stored policy v{} is invalid: {e}", row.version)))?,
            row.version,
        ),
        None => (Policy::default(), 0),
    })
}

fn contained_discussion_blocker(
    tx: &ag_storage::meta::Tx<'_>,
    op: &str,
    actor: &Identity,
    proposal: &ProposalRow,
    head: &ChangesetId,
    tree: TreeId,
) -> std::result::Result<Option<IntentRow>, ag_storage::meta::MetaError> {
    let (policy_version, requirement) = discussion_gate_in_tx(tx, proposal)?;
    let Some(requirement) = requirement else { return Ok(None) };
    let outcome = Outcome::Blocked {
        candidate_changeset: *head,
        candidate_tree: tree,
        policy_version,
        blockers: vec![requirement],
    };
    let intent = tx.intent_transition(
        op,
        &[IntentState::Prepared, IntentState::Ready, IntentState::Blocked],
        IntentState::Blocked,
        Some(&outcome_json(&outcome)),
        None,
    )?;
    tx.audit_append(actor.as_str(), "integrate.blocked", op, &outcome_json(&outcome))?;
    Ok(Some(intent))
}

pub(crate) fn discussion_gate_in_tx(
    tx: &ag_storage::meta::Tx<'_>,
    proposal: &ProposalRow,
) -> std::result::Result<(u64, Option<Blocker>), ag_storage::meta::MetaError> {
    let (policy, version) = policy_in_tx(tx, &proposal.stream)?;
    let count = if policy.require_resolved_threads { tx.unresolved_review_threads(&proposal.id)? } else { 0 };
    Ok((version, policy.thread_resolution_requirement(count).as_ref().map(Blocker::from_unmet)))
}
