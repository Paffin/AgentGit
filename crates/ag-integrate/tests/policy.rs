//! Policy, attestation, approval and agent-autonomy behaviour through the engine.
#![allow(missing_docs)]

mod common;

use ag_agent::leases;
use ag_agent::provenance::{self, ProvenanceRecord};
use ag_agent::registry::{self, KeyStore};
use ag_crypto::KeyPair;
use ag_integrate::{Blocker, IntegrateRequest, Outcome};
use ag_policy::{Attestation, CheckResult};
use ag_storage::meta::IntentState;
use ag_types::TreeId;
use common::*;

fn ci_key() -> KeyPair {
    KeyPair::from_seed([42; 32])
}

fn policy_json(extra: &str) -> String {
    format!(
        r#"{{"trusted_issuers":[{{"identity":"service:ci","public_key":"{}"}}],
            "required_validations":[{{"check":"tests"}}]{extra}}}"#,
        ci_key().public()
    )
}

fn attest(f: &Fx, tree: TreeId, result: CheckResult, policy_version: u64, key: &KeyPair, issuer: &str) -> Attestation {
    Attestation::sign(
        key,
        ag_crypto::random_id("att").unwrap(),
        tree,
        "tests".into(),
        result,
        policy_version,
        "env-1".into(),
        id(issuer),
        f.clock.now_ts(),
    )
    .unwrap()
}

trait ClockExt {
    fn now_ts(&self) -> ag_types::Timestamp;
}
impl ClockExt for std::sync::Arc<ag_storage::ManualClock> {
    fn now_ts(&self) -> ag_types::Timestamp {
        use ag_storage::Clock;
        self.now()
    }
}

fn blocked(r: &ag_integrate::IntegrationResult) -> (ag_types::TreeId, &Vec<Blocker>) {
    match &r.outcome {
        Outcome::Blocked { candidate_tree, blockers, .. } => (*candidate_tree, blockers),
        other => panic!("expected Blocked, got {other:?}"),
    }
}

#[test]
fn validation_gate_blocks_until_a_trusted_attestation_names_the_exact_tree() {
    let f = fx();
    let pol = f.repo.set_policy("main", &policy_json(""), &id("user:alice")).unwrap();
    let c = f.snap(None, &[("a", "1")], "user:bob");
    let p = f.propose(c, "t", "user:bob");

    let r = f.integrate(&p, "user:alice");
    assert_eq!(r.state, IntentState::Blocked);
    let (tree, blockers) = blocked(&r);
    assert_eq!(blockers[0].code, "missing_validation");
    assert_eq!(f.head(), None, "blocked integrations do not move the stream");

    // An attestation for a DIFFERENT tree changes nothing.
    let other = TreeId::new(ag_types::ObjectId::from_bytes([1; 32]));
    f.repo.submit_attestation(&attest(&f, other, CheckResult::Pass, pol.version, &ci_key(), "service:ci")).unwrap();
    assert_eq!(f.integrate(&p, "user:alice").state, IntentState::Blocked);

    // A correctly bound, trusted, passing attestation unblocks the *same* operation.
    f.repo.submit_attestation(&attest(&f, tree, CheckResult::Pass, pol.version, &ci_key(), "service:ci")).unwrap();
    let before_op = r.operation_id.clone();
    let r2 = f.integrate(&p, "user:alice");
    assert_eq!(r2.operation_id, before_op, "the blocked operation was resumed, not duplicated");
    assert!(matches!(r2.outcome, Outcome::Integrated { .. }), "{:?}", r2.outcome);
}

