//! Authorization: roles, capabilities and credential scopes.
//!
//! A request is allowed when **both** hold:
//!
//! 1. one of the caller's *roles* on the repository grants the capability
//!    (RBAC; roles are bound to identities, groups or identity patterns, per
//!    repository pattern), and
//! 2. the capability, repository and stream fall inside the *scope* of the
//!    credential that was presented (ABAC-style attenuation: a credential can
//!    only ever be narrower than its owner).
//!
//! Authorization answers "may this identity call this API on this resource".
//! Whether a *change* may land is a separate question answered by the stream
//! policy inside the engine; neither replaces the other.

use std::collections::BTreeSet;

use globset::{Glob, GlobMatcher};

use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};

macro_rules! caps {
    ($( $variant:ident => $name:literal ),+ $(,)?) => {
        /// A fine-grained permission (named `area.action`; see `Cap::as_str`).
        #[allow(missing_docs)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Cap { $( $variant ),+ }

        impl Cap {
            /// Every capability.
            pub const ALL: &'static [Cap] = &[ $( Cap::$variant ),+ ];

            /// Wire name, e.g. `proposal.create`.
            pub fn as_str(self) -> &'static str {
                match self { $( Cap::$variant => $name ),+ }
            }

            /// Parses a wire name.
            pub fn parse(s: &str) -> Option<Cap> {
                match s { $( $name => Some(Cap::$variant), )+ _ => None }
            }
        }
    };
}

caps! {
    RepoRead => "repo.read",
    RepoCreate => "repo.create",
    RepoConfigure => "repo.configure",
    ObjectsRead => "objects.read",
    ObjectsWrite => "objects.write",
    Publish => "publish",
    OperationRead => "operation.read",
    HistoryRead => "history.read",
    TreeRead => "tree.read",
    StreamRead => "stream.read",
    StreamCreate => "stream.create",
    StreamCas => "stream.cas",
    StreamForce => "stream.force",
    WorkspaceRead => "workspace.read",
    WorkspaceWrite => "workspace.write",
    ProposalRead => "proposal.read",
    ProposalCreate => "proposal.create",
    ProposalUpdate => "proposal.update",
    ProposalReview => "proposal.review",
    ProposalClose => "proposal.close",
    IntegratePrepare => "integrate.prepare",
    IntegrateValidate => "integrate.validate",
    IntegrateRun => "integrate.run",
    QueueRead => "queue.read",
    QueueWrite => "queue.write",
    QueueRun => "queue.run",
    AttestationRead => "attestation.read",
    AttestationSubmit => "attestation.submit",
    AgentRead => "agent.read",
    AgentRegister => "agent.register",
    AgentRun => "agent.run",
    TaskRead => "task.read",
    TaskWrite => "task.write",
    LeaseRead => "lease.read",
    LeaseWrite => "lease.write",
    ProvenanceRead => "provenance.read",
    ProvenanceWrite => "provenance.write",
    Predict => "predict",
    EventsRead => "events.read",
    PolicyRead => "policy.read",
    PolicyWrite => "policy.write",
    PinsRead => "pins.read",
    PinsWrite => "pins.write",
    AuditRead => "audit.read",
    GcRun => "gc.run",
    FsckRun => "fsck.run",
    CredentialsMint => "credentials.mint",
    CredentialsList => "credentials.list",
    CredentialsRevoke => "credentials.revoke",
    RolesManage => "roles.manage",
    JobsRead => "jobs.read",
    JobsClaim => "jobs.claim",
}

/// A named bundle of capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    /// Read everything about a repository.
    Reader,
    /// Reader + publish changes, open proposals, run as an agent.
    Writer,
    /// Reader + review proposals.
    Reviewer,
    /// Reader + land approved proposals (the stream policy still decides).
    Integrator,
    /// Execution-plane worker: claim jobs and submit attestations.
    Runner,
    /// Everything, including policy, credentials, roles, GC.
    Admin,
}

