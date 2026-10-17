use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Category of an [`Identity`], taken from its prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IdentityKind {
    /// A human user, `user:alice`.
    User,
    /// An autonomous agent, `agent:refactor-bot`.
    Agent,
    /// A trusted automated service such as an execution plane, `service:ci`.
    Service,
    /// The system itself (migrations, imports), `system:import`.
    System,
}

impl IdentityKind {
    /// The textual prefix used in identities.
    pub fn prefix(self) -> &'static str {
        match self {
            IdentityKind::User => "user",
            IdentityKind::Agent => "agent",
            IdentityKind::Service => "service",
            IdentityKind::System => "system",
        }
    }
}

/// Reasons a string is not a valid [`Identity`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    /// Missing or unknown `kind:` prefix.
    #[error("identity must look like 'user:name', 'agent:name', 'service:name' or 'system:name'")]
    BadPrefix,
    /// The name part is empty or too long.
    #[error("identity name must be 1..=128 characters")]
    BadLength,
    /// The name contains characters outside `[A-Za-z0-9._@#+-]`.
    #[error("identity name may only contain letters, digits and '._@#+-'")]
    BadChar,
}

/// Who performed an action: `kind:name`.
///
/// The name is restricted to `[A-Za-z0-9._@#+-]`, 1 to 128 characters, so an
/// identity can always be embedded in logs, paths and signed payloads
/// without escaping. Agent runs use the form `agent:name#run`.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Identity(String);

impl Identity {
    /// Validates and wraps an identity string.
    pub fn new(s: impl Into<String>) -> Result<Self, IdentityError> {
        let s: String = s.into();
        let (prefix, name) = s.split_once(':').ok_or(IdentityError::BadPrefix)?;
        if !["user", "agent", "service", "system"].contains(&prefix) {
            return Err(IdentityError::BadPrefix);
        }
        if name.is_empty() || name.len() > 128 {
            return Err(IdentityError::BadLength);
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._@#+-".contains(c))
        {
            return Err(IdentityError::BadChar);
        }
        Ok(Self(s))
    }

    /// Convenience constructor: `Identity::of(IdentityKind::Agent, "bot")`.
    pub fn of(kind: IdentityKind, name: &str) -> Result<Self, IdentityError> {
        Self::new(format!("{}:{}", kind.prefix(), name))
    }

    /// The identity as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The category of this identity.
    pub fn kind(&self) -> IdentityKind {
        match self.0.split_once(':').map(|(p, _)| p) {
            Some("user") => IdentityKind::User,
            Some("agent") => IdentityKind::Agent,
            Some("service") => IdentityKind::Service,
            _ => IdentityKind::System,
        }
    }

    /// The part after `kind:`.
    pub fn name(&self) -> &str {
        self.0.split_once(':').map(|(_, n)| n).unwrap_or("")
    }

    /// The identity with any `#run` suffix removed. Approval and ownership
    /// rules compare *principals* so that one agent cannot approve its own
    /// work by starting a new run.
    pub fn principal(&self) -> Identity {
        match self.0.split_once('#') {
            Some((head, _)) => Identity(head.to_string()),
            None => self.clone(),
        }
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Identity({})", self.0)
    }
}

impl std::str::FromStr for Identity {
    type Err = IdentityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Serialize for Identity {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Identity {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Identity::new(s).map_err(serde::de::Error::custom)
    }
}

/// Reasons a string is not a valid [`Namespace`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("namespace must be 1..=128 chars of lowercase letters, digits, '.', '_', '-' with optional '/' separators")]
pub struct NamespaceError;

/// The repository namespace (`org/name`), part of every changeset hash so a
/// changeset cannot be replayed into a different repository.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Namespace(String);

impl Namespace {
    /// Validates and wraps a namespace.
    pub fn new(s: impl Into<String>) -> Result<Self, NamespaceError> {
        let s: String = s.into();
        if s.is_empty() || s.len() > 128 {
            return Err(NamespaceError);
        }
        for seg in s.split('/') {
            let mut chars = seg.chars();
            match chars.next() {
                Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
                _ => return Err(NamespaceError),
            }
            if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c)) {
                return Err(NamespaceError);
            }
        }
        Ok(Self(s))
    }

