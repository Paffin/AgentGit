//! The presets through the real evaluator: what each one says it does is what [`ag_policy::evaluate`] decides.
#![allow(missing_docs)]

use ag_crypto::KeyPair;
use ag_policy::explain::{Effect, explain};
use ag_policy::presets::{CHECK, Preset, PresetOptions};
use ag_policy::{
    ApprovalFact, Attestation, ChangedPath, CheckResult, Decision, Evaluation, Facts, Policy, TrustedIssuer, Unmet,
    evaluate,
};
use ag_types::{ChangesetId, Identity, ObjectId, Timestamp, TreeId};

fn id(s: &str) -> Identity {
    Identity::new(s).unwrap()
}

fn head() -> ChangesetId {
    ChangesetId::new(ObjectId::from_bytes([1; 32]))
}

fn tree() -> TreeId {
    TreeId::new(ObjectId::from_bytes([2; 32]))
}

fn ci_key() -> KeyPair {
    KeyPair::from_seed([7; 32])
}

fn build(p: Preset, with_ci: bool) -> Policy {
    let ci = with_ci.then(|| TrustedIssuer { identity: id("service:ci"), public_key: ci_key().public() });
    p.build(&PresetOptions { admins: vec!["user:root".into()], ci }).unwrap()
}

struct Case<'a> {
    policy: &'a Policy,
    actor: &'a str,
    author: &'a str,
    paths: &'a [&'a str],
    approvers: &'a [&'a str],
    provenance: bool,
    passed: bool,
}

impl Case<'_> {
    fn eval(&self) -> Evaluation {
        let changed: Vec<ChangedPath> = self.paths.iter().map(|p| ChangedPath { path: (*p).into() }).collect();
        let approvals: Vec<ApprovalFact> =
            self.approvers.iter().map(|r| ApprovalFact { reviewer: id(r), head: head(), approve: true }).collect();
        let atts = if self.passed {
            vec![
                Attestation::sign(
                    &ci_key(),
                    "att_1".into(),
                    tree(),
                    CHECK.into(),
                    CheckResult::Pass,
                    1,
                    "env".into(),
                    id("service:ci"),
                    Timestamp::from_millis(1_000_000),
                )
                .unwrap(),
            ]
        } else {
            vec![]
        };
        evaluate(
            self.policy,
            &Facts {
                actor: &id(self.actor),
                author: &id(self.author),
                proposal_head: head(),
                candidate_tree: tree(),
                policy_version: 1,
                changed: &changed,
                attestations: &atts,
                approvals: &approvals,
                has_provenance: self.provenance,
                unresolved_threads: 0,
                now: Timestamp::from_millis(1_000_100),
            },
        )
    }
}

fn case<'a>(policy: &'a Policy, actor: &'a str, author: &'a str, paths: &'a [&'a str]) -> Case<'a> {
    Case { policy, actor, author, paths, approvers: &[], provenance: false, passed: false }
}

fn codes(e: &Evaluation) -> Vec<&'static str> {
    e.unmet.iter().map(Unmet::code).collect()
}

#[test]
fn solo_lets_a_person_land_alone_and_makes_an_agent_wait_for_one() {
    let p = build(Preset::Solo, false);
    assert!(case(&p, "user:me", "user:me", &["src/a.rs"]).eval().is_allowed());
    // An agent's change needs a person's approval, and the agent cannot land it.
    let agent = "agent:bot#run_1";
    let e = case(&p, "user:me", agent, &["src/a.rs"]).eval();
    assert_eq!(codes(&e), ["insufficient_approvals"]);
    let e = Case { approvers: &["user:me"], ..case(&p, "user:me", agent, &["src/a.rs"]) }.eval();
    assert!(e.is_allowed(), "{:?}", e.unmet);
    assert!(codes(&case(&p, agent, agent, &["docs/a.md"]).eval()).contains(&"actor_not_allowed"));
}

