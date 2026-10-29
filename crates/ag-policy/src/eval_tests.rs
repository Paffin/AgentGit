use ag_crypto::KeyPair;
use ag_types::ObjectId;

use super::*;
use crate::document::{AgentAutonomy, ApprovalRules, ProtectedPath, TrustedIssuer};

fn id(s: &str) -> Identity {
    Identity::new(s).unwrap()
}

fn cs(n: u8) -> ChangesetId {
    ChangesetId::new(ObjectId::from_bytes([n; 32]))
}

fn tree(n: u8) -> TreeId {
    TreeId::new(ObjectId::from_bytes([n; 32]))
}

fn ci_key() -> KeyPair {
    KeyPair::from_seed([9; 32])
}

fn attest(key: &KeyPair, who: &str, t: u8, check: &str, result: CheckResult, pv: u64, at: i64) -> Attestation {
    Attestation::sign(
        key,
        format!("att_{t}_{check}"),
        tree(t),
        check.into(),
        result,
        pv,
        "env1".into(),
        id(who),
        Timestamp::from_millis(at),
    )
    .unwrap()
}

fn policy_with_tests() -> Policy {
    Policy {
        trusted_issuers: vec![TrustedIssuer { identity: id("service:ci"), public_key: ci_key().public() }],
        required_validations: vec![RequiredValidation {
            check: "tests".into(),
            issuers: vec![],
            environment_digest: None,
            max_age_secs: None,
            run: None,
        }],
        ..Policy::default()
    }
}

struct Ctx {
    actor: Identity,
    author: Identity,
    changed: Vec<ChangedPath>,
    atts: Vec<Attestation>,
    approvals: Vec<ApprovalFact>,
    provenance: bool,
}

impl Ctx {
    fn new() -> Self {
        Self {
            actor: id("user:alice"),
            author: id("user:bob"),
            changed: vec![ChangedPath { path: "src/lib.rs".into() }],
            atts: vec![],
            approvals: vec![],
            provenance: false,
        }
    }

    fn eval(&self, p: &Policy) -> Evaluation {
        evaluate(
            p,
            &Facts {
                actor: &self.actor,
                author: &self.author,
                proposal_head: cs(1),
                candidate_tree: tree(1),
                policy_version: 3,
                changed: &self.changed,
                attestations: &self.atts,
                approvals: &self.approvals,
                has_provenance: self.provenance,
                unresolved_threads: 0,
                now: Timestamp::from_millis(1_000_000),
            },
        )
    }
}

fn approve(who: &str, head: u8) -> ApprovalFact {
    ApprovalFact { reviewer: id(who), head: cs(head), approve: true }
}

#[test]
fn empty_policy_allows_a_human() {
    let e = Ctx::new().eval(&Policy::default());
    assert!(e.is_allowed(), "{:?}", e.unmet);
}

#[test]
fn agents_cannot_integrate_by_default() {
    let mut c = Ctx::new();
    c.actor = id("agent:bot#run_1");
    let e = c.eval(&Policy::default());
    assert_eq!(e.decision, Decision::Denied);
    assert!(matches!(e.unmet[0], Unmet::ActorNotAllowed { .. }));
}

#[test]
fn missing_validation_blocks_and_valid_one_allows() {
    let p = policy_with_tests();
    let mut c = Ctx::new();
    assert_eq!(c.eval(&p).decision, Decision::Blocked);
    c.atts.push(attest(&ci_key(), "service:ci", 1, "tests", CheckResult::Pass, 3, 999_000));
    let e = c.eval(&p);
    assert!(e.is_allowed(), "{:?}", e.unmet);
}

#[test]
fn failed_validation_denies_even_if_a_pass_exists() {
    let p = policy_with_tests();
    let mut c = Ctx::new();
    c.atts.push(attest(&ci_key(), "service:ci", 1, "tests", CheckResult::Pass, 3, 999_000));
    c.atts.push(attest(&ci_key(), "service:ci", 1, "tests", CheckResult::Fail, 3, 999_500));
    // distinct ids so both exist
    c.atts[1].id = "att_fail".into();
    c.atts[1] = Attestation::sign(
        &ci_key(),
        "att_fail".into(),
        tree(1),
        "tests".into(),
        CheckResult::Fail,
        3,
        "env1".into(),
        id("service:ci"),
        Timestamp::from_millis(999_500),
    )
    .unwrap();
    assert_eq!(c.eval(&p).decision, Decision::Denied);
}