#[test]
fn untrusted_or_mismatched_attestations_do_not_count() {
    let f = fx();
    let pol = f.repo.set_policy("main", &policy_json(""), &id("user:alice")).unwrap();
    let c = f.snap(None, &[("a", "1")], "agent:bot#run_1");
    let p = f.propose(c, "t", "agent:bot#run_1");
    let r = f.integrate(&p, "user:alice");
    let (tree, _) = blocked(&r);

    // The agent signs its own "pass", claiming to be the CI service.
    let agent_key = KeyPair::from_seed([1; 32]);
    f.repo.submit_attestation(&attest(&f, tree, CheckResult::Pass, pol.version, &agent_key, "service:ci")).unwrap();
    assert_eq!(f.integrate(&p, "user:alice").state, IntentState::Blocked, "right name, wrong key");

    // The right key but the wrong policy version.
    f.repo.submit_attestation(&attest(&f, tree, CheckResult::Pass, pol.version + 5, &ci_key(), "service:ci")).unwrap();
    assert_eq!(f.integrate(&p, "user:alice").state, IntentState::Blocked, "attestation for another policy version");

    // A trusted, correctly bound FAIL denies the change outright, even though
    // a trusted PASS for the same tree exists too: failure wins.
    let good = attest(&f, tree, CheckResult::Pass, pol.version, &ci_key(), "service:ci");
    f.repo.submit_attestation(&good).unwrap();
    let bad = attest(&f, tree, CheckResult::Fail, pol.version, &ci_key(), "service:ci");
    f.repo.submit_attestation(&bad).unwrap();
    let r = f.integrate(&p, "user:alice");
    assert_eq!(r.state, IntentState::Rejected, "a trusted FAIL on this exact tree denies the change");
    assert!(matches!(r.outcome, Outcome::Denied { .. }));
}

#[test]
fn attestation_cannot_be_reused_after_the_stream_moves() {
    let f = fx();
    let base = f.land(None, &[("a", "1\n"), ("b", "1\n")]);
    let pol = f.repo.set_policy("main", &policy_json(""), &id("user:alice")).unwrap();

    // two proposals from the same base
    let c1 = f.snap(Some(base), &[("a", "2\n"), ("b", "1\n")], "user:bob");
    let c2 = f.snap(Some(base), &[("a", "1\n"), ("b", "2\n")], "user:carol");
    let p1 = f.propose(c1, "one", "user:bob");
    let p2 = f.propose(c2, "two", "user:carol");

    // validate p2 against the stream as it is NOW, then land p1 first
    let r2 = f.integrate(&p2, "user:alice");
    let (tree_v1, _) = blocked(&r2);
    f.repo.submit_attestation(&attest(&f, tree_v1, CheckResult::Pass, pol.version, &ci_key(), "service:ci")).unwrap();
    let r1 = f.integrate(&p1, "user:alice");
    let (tree_p1, _) = blocked(&r1);
    f.repo.submit_attestation(&attest(&f, tree_p1, CheckResult::Pass, pol.version, &ci_key(), "service:ci")).unwrap();
    assert!(matches!(f.integrate(&p1, "user:alice").outcome, Outcome::Integrated { .. }));

    // p2 was validated against the OLD head. The result of integrating it now is a different tree.
    let again = f.integrate(&p2, "user:alice");
    assert_eq!(again.state, IntentState::Blocked, "validation must be redone for the new candidate");
    let (tree_v2, _) = blocked(&again);
    assert_ne!(tree_v1, tree_v2);
    f.repo.submit_attestation(&attest(&f, tree_v2, CheckResult::Pass, pol.version, &ci_key(), "service:ci")).unwrap();
    assert!(matches!(f.integrate(&p2, "user:alice").outcome, Outcome::Integrated { .. }));
}