impl Role {
    /// Every role.
    pub const ALL: &'static [Role] =
        &[Role::Reader, Role::Writer, Role::Reviewer, Role::Integrator, Role::Runner, Role::Admin];

    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Reader => "reader",
            Role::Writer => "writer",
            Role::Reviewer => "reviewer",
            Role::Integrator => "integrator",
            Role::Runner => "runner",
            Role::Admin => "admin",
        }
    }

    /// Parses a wire name.
    pub fn parse(s: &str) -> Option<Role> {
        Role::ALL.iter().copied().find(|r| r.as_str() == s)
    }

    /// Capabilities granted by this role.
    pub fn caps(self) -> BTreeSet<Cap> {
        use Cap::*;
        let reader: &[Cap] = &[
            RepoRead,
            ObjectsRead,
            OperationRead,
            HistoryRead,
            TreeRead,
            StreamRead,
            WorkspaceRead,
            ProposalRead,
            IntegratePrepare,
            QueueRead,
            AttestationRead,
            AgentRead,
            TaskRead,
            LeaseRead,
            ProvenanceRead,
            Predict,
            EventsRead,
            PolicyRead,
            PinsRead,
            JobsRead,
        ];
        let mut set: BTreeSet<Cap> = BTreeSet::new();
        match self {
            Role::Reader => set.extend(reader),
            Role::Writer => {
                set.extend(reader);
                set.extend([
                    ObjectsWrite,
                    Publish,
                    StreamCreate,
                    WorkspaceWrite,
                    ProposalCreate,
                    ProposalUpdate,
                    ProposalClose,
                    IntegrateValidate,
                    QueueWrite,
                    AgentRegister,
                    AgentRun,
                    TaskWrite,
                    LeaseWrite,
                    ProvenanceWrite,
                ]);
            }
            Role::Reviewer => {
                set.extend(reader);
                set.insert(ProposalReview);
            }
            Role::Integrator => {
                set.extend(reader);
                set.extend([IntegrateRun, IntegrateValidate, QueueWrite, QueueRun, ProposalClose]);
            }
            Role::Runner => {
                set.extend(reader);
                set.extend([JobsClaim, AttestationSubmit]);
            }
            Role::Admin => set.extend(Cap::ALL.iter().copied()),
        }
        set
    }
}

/// A `*`-glob pattern list.
#[derive(Clone, Debug, Default)]
pub struct Patterns {
    raw: Vec<String>,
    compiled: Vec<GlobMatcher>,
}

impl Patterns {
    /// Compiles patterns; an invalid pattern is an error.
    pub fn new(raw: &[String]) -> Result<Self, String> {
        let mut compiled = Vec::with_capacity(raw.len());
        for p in raw {
            compiled.push(Glob::new(p).map_err(|e| format!("bad pattern '{p}': {e}"))?.compile_matcher());
        }
        Ok(Self { raw: raw.to_vec(), compiled })
    }

    /// True when there are no patterns (meaning "unrestricted" in scopes).
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    /// True if any pattern matches.
    pub fn matches(&self, s: &str) -> bool {
        self.compiled.iter().any(|m| m.is_match(s))
    }

    /// The source patterns.
    pub fn raw(&self) -> &[String] {
        &self.raw
    }
}

/// Matches a single pattern against a string (`*` wildcards).
pub fn glob_match(pattern: &str, value: &str) -> bool {
    Glob::new(pattern).map(|g| g.compile_matcher().is_match(value)).unwrap_or(false)
}

/// The narrowing restrictions attached to a credential.
#[derive(Clone, Debug, Default)]
pub struct Scope {
    /// Repository patterns (empty = any).
    pub repos: Patterns,
    /// Stream patterns (empty = any).
    pub streams: Patterns,
    /// Allowed capabilities (empty = any the roles grant).
    pub operations: BTreeSet<Cap>,
}

impl Scope {
    /// Parses and validates the wire form.
    pub fn from_wire(s: &ag_protocol::v1::CredentialScope) -> Result<Self, ApiError> {
        let mut operations = BTreeSet::new();
        for op in &s.operations {
            operations.insert(Cap::parse(op).ok_or_else(|| ApiError::invalid(format!("unknown operation '{op}'")))?);
        }
        Ok(Self {
            repos: Patterns::new(&s.repos).map_err(ApiError::invalid)?,
            streams: Patterns::new(&s.streams).map_err(ApiError::invalid)?,
            operations,
        })
    }

    /// The wire form.
    pub fn to_wire(&self) -> ag_protocol::v1::CredentialScope {
        ag_protocol::v1::CredentialScope {
            repos: self.repos.raw().to_vec(),
            streams: self.streams.raw().to_vec(),
            operations: self.operations.iter().map(|c| c.as_str().to_string()).collect(),
        }
    }