#[test]
fn self_signed_agent_attestations_are_worthless() {
    let p = policy_with_tests();
    let mut c = Ctx::new();
    let agent_key = KeyPair::from_seed([5; 32]);
    // valid signature, but the key is not a trusted issuer
    c.atts.push(attest(&agent_key, "service:ci", 1, "tests", CheckResult::Pass, 3, 999_000));
    // trusted identity name but wrong key
    c.atts.push(attest(&agent_key, "agent:bot", 1, "tests", CheckResult::Pass, 3, 999_000));
    assert_eq!(c.eval(&p).decision, Decision::Blocked);
}

#[test]
fn attestation_is_bound_to_tree_policy_version_and_signature() {
    let p = policy_with_tests();
    let mut c = Ctx::new();
    c.atts.push(attest(&ci_key(), "service:ci", 2, "tests", CheckResult::Pass, 3, 999_000)); // other tree
    c.atts.push(attest(&ci_key(), "service:ci", 1, "tests", CheckResult::Pass, 2, 999_000)); // old policy
    assert_eq!(c.eval(&p).decision, Decision::Blocked);

    // tampering with a field after signing invalidates it
    let mut forged = attest(&ci_key(), "service:ci", 2, "tests", CheckResult::Pass, 3, 999_000);
    forged.tree_id = tree(1);
    c.atts = vec![forged];
    assert_eq!(c.eval(&p).decision, Decision::Blocked);
}

#[test]
fn environment_digest_and_age_constraints() {
    let mut p = policy_with_tests();
    p.required_validations[0].environment_digest = Some("env-other".into());
    let mut c = Ctx::new();
    // issued 900 s before `now` (1_000_000 ms)
    c.atts.push(attest(&ci_key(), "service:ci", 1, "tests", CheckResult::Pass, 3, 100_000));
    assert_eq!(c.eval(&p).decision, Decision::Blocked, "environment differs");

    let mut p = policy_with_tests();
    p.required_validations[0].max_age_secs = Some(10);
    assert_eq!(c.eval(&p).decision, Decision::Blocked, "attestation 900s old, max 10s");
    p.required_validations[0].max_age_secs = Some(3600);
    assert!(c.eval(&p).is_allowed());

    // an attestation dated far in the future is rejected
    let mut c2 = Ctx::new();
    c2.atts.push(attest(&ci_key(), "service:ci", 1, "tests", CheckResult::Pass, 3, 1_000_000 + 3_600_000));
    assert_eq!(c2.eval(&policy_with_tests()).decision, Decision::Blocked);
}

/// Found by the `policy_eval` fuzz target: `now - issued_at` overflowed for an attestation timestamp near
/// `i64::MIN` (a panic in debug builds, a wrapped age in release) - and it was computed before the signature
/// was checked, so a forged attestation naming a trusted issuer could trigger it.
#[test]
fn extreme_timestamps_are_blocked_not_a_panic() {
    let p = policy_with_tests();
    let mut aged = policy_with_tests();
    aged.required_validations[0].max_age_secs = Some(3600);
    for at in [i64::MIN, i64::MIN + 1, i64::MAX, i64::MAX - 1] {
        let mut c = Ctx::new();
        c.atts.push(attest(&ci_key(), "service:ci", 1, "tests", CheckResult::Pass, 3, at));
        let far_future = at > 0;
        // Dated after `now` + skew: refused. Dated at the beginning of time: valid unless the policy limits
        // the age, in which case it is far too old.
        assert_eq!(c.eval(&p).is_allowed(), !far_future, "issued_at {at}");
        assert!(!c.eval(&aged).is_allowed(), "issued_at {at} with a one-hour limit");
        // ... and the same when the evaluation clock itself is extreme.
        let mut facts_now = Ctx::new();
        facts_now.atts.push(attest(&ci_key(), "service:ci", 1, "tests", CheckResult::Pass, 3, 0));
        let far = Facts {
            actor: &facts_now.actor,
            author: &facts_now.author,
            proposal_head: cs(1),
            candidate_tree: tree(1),
            policy_version: 3,
            changed: &facts_now.changed,
            attestations: &facts_now.atts,
            approvals: &facts_now.approvals,
            has_provenance: false,
            unresolved_threads: 0,
            now: Timestamp::from_millis(at),
        };
        let _ = evaluate(&p, &far);
    }
}

