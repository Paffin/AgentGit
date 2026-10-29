//! A policy document in plain language.
//!
//! [`explain`] reads a [`Policy`] and says what it allows, what it requires before a change lands and what it
//! refuses. Every sentence is built from the document's fields, so the text cannot drift from what the evaluator
//! enforces: change a field and the sentence changes with it. The test `every_field_of_the_document_is_explained`
//! fails when a field is added to [`Policy`] without a sentence for it.

use serde::Serialize;

use crate::document::Policy;

/// Which side of the line a sentence is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Someone may do something.
    Allows,
    /// Something must be true before a change lands.
    Requires,
    /// Something is refused.
    Denies,
}

/// One sentence about the policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Statement {
    /// Allows, requires or denies.
    pub effect: Effect,
    /// The field(s) of the document the sentence is read from (for example `approvals`).
    pub field: &'static str,
    /// The sentence.
    pub text: String,
}

/// The top-level fields of a policy document, each with the statements that explain it. Kept next to the code that
/// writes them; a test compares it with the fields the document really has.
pub const EXPLAINED_FIELDS: &[&str] = &[
    "integrators",
    "admins",
    "allowed_authors",
    "trusted_issuers",
    "required_validations",
    "approvals",
    "protected_paths",
    "agent_autonomy",
    "require_provenance_for_agents",
    "require_resolved_threads",
    "max_changed_files",
];