#[test]
fn approvals_are_bound_to_the_reviewed_head_and_exclude_the_author() {
    let f = fx();
    f.repo.set_policy("main", r#"{"approvals":{"min":1}}"#, &id("user:alice")).unwrap();
    let c1 = f.snap(None, &[("a", "1")], "user:bob");
    let p = f.propose(c1, "t", "user:bob");

    f.repo.review(&p, &id("user:bob"), "approve", "my own work is great").unwrap();
    assert_eq!(f.integrate(&p, "user:alice").state, IntentState::Blocked, "self-approval is ignored");

    f.repo.review(&p, &id("user:carol"), "approve", "lgtm").unwrap();
    // …but the author pushes a new commit before it is integrated
    let c2 = f.snap(Some(c1), &[("a", "1"), ("evil", "payload")], "user:bob");
    f.repo.update_proposal(&p, &c2, &id("user:bob")).unwrap();
    let r = f.integrate(&p, "user:alice");
    assert_eq!(r.state, IntentState::Blocked, "carol approved the old head, not this one");
    let (_, blockers) = blocked(&r);
    assert_eq!(blockers[0].code, "insufficient_approvals");

    f.repo.review(&p, &id("user:carol"), "approve", "re-checked the new commit").unwrap();
    assert!(matches!(f.integrate(&p, "user:alice").outcome, Outcome::Integrated { .. }));
    // only the author can move a proposal
    let c3 = f.snap(Some(c2), &[("a", "1"), ("evil", "payload"), ("z", "z")], "user:mallory");
    let p2 = f.propose(c3, "other", "user:mallory");
    assert!(f.repo.update_proposal(&p2, &c1, &id("user:bob")).is_err());
}

#[test]
fn changes_requested_blocks_until_the_reviewer_changes_their_mind() {
    let f = fx();
    f.repo.set_policy("main", r#"{"approvals":{"min":1}}"#, &id("user:alice")).unwrap();
    let c = f.snap(None, &[("a", "1")], "user:bob");
    let p = f.propose(c, "t", "user:bob");
    f.repo.review(&p, &id("user:carol"), "approve", "").unwrap();
    f.repo.review(&p, &id("user:dave"), "request_changes", "needs tests").unwrap();
    let r = f.integrate(&p, "user:alice");
    assert_eq!(r.state, IntentState::Blocked);
    assert!(blocked(&r).1.iter().any(|b| b.code == "changes_requested"));
    f.repo.review(&p, &id("user:dave"), "approve", "tests added").unwrap();
    assert!(matches!(f.integrate(&p, "user:alice").outcome, Outcome::Integrated { .. }));
}

#[test]
fn protected_paths_need_extra_approvals() {
    let f = fx();
    f.repo.set_policy(
        "main",
        r#"{"approvals":{"min":1},"protected_paths":[{"glob":".github/**","min_approvals":2,"approvers":["user:admin*"]}]}"#,
        &id("user:alice"),
    )
    .unwrap();
    let c = f.snap(None, &[(".github/workflows/ci.yml", "on: push"), ("README", "x")], "user:bob");
    let p = f.propose(c, "ci", "user:bob");
    f.repo.review(&p, &id("user:carol"), "approve", "").unwrap();
    let r = f.integrate(&p, "user:alice");
    assert_eq!(r.state, IntentState::Blocked);
    f.repo.review(&p, &id("user:admin1"), "approve", "").unwrap();
    f.repo.review(&p, &id("user:admin2"), "approve", "").unwrap();
    assert!(matches!(f.integrate(&p, "user:alice").outcome, Outcome::Integrated { .. }));
}

#[test]
fn agents_propose_humans_integrate() {
    let f = fx();
    let c = f.snap(None, &[("a", "1")], "agent:bot#run_1");
    let p = f.propose(c, "t", "agent:bot#run_1");
    // an agent cannot run the integration itself under the default policy
    let r = f.integrate(&p, "agent:bot#run_1");
    assert_eq!(r.state, IntentState::Rejected);
    assert!(matches!(r.outcome, Outcome::Denied { .. }));
    // …and an agent's own change needs a human approval (min_for_agents = 1)
    let r = f.integrate(&p, "user:alice");
    assert_eq!(r.state, IntentState::Blocked);
    f.repo.review(&p, &id("agent:reviewer"), "approve", "bot says fine").unwrap();
    assert_eq!(f.integrate(&p, "user:alice").state, IntentState::Blocked, "agent approvals do not count by default");
    f.repo.review(&p, &id("user:carol"), "approve", "human says fine").unwrap();
    assert!(matches!(f.integrate(&p, "user:alice").outcome, Outcome::Integrated { .. }));
}

#[test]
fn agent_autonomy_envelope_and_provenance() {
    let f = fx();
    let dir = tempfile::tempdir().unwrap();
    let keys = KeyStore::open(dir.path().join("keys")).unwrap();
    registry::register_agent(f.repo.meta(), &keys, "docbot", "agent", "user:admin").unwrap();
    f.repo
        .set_policy(
            "main",
            r#"{"agent_autonomy":{"auto_integrate":true,"allowed_paths":["docs/**"],"max_files":3},
            "require_provenance_for_agents":true}"#,
            &id("user:alice"),
        )
        .unwrap();
    let base = f.land(None, &[("src/lib.rs", "fn main() {}"), ("docs/a.md", "a")]);

    let run = registry::start_run(f.repo.meta(), "docbot", None).unwrap();
    let who = run.identity.as_str();

    // inside the envelope: docs only, but provenance is mandatory
    let c = f.snap(Some(base), &[("src/lib.rs", "fn main() {}"), ("docs/a.md", "a!")], who);
    let p = f.propose(c, "docs tweak", who);
    let r = f.integrate(&p, who);
    assert_eq!(r.state, IntentState::Blocked);
    assert_eq!(blocked(&r).1[0].code, "missing_provenance");

    let mut rec = ProvenanceRecord::new(c, run.identity.clone(), f.clock.now_ts());
    rec.model = Some("example-model".into());
    rec.task_id = None;
    provenance::attach(f.repo.meta(), &keys, "docbot", &rec).unwrap();
    let r = f.integrate(&p, who);
    assert!(
        matches!(r.outcome, Outcome::Integrated { .. }),
        "autonomous docs change lands without approval: {:?}",
        r.outcome
    );

    // outside the envelope (touches src/): the agent may not integrate, and needs a human
    let head = f.head().unwrap();
    let c2 = f.snap(Some(head), &[("src/lib.rs", "fn main() { evil() }"), ("docs/a.md", "a!")], who);
    let p2 = f.propose(c2, "refactor", who);
    let mut rec2 = ProvenanceRecord::new(c2, run.identity.clone(), f.clock.now_ts());
    rec2.model = Some("example-model".into());
    provenance::attach(f.repo.meta(), &keys, "docbot", &rec2).unwrap();
    let r = f.integrate(&p2, who);
    assert!(
        matches!(r.outcome, Outcome::Denied { .. }),
        "the agent itself may not integrate src/ changes: {:?}",
        r.outcome
    );
    let r = f.integrate(&p2, "user:alice");
    assert_eq!(r.state, IntentState::Blocked, "a human integrator still needs a human approval");
    f.repo.review(&p2, &id("user:carol"), "approve", "reviewed").unwrap();
    assert!(matches!(f.integrate(&p2, "user:alice").outcome, Outcome::Integrated { .. }));
}

#[test]
fn exclusive_leases_gate_integration_with_fencing_tokens() {
    let f = fx();
    let base = f.land(None, &[("src/a.rs", "1"), ("docs/x.md", "x")]);
    let l = leases::acquire(f.repo.meta(), "src/**", "agent:a#run_1", true, 600_000, None).unwrap().lease;

    // b is not the lease holder: blocked even though policy is satisfied
    let cb = f.snap(Some(base), &[("src/a.rs", "b-was-here"), ("docs/x.md", "x")], "agent:b#run_2");
    let pb = f.propose(cb, "b", "agent:b#run_2");
    f.repo.review(&pb, &id("user:carol"), "approve", "").unwrap();
    let r = f.integrate(&pb, "user:alice");
    assert_eq!(r.state, IntentState::Blocked);
    assert_eq!(blocked(&r).1[0].code, "lease");

    // a's change needs the token
    let ca = f.snap(Some(base), &[("src/a.rs", "a-was-here"), ("docs/x.md", "x")], "agent:a#run_1");
    let pa = f.propose(ca, "a", "agent:a#run_1");
    f.repo.review(&pa, &id("user:carol"), "approve", "").unwrap();
    let no_token = f
        .repo
        .integrate(&IntegrateRequest {
            proposal_id: pa.clone(),
            actor: id("user:alice"),
            idempotency_key: None,
            fencing_tokens: vec![],
        })
        .unwrap();
    assert_eq!(no_token.state, IntentState::Blocked);
    let with = f
        .repo
        .integrate(&IntegrateRequest {
            proposal_id: pa.clone(),
            actor: id("user:alice"),
            idempotency_key: None,
            fencing_tokens: vec![l.token],
        })
        .unwrap();
    assert!(matches!(with.outcome, Outcome::Integrated { .. }), "{:?}", with.outcome);

    // a's lease expires and is re-granted: b may now proceed, a's old token is worthless
    f.clock.advance_ms(700_000);
    let l2 = leases::acquire(f.repo.meta(), "src/**", "agent:b#run_2", true, 600_000, None).unwrap().lease;
    assert!(l2.token > l.token);
    let r = f
        .repo
        .integrate(&IntegrateRequest {
            proposal_id: pb.clone(),
            actor: id("user:alice"),
            idempotency_key: None,
            fencing_tokens: vec![l2.token],
        })
        .unwrap();
    // b's change conflicts textually with a's (both edit src/a.rs from the same base) — the lease gate passed
    assert!(
        matches!(r.outcome, Outcome::Conflict { .. }),
        "lease satisfied; the merge itself conflicts: {:?}",
        r.outcome
    );
}

#[test]
fn policy_changes_are_versioned_and_need_an_admin() {
    let f = fx();
    f.repo.set_policy("main", r#"{"admins":["user:root"]}"#, &id("user:alice")).unwrap();
    assert!(f.repo.set_policy("main", "{}", &id("user:mallory")).is_err(), "not an admin");
    assert!(f.repo.set_policy("main", "{}", &id("agent:bot")).is_err());
    let v2 = f.repo.set_policy("main", r#"{"admins":["user:root"],"approvals":{"min":2}}"#, &id("user:root")).unwrap();
    assert_eq!(v2.version, 2);
    assert_eq!(f.repo.policy_history("main").unwrap().len(), 2);
    assert!(f.repo.set_policy("main", r#"{"approvlas":{}}"#, &id("user:root")).is_err(), "typos are rejected");
    assert!(f.repo.set_policy("nope", "{}", &id("user:root")).is_err());
    // every change is in the audit trail
    let audit = f.repo.meta().read(|tx| tx.audit_list(Some("main"), 10)).unwrap();
    assert_eq!(audit.iter().filter(|a| a.action == "policy.set").count(), 2);
}

#[test]
fn setting_the_same_policy_again_is_a_no_op_that_keeps_the_attestations_valid() {
    let f = fx();
    let doc = policy_json("");
    let first = f.repo.set_policy("main", &doc, &id("user:alice")).unwrap();
    let audit = |f: &Fx| {
        let entries = f.repo.meta().read(|tx| tx.audit_list(Some("main"), 100)).unwrap();
        entries.iter().filter(|e| e.action == "policy.set").count()
    };
    assert_eq!(audit(&f), 1);

    // a retry after a lost response, or a policy-as-code loop: the same document, even spelled differently
    let again = f.repo.set_policy("main", &doc, &id("user:alice")).unwrap();
    assert_eq!((again.version, &again.doc_hash), (first.version, &first.doc_hash));
    let respelled = format!("\n  {}  \n", doc.replace("\"tests\"", "\"tests\" "));
    assert_eq!(f.repo.set_policy("main", &respelled, &id("user:alice")).unwrap().version, first.version);
    assert_eq!(audit(&f), 1, "nothing was audited for the repeats");
    assert_eq!(f.repo.policy_history("main").unwrap().len(), 1, "no version was added");

    // so the attestation made under that version still counts
    let c = f.snap(None, &[("a", "1")], "user:bob");
    let p = f.propose(c, "t", "user:bob");
    let (tree, _) = blocked(&f.integrate(&p, "user:alice"));
    f.repo.submit_attestation(&attest(&f, tree, CheckResult::Pass, first.version, &ci_key(), "service:ci")).unwrap();
    f.repo.set_policy("main", &doc, &id("user:alice")).unwrap();
    assert!(matches!(f.integrate(&p, "user:alice").outcome, Outcome::Integrated { .. }));

    // a different document is a new version, and the old attestations stop counting for it
    let changed = f.repo.set_policy("main", &policy_json(r#","max_changed_files":100"#), &id("user:alice")).unwrap();
    assert_eq!(changed.version, first.version + 1);
    assert_eq!(audit(&f), 2);
}

#[test]
fn updating_a_proposal_rejects_a_stale_inspected_head() {
    let f = fx();
    let first = f.snap(None, &[("a", "first")], "user:bob");
    let proposal = f.propose(first, "change", "user:bob");
    let second = f.snap(Some(first), &[("a", "second")], "user:bob");
    let stale_edit = f.snap(Some(first), &[("a", "stale edit")], "user:bob");
    f.repo.update_proposal_at_head(&proposal, &second, &id("user:bob"), Some(&first)).unwrap();
    let result = f.repo.update_proposal_at_head(&proposal, &stale_edit, &id("user:bob"), Some(&first));
    assert!(matches!(result, Err(ag_integrate::Error::Conflict(_))), "stale update: {result:?}");
    assert_eq!(f.repo.find_proposal(&proposal).unwrap().head, second);
}

#[test]
fn concurrent_proposal_updates_have_one_winner() {
    let f = fx();
    let first = f.snap(None, &[("a", "first")], "user:bob");
    let proposal = f.propose(first, "change", "user:bob");
    let heads = [f.snap(Some(first), &[("a", "left")], "user:bob"), f.snap(Some(first), &[("a", "right")], "user:bob")];
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = heads
            .iter()
            .map(|head| {
                scope.spawn(|| {
                    barrier.wait();
                    f.repo.update_proposal_at_head(&proposal, head, &id("user:bob"), Some(&first))
                })
            })
            .collect();
        handles.into_iter().map(|handle| handle.join().unwrap()).collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| matches!(result, Err(ag_integrate::Error::Conflict(_)))).count(), 1);
    let winner = results.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(f.repo.find_proposal(&proposal).unwrap().head, winner.head);
}

fn discussion(f: &Fx, proposal: &str, head: ag_types::ChangesetId, name: &str) {
    use ag_storage::meta::{ReviewThreadAnchorRow, ReviewThreadRow};
    f.repo
        .meta()
        .write(|tx| {
            tx.insert_review_thread(&ReviewThreadRow {
                id: name.into(),
                proposal: proposal.into(),
                anchor: ReviewThreadAnchorRow {
                    base: None,
                    head,
                    path: "a".into(),
                    side: "new".into(),
                    line: 1,
                    source_text: "first".into(),
                },
                body: "Please explain this".into(),
                author: "user:carol".into(),
                resolved: false,
                version: 1,
                created_at_ms: 0,
                updated_at_ms: 0,
            })
        })
        .unwrap();
}

#[test]
fn required_discussions_include_earlier_heads_and_block_until_resolved() {
    let f = fx();
    f.repo.set_policy("main", r#"{"require_resolved_threads":true}"#, &id("user:alice")).unwrap();
    let first = f.snap(None, &[("a", "first")], "user:bob");
    let proposal = f.propose(first, "change", "user:bob");
    discussion(&f, &proposal, first, "thread_old");
    let next = f.snap(Some(first), &[("a", "next")], "user:bob");
    f.repo.update_proposal(&proposal, &next, &id("user:bob")).unwrap();
    let result = f.integrate(&proposal, "user:alice");
    let (_, blockers) = blocked(&result);
    assert_eq!(blockers[0].code, "unresolved_threads");
    assert_eq!(blockers[0].detail["count"], 1);
    assert_eq!(f.head(), None);
    f.repo.meta().write(|tx| tx.resolve_review_thread("thread_old", 1, true)).unwrap();
    assert_eq!(f.integrate(&proposal, "user:alice").state, IntentState::Integrated);
}

#[test]
fn required_discussions_also_block_already_integrated_completion() {
    let f = fx();
    let first = f.land(None, &[("a", "first")]);
    f.repo.set_policy("main", r#"{"require_resolved_threads":true}"#, &id("user:alice")).unwrap();
    let proposal = f.propose(first, "already included", "user:bob");
    discussion(&f, &proposal, first, "thread_contained");
    let result = f.integrate(&proposal, "user:alice");
    assert_eq!(blocked(&result).1[0].code, "unresolved_threads");
    let preview = f.repo.prepare(&proposal, &id("user:alice"), &[]).unwrap();
    assert_eq!(preview.blockers[0].code, "unresolved_threads");
    assert_eq!(f.repo.find_proposal(&proposal).unwrap().state, "open");
    f.repo.meta().write(|tx| tx.resolve_review_thread("thread_contained", 1, true)).unwrap();
    assert_eq!(f.integrate(&proposal, "user:alice").state, IntentState::Integrated);
}

#[test]
fn a_reopened_discussion_is_rechecked_inside_the_integration_transaction() {
    let f = fx();
    f.repo.set_policy("main", r#"{"require_resolved_threads":true}"#, &id("user:alice")).unwrap();
    let first = f.snap(None, &[("a", "first")], "user:bob");
    let proposal = f.propose(first, "change", "user:bob");
    discussion(&f, &proposal, first, "thread_race");
    f.repo.meta().write(|tx| tx.resolve_review_thread("thread_race", 1, true)).unwrap();
    let gate = f.repo.failpoints().arm_gate("integrate.before_commit");
    let result = std::thread::scope(|scope| {
        let repo = &f.repo;
        let request = IntegrateRequest {
            proposal_id: proposal.clone(),
            actor: id("user:alice"),
            idempotency_key: None,
            fencing_tokens: vec![],
        };
        let worker = scope.spawn(move || repo.integrate(&request).unwrap());
        assert!(gate.wait_reached(std::time::Duration::from_secs(5)));
        f.repo.meta().write(|tx| tx.resolve_review_thread("thread_race", 2, false)).unwrap();
        gate.release();
        worker.join().unwrap()
    });
    assert_eq!(blocked(&result).1[0].code, "unresolved_threads");
    assert_eq!(f.head(), None);
}