/// A `max_age_secs` above `i64::MAX` used to wrap to a negative number of milliseconds, which made *every*
/// attestation "too old" (fail closed, but the opposite of what a very large limit means).
#[test]
fn a_huge_max_age_means_no_limit() {
    let mut p = policy_with_tests();
    let mut c = Ctx::new();
    c.atts.push(attest(&ci_key(), "service:ci", 1, "tests", CheckResult::Pass, 3, 100_000));
    for max in [u64::MAX, i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX / 1000] {
        p.required_validations[0].max_age_secs = Some(max);
        assert!(c.eval(&p).is_allowed(), "max_age_secs = {max}");
    }
}

fn approval_policy(min: u32) -> Policy {
    Policy { approvals: ApprovalRules { min, ..ApprovalRules::default() }, ..Policy::default() }
}

#[test]
fn approvals_must_be_for_the_current_head_and_not_self() {
    let p = approval_policy(1);
    let mut c = Ctx::new();
    assert_eq!(c.eval(&p).decision, Decision::Blocked);
    c.approvals.push(approve("user:bob", 1)); // self-approval (bob is the author)
    assert_eq!(c.eval(&p).decision, Decision::Blocked);
    c.approvals.push(approve("user:carol", 9)); // stale head
    assert_eq!(c.eval(&p).decision, Decision::Blocked);
    c.approvals.push(approve("user:carol", 1));
    assert!(c.eval(&p).is_allowed());
}

#[test]
fn one_person_cannot_approve_twice_via_runs() {
    let p = approval_policy(2);
    let mut c = Ctx::new();
    c.approvals.push(approve("user:carol", 1));
    c.approvals.push(approve("user:carol", 1));
    let e = c.eval(&p);
    assert!(matches!(e.unmet[0], Unmet::InsufficientApprovals { have: 1, required: 2, .. }));
    c.approvals.push(approve("user:dave", 1));
    assert!(c.eval(&p).is_allowed());
}

#[test]
fn latest_review_wins_and_changes_requested_blocks() {
    let p = approval_policy(1);
    let mut c = Ctx::new();
    c.approvals.push(approve("user:carol", 1));
    c.approvals.push(ApprovalFact { reviewer: id("user:carol"), head: cs(1), approve: false });
    let e = c.eval(&p);
    assert_eq!(e.decision, Decision::Blocked);
    assert!(e.unmet.iter().any(|u| matches!(u, Unmet::ChangesRequested { .. })));
    c.approvals.push(approve("user:carol", 1));
    assert!(c.eval(&p).is_allowed());
}

#[test]
fn agents_are_not_default_approvers() {
    let p = approval_policy(1);
    let mut c = Ctx::new();
    c.approvals.push(approve("agent:reviewer-bot", 1));
    assert_eq!(c.eval(&p).decision, Decision::Blocked);

    let mut p2 = approval_policy(1);
    p2.approvals.approvers = vec!["agent:reviewer-*".into(), "user:*".into()];
    assert!(c.eval(&p2).is_allowed());
}

#[test]
fn protected_paths_need_extra_approvals() {
    let mut p = approval_policy(1);
    p.protected_paths =
        vec![ProtectedPath { glob: ".github/**".into(), min_approvals: 2, approvers: vec!["user:admin*".into()] }];
    let mut c = Ctx::new();
    c.changed = vec![ChangedPath { path: ".github/workflows/ci.yml".into() }];
    c.approvals.push(approve("user:carol", 1));
    let e = c.eval(&p);
    assert_eq!(e.decision, Decision::Blocked);
    assert!(e.unmet.iter().any(|u| matches!(u, Unmet::InsufficientApprovals { scope, .. } if scope == ".github/**")));
    c.approvals.push(approve("user:admin1", 1));
    assert_eq!(c.eval(&p).decision, Decision::Blocked, "only one eligible protected-path approver");
    c.approvals.push(approve("user:admin2", 1));
    assert!(c.eval(&p).is_allowed());

    // a change elsewhere is unaffected
    let mut c2 = Ctx::new();
    c2.approvals.push(approve("user:carol", 1));
    assert!(c2.eval(&p).is_allowed());
}

fn autonomy_policy() -> Policy {
    Policy {
        agent_autonomy: AgentAutonomy { auto_integrate: true, allowed_paths: vec!["docs/**".into()], max_files: 3 },
        approvals: ApprovalRules { min: 0, min_for_agents: 1, ..ApprovalRules::default() },
        ..Policy::default()
    }
}

