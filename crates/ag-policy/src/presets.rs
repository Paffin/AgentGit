//! Policy presets: four ready-made starting points as **data**, built into ordinary [`Policy`] documents.
//!
//! A preset is not a mode and not a template with holes. [`Preset::build`] returns the same [`Policy`] that
//! `ag policy set` would store, parsed and validated by the same code, so a stored preset is indistinguishable
//! from a hand-written document and can be edited afterwards. What a preset *means* is read back from the
//! document by [`crate::explain`], never written down a second time.
//!
//! What a preset cannot know it takes as [`PresetOptions`]: who administers the stream, and which CI issuer (an
//! identity and an Ed25519 public key) is trusted to sign the `tests` check.

use ag_types::Identity;

use crate::document::TrustedIssuer;
use crate::document::{AgentAutonomy, ApprovalRules, Policy, PolicyError, ProtectedPath, RequiredValidation};

/// The name of the check a preset requires when a CI issuer is given.
pub const CHECK: &str = "tests";

/// The presets, from the most relaxed to the most demanding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preset {
    /// One person, perhaps with agents helping: nothing slows a person down, an agent's change needs a person.
    Solo,
    /// Several people: every change needs another person's approval.
    Team,
    /// People and agents together: agents are reviewed, checked and fenced into an autonomy envelope.
    Agents,
    /// Regulated or high-stakes: two approvals, a passing check on every change, tight control of sensitive paths.
    Strict,
}

/// What a preset needs from the caller and does not know itself.
#[derive(Clone, Debug, Default)]
pub struct PresetOptions {
    /// Identity patterns allowed to change the policy and close other people's proposals. Empty: any user (the
    /// built-in rule). The presets that care about who administers a stream (`team`, `agents`, `strict`) pass it on.
    pub admins: Vec<String>,
    /// The CI issuer trusted to sign the `tests` check. With one, the preset requires that check; without one,
    /// `solo`, `team` and `agents` simply do not require any check, and `strict` cannot be built.
    pub ci: Option<TrustedIssuer>,
}

/// Why a preset could not be built.
#[derive(Debug, thiserror::Error)]
pub enum PresetError {
    /// No preset of that name.
    #[error("unknown preset '{0}'; the presets are: {1}")]
    Unknown(String, String),
    /// The preset requires a check and so a trusted issuer.
    #[error(
        "the preset '{0}' requires a passing check on every change, so it needs a trusted CI issuer (an identity and \
         its public key)"
    )]
    NeedsIssuer(&'static str),
    /// An administrator pattern is not an identity pattern.
    #[error("invalid administrator '{0}': {1}")]
    BadAdmin(String, String),
    /// The built document did not pass the parser's validation (a bug in the preset, caught by its tests).
    #[error(transparent)]
    Policy(#[from] PolicyError),
}

impl Preset {
    /// Every preset, in the order `ag policy preset list` shows them.
    pub const ALL: [Preset; 4] = [Preset::Solo, Preset::Team, Preset::Agents, Preset::Strict];

    /// The name used on the command line and in the API.
    pub fn name(self) -> &'static str {
        match self {
            Preset::Solo => "solo",
            Preset::Team => "team",
            Preset::Agents => "agents",
            Preset::Strict => "strict",
        }
    }

    /// The names of all presets.
    pub fn names() -> Vec<&'static str> {
        Self::ALL.iter().map(|p| p.name()).collect()
    }

    /// Finds a preset by name.
    pub fn parse(name: &str) -> Result<Self, PresetError> {
        Self::ALL
            .iter()
            .copied()
            .find(|p| p.name() == name)
            .ok_or_else(|| PresetError::Unknown(name.to_string(), Self::names().join(", ")))
    }

    /// One line on who the preset is for. (What it allows and denies is [`crate::explain`]'s job, from the document.)
    pub fn audience(self) -> &'static str {
        match self {
            Preset::Solo => "one person, with or without agents helping",
            Preset::Team => "a team of people who review each other's work",
            Preset::Agents => "people and AI agents working on the same code",
            Preset::Strict => "code where every change needs two reviewers and a passing check",
        }
    }

    /// Whether the preset cannot be built without a trusted CI issuer.
    pub fn requires_ci(self) -> bool {
        matches!(self, Preset::Strict)
    }

    /// Builds the policy document.
    pub fn build(self, options: &PresetOptions) -> Result<Policy, PresetError> {
        if self.requires_ci() && options.ci.is_none() {
            return Err(PresetError::NeedsIssuer(self.name()));
        }
        for a in &options.admins {
            // Only the patterns' syntax is checked here; `Policy::validate` compiles them below.
            if a.is_empty() || a.contains(char::is_whitespace) {
                return Err(PresetError::BadAdmin(a.clone(), "an identity pattern such as user:alice".into()));
            }
        }
        let mut p = match self {
            Preset::Solo => solo(),
            Preset::Team => team(),
            Preset::Agents => agents(),
            Preset::Strict => strict(),
        };
        if self != Preset::Solo {
            p.admins = options.admins.clone();
        }
        if let Some(ci) = &options.ci {
            p.trusted_issuers = vec![ci.clone()];
            p.required_validations = vec![RequiredValidation {
                check: CHECK.into(),
                issuers: vec![ci.identity.clone()],
                environment_digest: None,
                max_age_secs: (self == Preset::Strict).then_some(24 * 3600),
                run: None,
            }];
        }
        p.validate()?;
        Ok(p)
    }
}