#[test]
fn team_needs_someone_else_to_approve() {
    let p = build(Preset::Team, false);
    assert_eq!(codes(&case(&p, "user:alice", "user:alice", &["a"]).eval()), ["insufficient_approvals"]);
    let own = Case { approvers: &["user:alice"], ..case(&p, "user:alice", "user:alice", &["a"]) }.eval();
    assert_eq!(codes(&own), ["insufficient_approvals"], "an author's own approval does not count");
    let ok = Case { approvers: &["user:bob"], ..case(&p, "user:alice", "user:alice", &["a"]) }.eval();
    assert!(ok.is_allowed(), "{:?}", ok.unmet);
}

#[test]
fn a_ci_issuer_adds_the_required_check_to_every_preset_that_takes_one() {
    for preset in [Preset::Solo, Preset::Team, Preset::Agents, Preset::Strict] {
        let p = build(preset, true);
        let mut c = case(&p, "user:alice", "user:alice", &["a"]);
        c.approvers = &["user:bob", "user:carol"];
        let e = c.eval();
        assert!(codes(&e).contains(&"missing_validation"), "{}: {:?}", preset.name(), e.unmet);
        c.passed = true;
        let e = c.eval();
        assert!(e.is_allowed(), "{}: {:?}", preset.name(), e.unmet);
    }
}

#[test]
fn agents_land_documentation_alone_and_everything_else_goes_to_a_person() {
    let p = build(Preset::Agents, true);
    let bot = "agent:docs-bot#run_1";
    // Inside the envelope: documentation, signed provenance, a passing check, no approval: the agent lands it.
    let inside = Case { provenance: true, passed: true, ..case(&p, bot, bot, &["docs/a.md", "README.md"]) }.eval();
    assert!(inside.is_allowed(), "{:?}", inside.unmet);
    // Without provenance it is blocked, not refused.
    let e = Case { passed: true, ..case(&p, bot, bot, &["docs/a.md"]) }.eval();
    assert_eq!(codes(&e), ["missing_provenance"]);
    assert_eq!(e.decision, Decision::Blocked);
    // Code is outside the envelope: the agent cannot land it ...
    let code = Case { provenance: true, passed: true, ..case(&p, bot, bot, &["src/a.rs"]) };
    assert!(codes(&code.eval()).contains(&"actor_not_allowed"));
    // ... and a person must approve it before a person lands it.
    let by_person = Case { actor: "user:alice", ..code };
    assert_eq!(codes(&by_person.eval()), ["insufficient_approvals"]);
    let approved = Case { approvers: &["user:alice"], ..by_person };
    assert!(approved.eval().is_allowed());
}