#[test]
fn agent_authored_change_needs_a_human_unless_within_autonomy() {
    let p = autonomy_policy();
    let mut c = Ctx::new();
    c.author = id("agent:docbot#run_1");
    c.changed = vec![ChangedPath { path: "src/main.rs".into() }];
    c.approvals.push(approve("agent:other", 1));
    let e = c.eval(&p);
    assert_eq!(e.decision, Decision::Blocked, "outside allowed paths -> human approval needed, agents don't count");
    c.approvals.push(approve("user:carol", 1));
    assert!(c.eval(&p).is_allowed());

    // inside the autonomy envelope: no approval needed
    let mut c2 = Ctx::new();
    c2.author = id("agent:docbot#run_1");
    c2.changed = vec![ChangedPath { path: "docs/a.md".into() }, ChangedPath { path: "docs/b.md".into() }];
    assert!(c2.eval(&p).is_allowed());

    // too many files leaves the envelope
    c2.changed = (0..4).map(|i| ChangedPath { path: format!("docs/{i}.md") }).collect();
    assert_eq!(c2.eval(&p).decision, Decision::Blocked);
}

#[test]
fn agent_may_integrate_only_inside_autonomy() {
    let p = autonomy_policy();
    let mut c = Ctx::new();
    c.actor = id("agent:docbot#run_1");
    c.author = id("agent:docbot#run_1");
    c.changed = vec![ChangedPath { path: "docs/a.md".into() }];
    assert!(c.eval(&p).is_allowed());
    c.changed = vec![ChangedPath { path: "src/a.rs".into() }];
    assert_eq!(c.eval(&p).decision, Decision::Denied);
}

#[test]
fn protected_paths_leave_the_autonomy_envelope() {
    let mut p = autonomy_policy();
    p.protected_paths = vec![ProtectedPath { glob: "docs/security/**".into(), min_approvals: 1, approvers: vec![] }];
    let mut c = Ctx::new();
    c.actor = id("agent:docbot#run_1");
    c.author = id("agent:docbot#run_1");
    c.changed = vec![ChangedPath { path: "docs/security/policy.md".into() }];
    assert_eq!(c.eval(&p).decision, Decision::Denied);
}

#[test]
fn provenance_and_size_limits() {
    let p = Policy { require_provenance_for_agents: true, max_changed_files: 2, ..Policy::default() };
    let mut c = Ctx::new();
    c.author = id("agent:bot");
    c.approvals.push(approve("user:carol", 1));
    let e = c.eval(&p);
    assert!(e.unmet.contains(&Unmet::MissingProvenance));
    c.provenance = true;
    assert!(c.eval(&p).is_allowed());
    c.changed = (0..3).map(|i| ChangedPath { path: format!("f{i}") }).collect();
    assert_eq!(c.eval(&p).decision, Decision::Denied);
}

#[test]
fn integrator_and_author_allow_lists() {
    let p =
        Policy { integrators: vec!["user:alice".into()], allowed_authors: vec!["user:*".into()], ..Policy::default() };
    let mut c = Ctx::new();
    assert!(c.eval(&p).is_allowed());
    c.actor = id("user:mallory");
    assert_eq!(c.eval(&p).decision, Decision::Denied);
    let mut c = Ctx::new();
    c.author = id("agent:bot");
    assert_eq!(c.eval(&p).decision, Decision::Denied);
}

#[test]
fn only_an_attestation_for_the_required_check_counts() {
    let p = policy_with_tests();
    let mut c = Ctx::new();
    // trusted, signed, for this tree and policy version - but for another check
    c.atts.push(attest(&ci_key(), "service:ci", 1, "lint", CheckResult::Pass, 3, 999_000));
    let e = c.eval(&p);
    assert_eq!(e.decision, Decision::Blocked);
    assert_eq!(e.unmet, vec![Unmet::MissingValidation { check: "tests".into() }]);
    // a failure of another check is not a failure of this one
    c.atts.push(attest(&ci_key(), "service:ci", 1, "tests", CheckResult::Pass, 3, 999_000));
    c.atts.push(attest(&ci_key(), "service:ci", 1, "lint", CheckResult::Fail, 3, 999_000));
    assert!(c.eval(&p).is_allowed());
}