/// Patterns of what an organisation usually protects. Matched against changed paths, so only the paths that exist
/// in a repository ever cause a rule to apply.
const SENSITIVE: [&str; 4] = [".github/**", "SECURITY.md", "deploy/**", "LICENSE"];

fn protected(min_approvals: u32) -> Vec<ProtectedPath> {
    SENSITIVE.iter().map(|g| ProtectedPath { glob: (*g).into(), min_approvals, approvers: Vec::new() }).collect()
}

fn people() -> Vec<String> {
    vec!["user:*".into()]
}

fn solo() -> Policy {
    Policy {
        integrators: people(),
        // A person's own change needs nobody; an agent's change needs a person (never the agent itself).
        approvals: ApprovalRules { min: 0, min_for_agents: 1, approvers: people(), forbid_self_approval: true },
        ..Policy::default()
    }
}

fn team() -> Policy {
    Policy {
        integrators: people(),
        allowed_authors: vec!["user:*".into(), "agent:*".into()],
        approvals: ApprovalRules { min: 1, min_for_agents: 1, approvers: people(), forbid_self_approval: true },
        ..Policy::default()
    }
}

fn agents() -> Policy {
    Policy {
        integrators: people(),
        allowed_authors: vec!["user:*".into(), "agent:*".into()],
        // A person's change lands at once; an agent's change must be reviewed by a person unless it stays inside
        // the envelope below.
        approvals: ApprovalRules { min: 0, min_for_agents: 1, approvers: people(), forbid_self_approval: true },
        protected_paths: protected(1),
        agent_autonomy: AgentAutonomy {
            auto_integrate: true,
            allowed_paths: vec!["docs/**".into(), "**/*.md".into()],
            max_files: 10,
        },
        require_provenance_for_agents: true,
        max_changed_files: 500,
        ..Policy::default()
    }
}

fn strict() -> Policy {
    Policy {
        integrators: people(),
        allowed_authors: vec!["user:*".into(), "agent:*".into()],
        approvals: ApprovalRules { min: 2, min_for_agents: 2, approvers: people(), forbid_self_approval: true },
        protected_paths: protected(3),
        // No envelope: an agent never lands anything by itself.
        agent_autonomy: AgentAutonomy::default(),
        require_provenance_for_agents: true,
        max_changed_files: 200,
        ..Policy::default()
    }
}