    /// The namespace as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Namespace({})", self.0)
    }
}

impl std::str::FromStr for Namespace {
    type Err = NamespaceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Serialize for Namespace {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Namespace {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Namespace::new(s).map_err(serde::de::Error::custom)
    }
}

/// Reasons a string is not a valid [`StreamName`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("stream name must be 1..=64 chars of lowercase letters, digits, '.', '_', '-' with optional '/' separators")]
pub struct StreamNameError;

/// The name of a stream (AgentGit's branch-like line of integration).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StreamName(String);

impl StreamName {
    /// Validates and wraps a stream name.
    pub fn new(s: impl Into<String>) -> Result<Self, StreamNameError> {
        let s: String = s.into();
        if s.is_empty() || s.len() > 64 {
            return Err(StreamNameError);
        }
        for seg in s.split('/') {
            let mut chars = seg.chars();
            match chars.next() {
                Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
                _ => return Err(StreamNameError),
            }
            if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c)) {
                return Err(StreamNameError);
            }
        }
        Ok(Self(s))
    }

    /// The name as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StreamName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for StreamName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StreamName({})", self.0)
    }
}

impl std::str::FromStr for StreamName {
    type Err = StreamNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Serialize for StreamName {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for StreamName {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        StreamName::new(s).map_err(serde::de::Error::custom)
    }
}

/// Reasons a string is not a valid [`OperationId`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("operation id must be 8..=64 characters of [A-Za-z0-9_-]")]
pub struct OperationIdError;

/// Identifier of a mutating operation. Clients use it to ask for the outcome
/// of an operation whose response they lost.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OperationId(String);

impl OperationId {
    /// Validates and wraps an operation id.
    pub fn new(s: impl Into<String>) -> Result<Self, OperationIdError> {
        let s: String = s.into();
        if !(8..=64).contains(&s.len())
            || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(OperationIdError);
        }
        Ok(Self(s))
    }

    /// The id as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OperationId({})", self.0)
    }
}

impl std::str::FromStr for OperationId {
    type Err = OperationIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Serialize for OperationId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for OperationId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        OperationId::new(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_validation() {
        assert!(Identity::new("user:alice").is_ok());
        assert!(Identity::new("agent:codegen#run-7f3a").is_ok());
        assert_eq!(Identity::new("alice"), Err(IdentityError::BadPrefix));
        assert_eq!(Identity::new("root:alice"), Err(IdentityError::BadPrefix));
        assert_eq!(Identity::new("user:"), Err(IdentityError::BadLength));
        assert_eq!(Identity::new("user:a b"), Err(IdentityError::BadChar));
        assert_eq!(Identity::new("user:a\nb"), Err(IdentityError::BadChar));
    }

    #[test]
    fn principal_strips_run() {
        let id = Identity::new("agent:bot#run-1").unwrap();
        assert_eq!(id.principal().as_str(), "agent:bot");
        assert_eq!(id.kind(), IdentityKind::Agent);
        assert_eq!(id.name(), "bot#run-1");
    }

    #[test]
    fn namespace_validation() {
        assert!(Namespace::new("acme/webapp").is_ok());
        assert!(Namespace::new("a").is_ok());
        for bad in ["", "Acme", "/a", "a/", "a//b", "-a", "a b"] {
            assert!(Namespace::new(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn stream_name_validation() {
        assert!(StreamName::new("main").is_ok());
        assert!(StreamName::new("feature/login-v2").is_ok());
        for bad in ["", "Main", "/x", "x/", "a//b", "a b", "-x", &"a".repeat(65)] {
            assert!(StreamName::new(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn operation_id_validation() {
        assert!(OperationId::new("op_0123456789").is_ok());
        assert!(OperationId::new("short").is_err());
        assert!(OperationId::new("has space 123").is_err());
    }
}
