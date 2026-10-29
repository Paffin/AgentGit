use std::collections::BTreeMap;

use ag_crypto::PublicKey;
use ag_types::Identity;
use serde::{Deserialize, Serialize};

use crate::matcher::{IdentityMatcher, PathMatcher};

/// Errors from parsing or validating a policy document.
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    /// The JSON is malformed or has unknown fields.
    #[error("invalid policy document: {0}")]
    Parse(#[from] serde_json::Error),
    /// A value in the document is not acceptable.
    #[error("invalid policy: {0}")]
    Invalid(String),
}

/// An issuer whose attestations are trusted, pinned to a public key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedIssuer {
    /// Issuer identity, e.g. `service:ci`.
    pub identity: Identity,
    /// Its Ed25519 public key.
    pub public_key: PublicKey,
}

fn default_timeout_secs() -> u32 {
    600
}

/// How a trusted execution-plane worker runs a check.
///
/// This lives in the **policy** (set by administrators, versioned, audited),
/// never in the repository under test: the code being checked must not get to
/// choose the command that checks it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSpec {
    /// Command and arguments, executed without a shell.
    pub argv: Vec<String>,
    /// Wall-clock limit.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u32,
    /// Memory limit in MiB (0 = worker default).
    #[serde(default)]
    pub memory_mb: u32,
    /// CPU-time limit in seconds (0 = worker default).
    #[serde(default)]
    pub cpu_secs: u32,
    /// Process-count limit (0 = worker default).
    #[serde(default)]
    pub max_pids: u32,
    /// Whether the check may use the network. Off unless stated.
    #[serde(default)]
    pub network: bool,
    /// Pinned container image for OCI workers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Non-secret environment variables for the check.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl RunSpec {
    fn validate(&self, check: &str) -> Result<(), PolicyError> {
        let bad = |m: String| PolicyError::Invalid(format!("validation '{check}': {m}"));
        if self.argv.is_empty() || self.argv[0].is_empty() {
            return Err(bad("run.argv must name a command".into()));
        }
        if self.argv.iter().any(|a| a.contains('\0')) {
            return Err(bad("run.argv must not contain NUL bytes".into()));
        }
        if self.timeout_secs == 0 || self.timeout_secs > 24 * 3600 {
            return Err(bad("run.timeout_secs must be between 1 and 86400".into()));
        }
        for (k, v) in &self.env {
            let name_ok = !k.is_empty()
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !k.chars().next().is_some_and(|c| c.is_ascii_digit());
            if !name_ok || v.contains('\0') {
                return Err(bad(format!("run.env has an invalid entry '{k}'")));
            }
        }
        Ok(())
    }
}

/// A check that must have passed on the exact candidate tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredValidation {
    /// Check name, e.g. `tests`.
    pub check: String,
    /// If set, only these issuer identities count for this check.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issuers: Vec<Identity>,
    /// If set, the attestation must carry exactly this environment digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_digest: Option<String>,
    /// If set, attestations older than this many seconds do not count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_secs: Option<u64>,
    /// How a trusted worker runs this check. Without it the check can only be
    /// satisfied by an attestation submitted by a trusted issuer on its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunSpec>,
}

/// Rules about human (and agent) approvals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ApprovalRules {
    /// Approvals required for every integration.
    pub min: u32,
    /// Approvals required when the author is an agent and the change is
    /// outside the agent's autonomy limits.
    pub min_for_agents: u32,
    /// Identity patterns allowed to approve (empty = any `user:` identity).
    pub approvers: Vec<String>,
    /// Whether an author's own approval is ignored.
    pub forbid_self_approval: bool,
}

impl Default for ApprovalRules {
    fn default() -> Self {
        Self { min: 0, min_for_agents: 1, approvers: Vec::new(), forbid_self_approval: true }
    }
}

/// Extra approvals for sensitive paths (security policy, CI config, ...).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedPath {
    /// Glob of protected paths.
    pub glob: String,
    /// Approvals required when a change touches a matching path.
    pub min_approvals: u32,
    /// Identity patterns allowed to approve this rule (empty = same as global).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approvers: Vec<String>,
}

/// What agents may integrate without human approval.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AgentAutonomy {
    /// Whether agents may run the integration step themselves.
    pub auto_integrate: bool,
    /// Path globs agents may change without approval. Empty = none.
    pub allowed_paths: Vec<String>,
    /// Maximum number of changed files for an autonomous change.
    pub max_files: u32,
}

/// A stream policy document (stored versioned, evaluated by [`crate::evaluate`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Policy {
    /// Identity patterns allowed to run integration (empty = any non-agent).
    pub integrators: Vec<String>,
    /// Identity patterns allowed to change policy and administer (empty = any `user:`).
    pub admins: Vec<String>,
    /// Identity patterns allowed to author proposals (empty = anyone).
    pub allowed_authors: Vec<String>,
    /// Issuers whose attestations are trusted.
    pub trusted_issuers: Vec<TrustedIssuer>,
    /// Checks that must pass on the candidate tree.
    pub required_validations: Vec<RequiredValidation>,
    /// Approval rules.
    pub approvals: ApprovalRules,
    /// Extra rules for protected paths.
    pub protected_paths: Vec<ProtectedPath>,
    /// Limits for autonomous agent integration.
    pub agent_autonomy: AgentAutonomy,
    /// Whether agent-authored changes must carry signed provenance.
    pub require_provenance_for_agents: bool,
    /// Whether every line discussion must be resolved before integration.
    #[serde(skip_serializing_if = "is_false")]
    pub require_resolved_threads: bool,
    /// Maximum number of files one integration may change (0 = unlimited).
    pub max_changed_files: u32,
}