#[test]
fn a_validation_can_be_limited_to_named_issuers() {
    let other_key = KeyPair::from_seed([7; 32]);
    let mut p = policy_with_tests();
    p.trusted_issuers.push(TrustedIssuer { identity: id("service:other"), public_key: other_key.public() });
    p.required_validations[0].issuers = vec![id("service:ci")];
    let mut c = Ctx::new();
    // trusted, but not one of the issuers this check accepts
    c.atts.push(attest(&other_key, "service:other", 1, "tests", CheckResult::Pass, 3, 999_000));
    assert_eq!(c.eval(&p).decision, Decision::Blocked);
    c.atts.push(attest(&ci_key(), "service:ci", 1, "tests", CheckResult::Pass, 3, 999_000));
    assert!(c.eval(&p).is_allowed());
    // without the restriction either issuer is enough
    p.required_validations[0].issuers.clear();
    c.atts.remove(1);
    assert!(c.eval(&p).is_allowed());
}

/// `now` is 1_000_000 ms in every [`Ctx`].
#[test]
fn the_clock_skew_and_the_age_limit_are_inclusive() {
    let allowed = |policy: &Policy, issued_at: i64| {
        let mut c = Ctx::new();
        c.atts.push(attest(&ci_key(), "service:ci", 1, "tests", CheckResult::Pass, 3, issued_at));
        c.eval(policy).is_allowed()
    };
    // five minutes ahead of the evaluation clock is tolerated, one millisecond more is not
    let p = policy_with_tests();
    assert!(allowed(&p, 1_000_000 + 300_000));
    assert!(!allowed(&p, 1_000_000 + 300_001));
    // exactly as old as the limit is fine, one millisecond older is not
    let mut aged = policy_with_tests();
    aged.required_validations[0].max_age_secs = Some(900);
    assert!(allowed(&aged, 100_000));
    assert!(!allowed(&aged, 99_999));
}

#[test]
fn the_changed_file_limit_is_inclusive_and_zero_means_no_limit() {
    let changed = |n: usize| (0..n).map(|i| ChangedPath { path: format!("f{i}") }).collect::<Vec<_>>();
    let limited = Policy { max_changed_files: 2, ..Policy::default() };
    let mut c = Ctx::new();
    c.changed = changed(2);
    assert!(c.eval(&limited).is_allowed());
    c.changed = changed(3);
    let e = c.eval(&limited);
    assert_eq!(e.decision, Decision::Denied);
    assert_eq!(e.unmet, vec![Unmet::TooManyChangedFiles { changed: 3, max: 2 }]);
    c.changed = changed(500);
    assert!(c.eval(&Policy::default()).is_allowed());
}

#[test]
fn what_was_satisfied_is_listed_and_nothing_else() {
    let e = Ctx::new().eval(&Policy::default());
    assert!(e.unmet.is_empty());
    assert_eq!(e.satisfied, vec!["actor user:alice may integrate".to_string()], "no approvals were required");
    let mut c = Ctx::new();
    c.approvals.push(approve("user:carol", 1));
    let e = c.eval(&approval_policy(1));
    assert_eq!(
        e.satisfied,
        vec!["actor user:alice may integrate".to_string(), "1 approval(s) of 1 required".to_string()]
    );
}

#[test]
fn an_envelope_with_no_room_or_no_paths_contains_nothing_not_even_an_empty_change() {
    let agent = id("agent:docbot#run_1");
    // max_files 0: "nothing is ever inside the envelope", so the agent may not integrate
    let no_room = Policy {
        agent_autonomy: AgentAutonomy { auto_integrate: true, allowed_paths: vec!["docs/**".into()], max_files: 0 },
        ..Policy::default()
    };
    let mut c = Ctx::new();
    c.actor = agent.clone();
    c.author = agent.clone();
    c.changed = vec![];
    assert_eq!(c.eval(&no_room).decision, Decision::Denied);
    // no allowed paths: an agent-authored change still needs the human approval
    let no_paths = Policy {
        agent_autonomy: AgentAutonomy { auto_integrate: false, allowed_paths: vec![], max_files: 3 },
        approvals: ApprovalRules { min: 0, min_for_agents: 1, ..ApprovalRules::default() },
        ..Policy::default()
    };
    let mut c = Ctx::new();
    c.author = agent;
    c.changed = vec![];
    assert_eq!(c.eval(&no_paths).decision, Decision::Blocked);
    c.approvals.push(approve("user:carol", 1));
    assert!(c.eval(&no_paths).is_allowed());
}