fn list(items: &[String]) -> String {
    items.join(", ")
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn short_key(hex: &str) -> String {
    hex.chars().take(8).collect()
}

fn duration(secs: u64) -> String {
    match secs {
        s if s % 86_400 == 0 => plural((s / 86_400) as u32, "day", "days"),
        s if s % 3_600 == 0 => plural((s / 3_600) as u32, "hour", "hours"),
        s if s % 60 == 0 => plural((s / 60) as u32, "minute", "minutes"),
        s => format!("{s} seconds"),
    }
}

/// Reads a policy as a list of sentences, in the order a person asks the questions: who may propose, who may
/// land, who administers, what must be approved and checked, what is fenced off, what agents may do alone.
pub fn explain(p: &Policy) -> Vec<Statement> {
    let mut out: Vec<Statement> = Vec::new();
    let mut say = |effect, field, text: String| out.push(Statement { effect, field, text });

    // ---- who ----
    if p.allowed_authors.is_empty() {
        say(Effect::Allows, "allowed_authors", "Anyone may propose a change to this stream.".into());
    } else {
        say(
            Effect::Allows,
            "allowed_authors",
            format!("Only {} may propose a change to this stream.", list(&p.allowed_authors)),
        );
        say(Effect::Denies, "allowed_authors", "A proposal from anyone else is refused (`author_not_allowed`).".into());
    }
    if p.integrators.is_empty() {
        say(
            Effect::Allows,
            "integrators",
            "Any person or service may land a change that meets the requirements below.".into(),
        );
    } else {
        say(
            Effect::Allows,
            "integrators",
            format!("Only {} may land a change (`ag integrate`).", list(&p.integrators)),
        );
        say(
            Effect::Denies,
            "integrators",
            "Anyone else who tries to land a change is refused (`actor_not_allowed`).".into(),
        );
    }
    if p.admins.is_empty() {
        say(Effect::Allows, "admins", "Any user may change this policy and close other people's proposals.".into());
    } else {
        say(
            Effect::Allows,
            "admins",
            format!("Only {} may change this policy or close other people's proposals.", list(&p.admins)),
        );
    }

    // ---- approvals ----
    let a = &p.approvals;
    let who = if a.approvers.is_empty() { "a user".to_string() } else { list(&a.approvers) };
    if a.min == 0 {
        say(Effect::Allows, "approvals", "A change by a person needs no approval.".into());
    } else {
        say(
            Effect::Requires,
            "approvals",
            format!(
                "Every change needs {} of the current version of the proposal, from {who}.",
                plural(a.min, "approval", "approvals")
            ),
        );
    }
    let for_agents = a.min.max(a.min_for_agents);
    // The envelope exists when it has room and paths, whether or not agents may also land changes inside it.
    let envelope = p.agent_autonomy.max_files > 0 && !p.agent_autonomy.allowed_paths.is_empty();
    if for_agents > 0 {
        say(
            Effect::Requires,
            "approvals",
            format!(
                "A change by an agent{} needs {} from a person.",
                if envelope { " that goes beyond the autonomy envelope" } else { "" },
                plural(for_agents, "approval", "approvals")
            ),
        );
    } else {
        say(Effect::Allows, "approvals", "A change by an agent needs no approval.".into());
    }
    if envelope && a.min == 0 {
        say(Effect::Allows, "approvals", "Inside the autonomy envelope an agent's change needs no approval.".into());
    }
    // for_agents is max(min, min_for_agents), so it already covers the global minimum.
    if for_agents > 0 {
        if a.forbid_self_approval {
            say(Effect::Denies, "approvals", "An author's own approval does not count.".into());
        } else {
            say(Effect::Allows, "approvals", "An author may approve their own change.".into());
        }
        say(
            Effect::Requires,
            "approvals",
            "An approval counts only for the version it was given on: when the proposal changes, it must be approved again; a reviewer asking for changes blocks the change until they approve (`insufficient_approvals`, `changes_requested`).".into(),
        );
    }

    // ---- checks ----
    if p.required_validations.is_empty() {
        say(Effect::Allows, "required_validations", "No check has to pass before a change lands.".into());
    }
    for v in &p.required_validations {
        let by = if v.issuers.is_empty() {
            "a trusted issuer".to_string()
        } else {
            v.issuers.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
        };
        let mut text =
            format!("The check '{}' must have passed on the exact files being landed, signed by {by}", v.check);
        if let Some(s) = v.max_age_secs {
            text.push_str(&format!(", and no more than {} ago", duration(s)));
        }
        if v.environment_digest.is_some() {
            text.push_str(", in the pinned environment");
        }
        text.push_str(" (`missing_validation`).");
        say(Effect::Requires, "required_validations", text);
        if let Some(run) = &v.run {
            say(
                Effect::Allows,
                "required_validations",
                format!(
                    "Trusted workers run it as `{}` ({}s limit, network {}).",
                    run.argv.join(" "),
                    run.timeout_secs,
                    if run.network { "on" } else { "off" }
                ),
            );
        }
    }
    if !p.required_validations.is_empty() {
        say(
            Effect::Denies,
            "required_validations",
            "A trusted failing result of a required check refuses the change, even if a passing one also exists (`failed_validation`).".into(),
        );
    }
    if p.trusted_issuers.is_empty() {
        say(
            Effect::Denies,
            "trusted_issuers",
            "No issuer is trusted to sign a check result, so none would count (and none is required).".into(),
        );
    } else {
        let issuers: Vec<String> = p
            .trusted_issuers
            .iter()
            .map(|t| format!("{} (key {}…)", t.identity, short_key(&t.public_key.to_hex())))
            .collect();
        say(
            Effect::Allows,
            "trusted_issuers",
            format!(
                "Check results count only from {}; a signature from anyone else, including an agent vouching for itself, is ignored.",
                issuers.join(", ")
            ),
        );
    }

    // ---- protected paths ----
    if p.protected_paths.is_empty() {
        say(Effect::Allows, "protected_paths", "No path needs extra approval.".into());
    }
    for pp in &p.protected_paths {
        let from = if pp.approvers.is_empty() { "eligible reviewers".to_string() } else { list(&pp.approvers) };
        say(
            Effect::Requires,
            "protected_paths",
            format!(
                "A change touching `{}` needs {} from {from} (`insufficient_approvals`).",
                pp.glob,
                plural(pp.min_approvals, "approval", "approvals")
            ),
        );
    }

    // ---- agents ----
    let au = &p.agent_autonomy;
    if envelope {
        let inside = format!(
            "A change by an agent is inside the autonomy envelope when it changes at most {} and only paths matching {}, and touches no protected path.",
            plural(au.max_files, "file", "files"),
            au.allowed_paths.iter().map(|g| format!("`{g}`")).collect::<Vec<_>>().join(", ")
        );
        if au.auto_integrate {
            say(Effect::Allows, "agent_autonomy", format!("{inside} An agent may land such a change itself."));
            say(
                Effect::Denies,
                "agent_autonomy",
                "An agent cannot land a change outside that envelope; it waits for a person (`actor_not_allowed`)."
                    .into(),
            );
        } else {
            say(Effect::Allows, "agent_autonomy", inside);
            say(
                Effect::Denies,
                "agent_autonomy",
                "An agent cannot land a change itself, even inside the envelope; a person does (`actor_not_allowed`)."
                    .into(),
            );
        }
    } else if au.auto_integrate {
        say(
            Effect::Denies,
            "agent_autonomy",
            "Agents may land changes themselves only inside an envelope, but it has no room (a file limit of 0), so nothing is inside it: an agent still cannot land a change itself (`actor_not_allowed`).".into(),
        );
    } else {
        say(
            Effect::Denies,
            "agent_autonomy",
            "An agent cannot land a change itself; a person does (`actor_not_allowed`).".into(),
        );
    }
    if p.require_provenance_for_agents {
        say(
            Effect::Requires,
            "require_provenance_for_agents",
            "A change by an agent must carry signed provenance: who ran it, with which task and tools (`missing_provenance`).".into(),
        );
    } else {
        say(Effect::Allows, "require_provenance_for_agents", "A change by an agent needs no provenance record.".into());
    }

    if p.require_resolved_threads {
        say(Effect::Requires, "require_resolved_threads", "Every line discussion, including earlier revisions, must be resolved before landing (`unresolved_threads`).".into());
    } else {
        say(Effect::Allows, "require_resolved_threads", "Open line discussions do not block landing.".into());
    }

    // ---- limits ----
    if p.max_changed_files == 0 {
        say(
            Effect::Allows,
            "max_changed_files",
            "There is no limit on the number of files one integration may change.".into(),
        );
    } else {
        say(
            Effect::Denies,
            "max_changed_files",
            format!(
                "A change that touches more than {} is refused (`too_many_changed_files`).",
                plural(p.max_changed_files, "file", "files")
            ),
        );
    }
    out
}

/// The statements as text, grouped by effect, for a terminal.
pub fn render(statements: &[Statement]) -> String {
    let mut s = String::new();
    for (effect, heading) in
        [(Effect::Allows, "Allowed"), (Effect::Requires, "Required before a change lands"), (Effect::Denies, "Refused")]
    {
        let lines: Vec<&Statement> = statements.iter().filter(|x| x.effect == effect).collect();
        if lines.is_empty() {
            continue;
        }
        s.push_str(heading);
        s.push_str(":\n");
        for l in lines {
            s.push_str("  - ");
            s.push_str(&l.text);
            s.push('\n');
        }
        s.push('\n');
    }
    s.truncate(s.trim_end().len());
    s
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use ag_crypto::KeyPair;
    use ag_types::Identity;

    use super::*;
    use crate::document::{AgentAutonomy, ApprovalRules, ProtectedPath, RequiredValidation, TrustedIssuer};

    fn text(p: &Policy) -> String {
        explain(p).into_iter().map(|s| s.text).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn every_field_of_the_document_is_explained() {
        // The fields the document really has (its serialized form lists them all) ...
        let value = serde_json::to_value(Policy { require_resolved_threads: true, ..Policy::default() }).unwrap();
        let fields: BTreeSet<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
        let declared: BTreeSet<&str> = EXPLAINED_FIELDS.iter().copied().collect();
        assert_eq!(fields, declared, "a field of Policy has no explanation (or an explanation names a removed field)");
        // ... and the sentences really are about them: a rich policy produces at least one sentence per field.
        let rich = Policy {
            integrators: vec!["user:*".into()],
            admins: vec!["user:root".into()],
            allowed_authors: vec!["user:*".into()],
            trusted_issuers: vec![TrustedIssuer {
                identity: Identity::new("service:ci").unwrap(),
                public_key: KeyPair::from_seed([1; 32]).public(),
            }],
            required_validations: vec![RequiredValidation {
                check: "tests".into(),
                issuers: vec![],
                environment_digest: None,
                max_age_secs: None,
                run: None,
            }],
            approvals: ApprovalRules { min: 1, min_for_agents: 1, approvers: vec![], forbid_self_approval: true },
            protected_paths: vec![ProtectedPath { glob: "x/**".into(), min_approvals: 1, approvers: vec![] }],
            agent_autonomy: AgentAutonomy { auto_integrate: true, allowed_paths: vec!["docs/**".into()], max_files: 3 },
            require_provenance_for_agents: true,
            require_resolved_threads: true,
            max_changed_files: 9,
        };
        let spoken: BTreeSet<&str> = explain(&rich).iter().map(|s| s.field).collect();
        assert_eq!(spoken, declared);
        let default_spoken: BTreeSet<&str> = explain(&Policy::default()).iter().map(|s| s.field).collect();
        assert_eq!(default_spoken, declared, "the default policy is explained field by field, too");
    }

    #[test]
    fn the_sentences_follow_the_numbers_of_the_document() {
        let mut p = Policy {
            approvals: ApprovalRules {
                min: 2,
                min_for_agents: 3,
                approvers: vec!["user:lead*".into()],
                forbid_self_approval: false,
            },
            ..Policy::default()
        };
        let t = text(&p);
        assert!(t.contains("needs 2 approvals"), "{t}");
        assert!(t.contains("from user:lead*"), "{t}");
        assert!(t.contains("agent needs 3 approvals"), "{t}");
        assert!(t.contains("may approve their own change"), "{t}");
        p.approvals.min = 1;
        p.approvals.forbid_self_approval = true;
        let t = text(&p);
        assert!(t.contains("needs 1 approval of"), "{t}");
        assert!(t.contains("own approval does not count"), "{t}");
        p.max_changed_files = 40;
        assert!(text(&p).contains("more than 40 files is refused"));
        p.max_changed_files = 1;
        assert!(text(&p).contains("more than 1 file is refused"));
    }

    #[test]
    fn autonomy_is_described_the_way_the_evaluator_treats_it() {
        let mut p = Policy::default();
        assert!(text(&p).contains("cannot land a change itself"));
        p.agent_autonomy = AgentAutonomy { auto_integrate: true, allowed_paths: vec!["docs/**".into()], max_files: 0 };
        assert!(text(&p).contains("a file limit of 0"), "{}", text(&p));
        p.agent_autonomy.max_files = 5;
        let t = text(&p);
        assert!(t.contains("at most 5 files") && t.contains("`docs/**`"), "{t}");
        assert!(t.contains("An agent may land such a change itself"), "{t}");
        // An envelope without auto_integrate still sets how many approvals a change needs, but agents never land it.
        p.agent_autonomy.auto_integrate = false;
        let t = text(&p);
        assert!(t.contains("even inside the envelope"), "{t}");
        assert!(t.contains("beyond the autonomy envelope needs 1 approval"), "{t}");
        assert!(t.contains("Inside the autonomy envelope an agent's change needs no approval"), "{t}");
    }

    #[test]
    fn checks_name_their_issuers_and_age() {
        let key = KeyPair::from_seed([2; 32]).public();
        let p = Policy {
            trusted_issuers: vec![TrustedIssuer { identity: Identity::new("service:ci").unwrap(), public_key: key }],
            required_validations: vec![RequiredValidation {
                check: "lint".into(),
                issuers: vec![Identity::new("service:ci").unwrap()],
                environment_digest: None,
                max_age_secs: Some(86_400),
                run: None,
            }],
            ..Policy::default()
        };
        let t = text(&p);
        assert!(t.contains("'lint'") && t.contains("service:ci") && t.contains("no more than 1 day ago"), "{t}");
        assert!(t.contains(&key.to_hex()[..8]), "{t}");
    }

    #[test]
    fn rendering_groups_by_effect_and_loses_nothing() {
        let s = explain(&Policy::default());
        let r = render(&s);
        assert!(r.starts_with("Allowed:"), "{r}");
        for st in &s {
            assert!(r.contains(&st.text), "{}", st.text);
        }
    }
    #[test]
    fn validation_age_is_readable_at_each_unit_boundary() {
        for (seconds, expected) in [
            (1, "1 seconds"),
            (59, "59 seconds"),
            (60, "1 minute"),
            (120, "2 minutes"),
            (3599, "3599 seconds"),
            (3600, "1 hour"),
            (7200, "2 hours"),
            (86400, "1 day"),
            (172800, "2 days"),
        ] {
            let p = Policy {
                required_validations: vec![RequiredValidation {
                    check: "tests".into(),
                    issuers: vec![],
                    environment_digest: None,
                    max_age_secs: Some(seconds),
                    run: None,
                }],
                ..Policy::default()
            };
            let explanation = text(&p);
            assert!(explanation.contains(&format!("no more than {expected} ago")), "{explanation}");
        }
    }

    #[test]
    fn zero_approvals_do_not_claim_that_a_review_is_required() {
        for min in [0, 1] {
            for min_for_agents in [0, 1] {
                for envelope in [false, true] {
                    let mut p = Policy::default();
                    p.approvals.min = min;
                    p.approvals.min_for_agents = min_for_agents;
                    if envelope {
                        p.agent_autonomy = AgentAutonomy {
                            auto_integrate: false,
                            max_files: 1,
                            allowed_paths: vec!["docs/**".into()],
                        };
                    }
                    let t = text(&p);
                    let review_required = min > 0 || min_for_agents > 0;
                    assert_eq!(t.contains("An author's own approval does not count"), review_required, "{t}");
                    assert_eq!(t.contains("a reviewer asking for changes"), review_required, "{t}");
                    assert_eq!(
                        t.contains("Inside the autonomy envelope an agent's change needs no approval"),
                        envelope && min == 0,
                        "{t}"
                    );
                    assert_eq!(t.contains("A change by an agent needs no approval."), !review_required, "{t}");
                }
            }
        }
    }

    #[test]
    fn rendering_places_each_statement_under_its_own_heading() {
        let statements = [
            Statement { effect: Effect::Denies, field: "approvals", text: "denied only".into() },
            Statement { effect: Effect::Allows, field: "admins", text: "allowed only".into() },
            Statement { effect: Effect::Requires, field: "approvals", text: "required only".into() },
        ];
        assert_eq!(
            render(&statements),
            "Allowed:\n  - allowed only\n\nRequired before a change lands:\n  - required only\n\nRefused:\n  - denied only"
        );
        assert_eq!(render(&[]), "");
    }

    #[test]
    fn failing_check_warning_requires_a_configured_check() {
        assert!(!text(&Policy::default()).contains("A trusted failing result"));
        let mut p = Policy::default();
        p.required_validations.push(RequiredValidation {
            check: "tests".into(),
            issuers: vec![],
            environment_digest: None,
            max_age_secs: None,
            run: None,
        });
        assert!(text(&p).contains("A trusted failing result"));
    }
}