impl Default for Policy {
    /// The permissive local default: no requirements beyond "agents do not
    /// integrate themselves".
    fn default() -> Self {
        Self {
            integrators: Vec::new(),
            admins: Vec::new(),
            allowed_authors: Vec::new(),
            trusted_issuers: Vec::new(),
            required_validations: Vec::new(),
            approvals: ApprovalRules::default(),
            protected_paths: Vec::new(),
            agent_autonomy: AgentAutonomy::default(),
            require_provenance_for_agents: false,
            require_resolved_threads: false,
            max_changed_files: 0,
        }
    }
}

impl Policy {
    /// Parses and validates a JSON policy document. Unknown fields are
    /// rejected so a typo cannot silently weaken a policy.
    pub fn parse(json: &str) -> Result<Self, PolicyError> {
        let p: Policy = serde_json::from_str(json)?;
        p.validate()?;
        Ok(p)
    }

    /// Canonical JSON form (what gets stored and hashed).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("policy serializes")
    }

    /// Checks that every glob compiles and every reference resolves.
    pub fn validate(&self) -> Result<(), PolicyError> {
        let bad = |m: String| PolicyError::Invalid(m);
        IdentityMatcher::new(&self.integrators).map_err(bad)?;
        IdentityMatcher::new(&self.admins).map_err(bad)?;
        IdentityMatcher::new(&self.allowed_authors).map_err(bad)?;
        IdentityMatcher::new(&self.approvals.approvers).map_err(bad)?;
        PathMatcher::new(&self.agent_autonomy.allowed_paths).map_err(bad)?;
        for p in &self.protected_paths {
            PathMatcher::new(std::slice::from_ref(&p.glob)).map_err(bad)?;
            IdentityMatcher::new(&p.approvers).map_err(bad)?;
            if p.min_approvals == 0 {
                return Err(bad(format!("protected path '{}' needs min_approvals >= 1", p.glob)));
            }
        }
        for v in &self.required_validations {
            if v.check.is_empty() {
                return Err(bad("required validation has an empty check name".into()));
            }
            if self.trusted_issuers.is_empty() {
                return Err(bad(format!(
                    "validation '{}' is required but trusted_issuers is empty, so it could never be satisfied",
                    v.check
                )));
            }
            if let Some(run) = &v.run {
                run.validate(&v.check)?;
            }
            for i in &v.issuers {
                if !self.trusted_issuers.iter().any(|t| &t.identity == i) {
                    return Err(bad(format!(
                        "validation '{}' names issuer {i} which is not in trusted_issuers",
                        v.check
                    )));
                }
            }
        }
        if self.agent_autonomy.auto_integrate && self.agent_autonomy.allowed_paths.is_empty() {
            return Err(bad("agent_autonomy.auto_integrate requires non-empty allowed_paths".into()));
        }
        Ok(())
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrips() {
        let p = Policy::default();
        assert_eq!(Policy::parse(&p.to_json()).unwrap(), p);
        assert_eq!(Policy::parse("{}").unwrap(), p);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(matches!(Policy::parse(r#"{"approvals":{"mn":2}}"#), Err(PolicyError::Parse(_))));
        assert!(matches!(Policy::parse(r#"{"requird_validations":[]}"#), Err(PolicyError::Parse(_))));
    }

    #[test]
    fn run_specs_are_validated_and_round_trip() {
        let spec: super::RunSpec = serde_json::from_str(r#"{"argv":["cargo","test"],"env":{"CI":"1"}}"#).unwrap();
        assert_eq!(spec.timeout_secs, 600, "default timeout");
        assert!(!spec.network, "network is off unless stated");
        assert!(spec.validate("t").is_ok());
        assert!(serde_json::from_str::<super::RunSpec>(r#"{"argv":["x"],"shell":true}"#).is_err());
        for bad in [
            r#"{"argv":[]}"#,
            r#"{"argv":[""]}"#,
            r#"{"argv":["x"],"timeout_secs":0}"#,
            r#"{"argv":["x"],"env":{"1BAD":"v"}}"#,
            r#"{"argv":["x"],"env":{"A B":"v"}}"#,
        ] {
            let s: super::RunSpec = serde_json::from_str(bad).unwrap();
            assert!(s.validate("t").is_err(), "{bad}");
        }
    }

    #[test]
    fn a_timeout_is_between_one_second_and_one_day() {
        for (timeout, ok) in [(1u32, true), (600, true), (86_400, true), (86_401, false), (0, false), (u32::MAX, false)]
        {
            let spec: super::RunSpec =
                serde_json::from_str(&format!(r#"{{"argv":["x"],"timeout_secs":{timeout}}}"#)).unwrap();
            assert_eq!(spec.validate("t").is_ok(), ok, "timeout_secs {timeout}");
        }
    }

    #[test]
    fn unsatisfiable_policies_are_rejected() {
        let no_issuers = r#"{"required_validations":[{"check":"tests"}]}"#;
        assert!(matches!(Policy::parse(no_issuers), Err(PolicyError::Invalid(_))));
        let zero = r#"{"protected_paths":[{"glob":"x/**","min_approvals":0}]}"#;
        assert!(matches!(Policy::parse(zero), Err(PolicyError::Invalid(_))));
        let auto = r#"{"agent_autonomy":{"auto_integrate":true}}"#;
        assert!(matches!(Policy::parse(auto), Err(PolicyError::Invalid(_))));
        let badglob = r#"{"protected_paths":[{"glob":"[","min_approvals":1}]}"#;
        assert!(matches!(Policy::parse(badglob), Err(PolicyError::Invalid(_))));
    }
}