#[test]
fn every_unmet_requirement_reads_as_a_sentence_and_the_denials_are_the_ones_that_cannot_wait() {
    let cases = [
        (Unmet::ActorNotAllowed { reason: "no".into() }, "actor not allowed to integrate: no", true),
        (
            Unmet::AuthorNotAllowed { author: "agent:x".into() },
            "author agent:x is not allowed to propose to this stream",
            true,
        ),
        (
            Unmet::FailedValidation { check: "tests".into(), attestation: "att_1".into() },
            "check 'tests' FAILED on this exact tree (attestation att_1)",
            true,
        ),
        (
            Unmet::MissingValidation { check: "tests".into() },
            "check 'tests' has no trusted passing attestation for this exact tree",
            false,
        ),
        (
            Unmet::InsufficientApprovals { scope: "default".into(), required: 2, have: 1 },
            "approvals (default): 1 of 2 required",
            false,
        ),
        (Unmet::ChangesRequested { reviewer: "user:carol".into() }, "user:carol requested changes", false),
        (Unmet::UnresolvedThreads { count: 2 }, "2 unresolved line discussion(s) must be resolved", false),
        (Unmet::MissingProvenance, "agent-authored change has no signed provenance", false),
        (Unmet::TooManyChangedFiles { changed: 5, max: 3 }, "5 files changed, policy allows at most 3", true),
    ];
    for (unmet, text, denial) in cases {
        assert_eq!(unmet.to_string(), text);
        assert_eq!(unmet.is_denial(), denial, "{text}");
    }
}

#[test]
fn evaluation_is_deterministic() {
    let p = policy_with_tests();
    let mut c = Ctx::new();
    c.atts.push(attest(&ci_key(), "service:ci", 1, "tests", CheckResult::Pass, 3, 999_000));
    let a = serde_json::to_string(&c.eval(&p)).unwrap();
    let b = serde_json::to_string(&c.eval(&p)).unwrap();
    assert_eq!(a, b);
}

#[test]
fn every_unmet_requirement_has_a_fixed_wire_code_equal_to_its_serde_tag() {
    let all = [
        Unmet::ActorNotAllowed { reason: "r".into() },
        Unmet::AuthorNotAllowed { author: "a".into() },
        Unmet::FailedValidation { check: "c".into(), attestation: "x".into() },
        Unmet::MissingValidation { check: "c".into() },
        Unmet::InsufficientApprovals { scope: "default".into(), required: 2, have: 1 },
        Unmet::ChangesRequested { reviewer: "u".into() },
        Unmet::UnresolvedThreads { count: 2 },
        Unmet::MissingProvenance,
        Unmet::TooManyChangedFiles { changed: 5, max: 3 },
    ];
    let mut seen = std::collections::BTreeSet::new();
    for u in &all {
        let tag = serde_json::to_value(u).unwrap()["kind"].as_str().unwrap().to_string();
        assert_eq!(u.code(), tag, "{u:?}");
        assert!(seen.insert(u.code()), "codes are distinct");
    }
    assert_eq!(seen.len(), 9);
    let mut listed: Vec<&str> = Unmet::CODES.to_vec();
    listed.sort_unstable();
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), listed, "Unmet::CODES lists exactly the codes of the variants");
}

#[test]
fn required_discussions_block_until_every_thread_is_resolved() {
    let actor = id("user:alice");
    let author = id("user:bob");
    let mut facts = Facts {
        actor: &actor,
        author: &author,
        proposal_head: cs(1),
        candidate_tree: tree(1),
        policy_version: 1,
        changed: &[],
        attestations: &[],
        approvals: &[],
        has_provenance: false,
        unresolved_threads: 2,
        now: Timestamp::from_millis(1),
    };
    let mut policy = Policy::default();
    assert!(evaluate(&policy, &facts).is_allowed());
    policy.require_resolved_threads = true;
    let blocked = evaluate(&policy, &facts);
    assert_eq!(blocked.decision, Decision::Blocked);
    assert!(blocked.unmet.iter().any(|item| item.code() == "unresolved_threads"));
    facts.unresolved_threads = 0;
    assert!(evaluate(&policy, &facts).is_allowed());
}

#[test]
fn disabled_discussion_policy_does_not_change_serialized_legacy_documents() {
    assert!(serde_json::to_value(Policy::default()).unwrap().get("require_resolved_threads").is_none());
    let policy = Policy::parse(r#"{"require_resolved_threads":true}"#).unwrap();
    assert_eq!(serde_json::to_value(policy).unwrap()["require_resolved_threads"], true);
}
