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