    /// True if this scope puts no restriction at all.
    pub fn is_unrestricted(&self) -> bool {
        self.repos.is_empty() && self.streams.is_empty() && self.operations.is_empty()
    }

    /// Whether the scope permits `cap` on `repo` / `stream`.
    pub fn permits(&self, repo: Option<&str>, cap: Cap, stream: Option<&str>) -> bool {
        if !self.operations.is_empty() && !self.operations.contains(&cap) {
            return false;
        }
        if let Some(r) = repo
            && !self.repos.is_empty()
            && !self.repos.matches(r)
        {
            return false;
        }
        if let Some(s) = stream
            && !self.streams.is_empty()
            && !self.streams.matches(s)
        {
            return false;
        }
        true
    }

    /// True if `narrower` is contained in `self` (a delegated credential must
    /// never exceed the scope of the credential that minted it).
    pub fn contains(&self, narrower: &Scope) -> bool {
        let list_ok = |outer: &Patterns, inner: &Patterns| {
            outer.is_empty()
                || (!inner.is_empty() && inner.raw().iter().all(|p| outer.matches(p) || outer.raw().contains(p)))
        };
        let ops_ok = self.operations.is_empty()
            || (!narrower.operations.is_empty() && narrower.operations.iter().all(|o| self.operations.contains(o)));
        list_ok(&self.repos, &narrower.repos) && list_ok(&self.streams, &narrower.streams) && ops_ok
    }
}

/// A role granted on repositories matching `repo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// Subject pattern the binding applies to.
    pub subject: String,
    /// Repository pattern.
    pub repo: String,
    /// The role.
    pub role: Role,
}

/// Decides whether a principal may do something.
pub struct Authorizer;

impl Authorizer {
    /// The roles `principal` holds on `repo` given the stored `bindings`
    /// (and its OIDC group mappings).
    pub fn roles(principal: &Principal, bindings: &[Binding], repo: &str) -> BTreeSet<Role> {
        let mut out = BTreeSet::new();
        if principal.bootstrap {
            out.insert(Role::Admin);
            return out;
        }
        let ident = principal.identity.as_str();
        let base = principal.identity.principal();
        for b in bindings {
            let subject_ok = glob_match(&b.subject, ident)
                || glob_match(&b.subject, base.as_str())
                || principal.groups.iter().any(|g| b.subject == format!("group:{g}"));
            if subject_ok && glob_match(&b.repo, repo) {
                out.insert(b.role);
            }
        }
        for (role, pattern) in &principal.group_roles {
            if glob_match(pattern, repo) {
                out.insert(*role);
            }
        }
        out
    }

    /// Checks `cap` for `principal` on `repo` (and optionally a stream).
    ///
    /// A caller with no role at all on the repository gets `not_found`, so
    /// the existence of private repositories is not revealed.
    pub fn check(
        principal: &Principal,
        bindings: &[Binding],
        repo: &str,
        cap: Cap,
        stream: Option<&str>,
    ) -> ApiResult<()> {
        let roles = Self::roles(principal, bindings, repo);
        if roles.is_empty() {
            return Err(ApiError::not_found(format!("repository '{repo}' not found")));
        }
        if !roles.iter().any(|r| r.caps().contains(&cap)) {
            return Err(ApiError::denied(format!(
                "this identity ({}) may not perform '{}' on '{repo}' (roles: {})",
                principal.identity,
                cap.as_str(),
                roles.iter().map(|r| r.as_str()).collect::<Vec<_>>().join(", ")
            )));
        }
        if !principal.scope.permits(Some(repo), cap, stream) {
            return Err(ApiError::denied(format!(
                "the credential presented is not scoped for '{}' on '{repo}'{}",
                cap.as_str(),
                stream.map(|s| format!(" (stream '{s}')")).unwrap_or_default()
            )));
        }
        Ok(())
    }