/// Parses `IDENTITY=PUBLIC_KEY_HEX` (for example `service:ci=3f2a…`, 64 hex digits) into a trusted issuer.
pub fn parse_issuer(text: &str) -> Result<TrustedIssuer, String> {
    let (who, key) = text.split_once('=').ok_or_else(|| {
        format!(
            "'{text}' is not IDENTITY=PUBLIC_KEY, for example service:ci=<64 hex digits from `ag validate keygen ci`>"
        )
    })?;
    let identity = Identity::new(who.trim()).map_err(|e| format!("invalid issuer identity '{who}': {e}"))?;
    let public_key =
        ag_crypto::PublicKey::from_hex(key.trim()).map_err(|e| format!("invalid public key (64 hex digits): {e}"))?;
    Ok(TrustedIssuer { identity, public_key })
}

#[cfg(test)]
mod tests {
    use ag_crypto::KeyPair;

    use super::*;

    fn ci() -> TrustedIssuer {
        TrustedIssuer {
            identity: Identity::new("service:ci").unwrap(),
            public_key: KeyPair::from_seed([4; 32]).public(),
        }
    }

    #[test]
    fn names_round_trip_and_unknown_names_list_the_choices() {
        for p in Preset::ALL {
            assert_eq!(Preset::parse(p.name()).unwrap(), p);
        }
        let e = Preset::parse("lax").unwrap_err().to_string();
        assert!(e.contains("solo, team, agents, strict"), "{e}");
    }

    #[test]
    fn every_preset_is_accepted_by_the_real_parser_with_and_without_a_ci_issuer() {
        for p in Preset::ALL {
            for with_ci in [false, true] {
                let options = PresetOptions { admins: vec!["user:alice".into()], ci: with_ci.then(ci) };
                match p.build(&options) {
                    Ok(policy) => {
                        // What is stored and read back is the same document.
                        let text = policy.to_json();
                        assert_eq!(Policy::parse(&text).unwrap(), policy, "{} (ci: {with_ci})", p.name());
                        assert_eq!(policy.required_validations.len(), usize::from(with_ci));
                    }
                    Err(PresetError::NeedsIssuer(name)) => {
                        assert!(!with_ci && p.requires_ci() && name == p.name());
                    }
                    Err(e) => panic!("{} (ci: {with_ci}): {e}", p.name()),
                }
            }
        }
    }

    #[test]
    fn the_presets_differ_and_none_is_the_built_in_default() {
        let options = PresetOptions { admins: vec![], ci: Some(ci()) };
        let docs: Vec<Policy> = Preset::ALL.iter().map(|p| p.build(&options).unwrap()).collect();
        for (i, a) in docs.iter().enumerate() {
            assert_ne!(*a, Policy::default(), "{} configures nothing", Preset::ALL[i].name());
            for b in &docs[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn strict_without_an_issuer_says_what_it_needs() {
        let e = Preset::Strict.build(&PresetOptions::default()).unwrap_err().to_string();
        assert!(e.contains("strict") && e.contains("trusted CI issuer"), "{e}");
    }

    #[test]
    fn administrators_are_carried_into_every_preset_but_solo() {
        let options = PresetOptions { admins: vec!["user:alice".into()], ci: Some(ci()) };
        for p in [Preset::Team, Preset::Agents, Preset::Strict] {
            assert_eq!(p.build(&options).unwrap().admins, vec!["user:alice".to_string()], "{}", p.name());
        }
        assert!(Preset::Solo.build(&options).unwrap().admins.is_empty(), "a one-person stream has no admin list");
        let bad = PresetOptions { admins: vec!["has space".into()], ci: None };
        assert!(matches!(Preset::Team.build(&bad), Err(PresetError::BadAdmin(..))));
    }

    #[test]
    fn issuers_are_parsed_from_identity_equals_key() {
        let key = KeyPair::from_seed([4; 32]).public().to_hex();
        let i = parse_issuer(&format!("service:ci={key}")).unwrap();
        assert_eq!(i, ci());
        for bad in ["service:ci", "ci=00", &format!("nobody={key}"), "service:ci=zz"] {
            assert!(parse_issuer(bad).is_err(), "{bad}");
        }
    }
}