#[test]
fn agents_cannot_slip_a_sensitive_file_into_a_documentation_change() {
    let p = build(Preset::Agents, false);
    let bot = "agent:docs-bot#run_1";
    let e = Case { provenance: true, ..case(&p, bot, bot, &["docs/a.md", ".github/workflows/ci.yml"]) }.eval();
    assert!(codes(&e).contains(&"actor_not_allowed"), "{:?}", e.unmet);
    // More than the envelope's file limit also leaves it.
    let many: Vec<String> = (0..11).map(|i| format!("docs/{i}.md")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let e = Case { provenance: true, ..case(&p, bot, bot, &many) }.eval();
    assert!(codes(&e).contains(&"actor_not_allowed"), "{:?}", e.unmet);
}

#[test]
fn people_land_their_own_changes_under_the_agents_preset_but_protected_paths_need_a_second_pair_of_eyes() {
    let p = build(Preset::Agents, false);
    assert!(case(&p, "user:alice", "user:alice", &["src/a.rs"]).eval().is_allowed());
    let sensitive = case(&p, "user:alice", "user:alice", &["SECURITY.md"]);
    assert_eq!(codes(&sensitive.eval()), ["insufficient_approvals"]);
    let ok = Case { approvers: &["user:bob"], ..sensitive };
    assert!(ok.eval().is_allowed());
}

#[test]
fn strict_needs_two_approvals_three_for_sensitive_paths_and_never_lets_an_agent_land() {
    let p = build(Preset::Strict, true);
    let base = Case { passed: true, provenance: true, ..case(&p, "user:alice", "user:alice", &["src/a.rs"]) };
    let one = Case { approvers: &["user:bob"], ..base };
    assert_eq!(codes(&one.eval()), ["insufficient_approvals"]);
    let two = Case { approvers: &["user:bob", "user:carol"], ..one };
    assert!(two.eval().is_allowed());
    let sensitive = Case { paths: &["deploy/prod.yaml"], ..two };
    assert_eq!(codes(&sensitive.eval()), ["insufficient_approvals"], "protected paths need three");
    let three = Case { approvers: &["user:bob", "user:carol", "user:dave"], ..sensitive };
    assert!(three.eval().is_allowed());
    let bot = "agent:b#run_1";
    let agent = Case { actor: bot, author: bot, paths: &["docs/a.md"], ..three };
    assert!(codes(&agent.eval()).contains(&"actor_not_allowed"));
}

#[test]
fn what_the_explanation_says_about_agents_is_what_the_evaluator_does() {
    for preset in Preset::ALL {
        let p = build(preset, true);
        let says_agents_land_alone = explain(&p)
            .iter()
            .any(|s| s.effect == Effect::Allows && s.text.contains("An agent may land such a change itself"));
        let bot = "agent:b#run_1";
        let e = Case { provenance: true, passed: true, ..case(&p, bot, bot, &["docs/a.md"]) }.eval();
        let lands_alone = !codes(&e).contains(&"actor_not_allowed");
        assert_eq!(says_agents_land_alone, lands_alone, "{}", preset.name());
    }
}

#[test]
fn presets_exclude_services_from_integrating_and_authoring_team_changes() {
    for preset in Preset::ALL {
        let p = build(preset, true);
        let service = Case {
            passed: true,
            provenance: true,
            approvers: &["user:bob", "user:carol"],
            ..case(&p, "service:ci", "user:alice", &["src/a.rs"])
        };
        assert!(codes(&service.eval()).contains(&"actor_not_allowed"), "{}", preset.name());
        if preset != Preset::Solo {
            let author = Case { actor: "user:alice", author: "service:ci", ..service };
            assert!(codes(&author.eval()).contains(&"author_not_allowed"), "{}", preset.name());
        }
    }
}

#[test]
fn configured_file_limits_and_strict_provenance_are_enforced() {
    for (preset, limit) in [(Preset::Agents, 500), (Preset::Strict, 200)] {
        let p = build(preset, true);
        let paths: Vec<String> = (0..=limit).map(|i| format!("src/{i}.rs")).collect();
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let base = Case {
            passed: true,
            provenance: true,
            approvers: &["user:bob", "user:carol"],
            ..case(&p, "user:alice", "agent:bot#run_1", &paths[..limit])
        };
        assert!(base.eval().is_allowed(), "{}", preset.name());
        let oversized = Case { paths: &paths, ..base };
        assert!(codes(&oversized.eval()).contains(&"too_many_changed_files"), "{}", preset.name());
        let unsigned = Case { provenance: false, paths: &paths[..1], ..oversized };
        assert!(codes(&unsigned.eval()).contains(&"missing_provenance"), "{}", preset.name());
    }
}

#[test]
fn preset_audiences_are_distinct_and_descriptive() {
    assert_eq!(Preset::Solo.audience(), "one person, with or without agents helping");
    assert_eq!(Preset::Team.audience(), "a team of people who review each other's work");
    assert_eq!(Preset::Agents.audience(), "people and AI agents working on the same code");
    assert_eq!(Preset::Strict.audience(), "code where every change needs two reviewers and a passing check");
}