    /// Checks a capability that is not tied to a repository (listing
    /// credentials, granting global roles): requires the role on pattern `*`.
    pub fn check_global(principal: &Principal, bindings: &[Binding], cap: Cap) -> ApiResult<()> {
        Self::check(principal, bindings, "*", cap, None).map_err(|e| {
            if e.code() == ag_protocol::ErrorCode::NotFound {
                ApiError::denied(format!("'{}' requires a global role", cap.as_str()))
            } else {
                e
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Principal;

    fn binding(subject: &str, repo: &str, role: Role) -> Binding {
        Binding { subject: subject.into(), repo: repo.into(), role }
    }

    #[test]
    fn capability_names_round_trip_and_are_unique() {
        let mut seen = BTreeSet::new();
        for c in Cap::ALL {
            assert_eq!(Cap::parse(c.as_str()), Some(*c));
            assert!(seen.insert(c.as_str()));
        }
    }

    #[test]
    fn roles_are_strictly_ordered_by_power() {
        let reader = Role::Reader.caps();
        assert!(Role::Writer.caps().is_superset(&reader));
        assert!(Role::Reviewer.caps().is_superset(&reader));
        assert!(Role::Integrator.caps().is_superset(&reader));
        assert!(Role::Admin.caps().len() == Cap::ALL.len());
        // Integrating and reviewing are separate powers from authoring.
        assert!(!Role::Writer.caps().contains(&Cap::IntegrateRun));
        assert!(!Role::Writer.caps().contains(&Cap::ProposalReview));
        assert!(!Role::Reviewer.caps().contains(&Cap::ProposalCreate));
        assert!(!Role::Runner.caps().contains(&Cap::ObjectsWrite));
        assert!(Role::Runner.caps().contains(&Cap::AttestationSubmit));
        assert!(!Role::Admin.caps().is_empty());
    }

    #[test]
    fn bindings_match_identities_patterns_and_runs() {
        let p = Principal::for_tests("agent:bot#run_1");
        let b = vec![binding("agent:*", "acme/*", Role::Writer), binding("user:alice", "*", Role::Admin)];
        let roles = Authorizer::roles(&p, &b, "acme/web");
        assert_eq!(roles.into_iter().collect::<Vec<_>>(), vec![Role::Writer]);
        assert!(Authorizer::roles(&p, &b, "other/web").is_empty());
        assert!(Authorizer::check(&p, &b, "acme/web", Cap::ObjectsWrite, None).is_ok());
        let e = Authorizer::check(&p, &b, "acme/web", Cap::IntegrateRun, None).unwrap_err();
        assert_eq!(e.code(), ag_protocol::ErrorCode::PermissionDenied);
        let e = Authorizer::check(&p, &b, "other/web", Cap::RepoRead, None).unwrap_err();
        assert_eq!(e.code(), ag_protocol::ErrorCode::NotFound, "no role at all must not reveal existence");
    }

    #[test]
    fn scope_can_only_narrow() {
        let mut p = Principal::for_tests("user:alice");
        let b = vec![binding("user:alice", "*", Role::Admin)];
        p.scope = Scope {
            repos: Patterns::new(&["acme/web".into()]).unwrap(),
            streams: Patterns::new(&["agents/*".into()]).unwrap(),
            operations: [Cap::ProposalCreate, Cap::ObjectsWrite].into_iter().collect(),
        };
        assert!(Authorizer::check(&p, &b, "acme/web", Cap::ProposalCreate, Some("agents/a")).is_ok());
        assert!(Authorizer::check(&p, &b, "acme/web", Cap::ProposalCreate, Some("main")).is_err());
        assert!(Authorizer::check(&p, &b, "acme/web", Cap::IntegrateRun, Some("agents/a")).is_err());
        assert!(Authorizer::check(&p, &b, "acme/other", Cap::ProposalCreate, None).is_err());
    }

    #[test]
    fn delegation_cannot_widen() {
        let wide = Scope::default();
        let narrow = Scope {
            repos: Patterns::new(&["acme/web".into()]).unwrap(),
            streams: Patterns::default(),
            operations: [Cap::RepoRead].into_iter().collect(),
        };
        assert!(wide.contains(&narrow));
        assert!(!narrow.contains(&wide), "an unrestricted credential cannot be minted from a scoped one");
        let other = Scope { repos: Patterns::new(&["acme/api".into()]).unwrap(), ..Scope::default() };
        assert!(!narrow.contains(&other));
        let sub = Scope {
            repos: Patterns::new(&["acme/web".into()]).unwrap(),
            streams: Patterns::default(),
            operations: [Cap::RepoRead].into_iter().collect(),
        };
        assert!(narrow.contains(&sub));
    }

    #[test]
    fn group_bindings_apply() {
        let mut p = Principal::for_tests("user:carol");
        p.groups = vec!["platform".into()];
        let b = vec![binding("group:platform", "acme/*", Role::Integrator)];
        assert!(Authorizer::check(&p, &b, "acme/x", Cap::IntegrateRun, None).is_ok());
    }
}
