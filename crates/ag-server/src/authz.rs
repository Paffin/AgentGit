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
    CredentialsDelegate => "credentials.delegate",
    CredentialsList => "credentials.list",
    CredentialsRevoke => "credentials.revoke",
    RolesManage => "roles.manage",
    JobsRead => "jobs.read",
    JobsClaim => "jobs.claim",
    AgentOperate => "agent.operate",
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
    /// Runs agents for others (an orchestrator): everything a writer may do, plus minting credentials for
    /// `agent:*` subjects - never for people, services or administrators, and never with a capability the
    /// delegator does not hold itself - and operating other agents' tasks and runs (`agent.operate`).
    Delegator,
    /// Everything, including policy, credentials, roles, GC.
    Admin,
}

impl Role {
    /// Every role.
    pub const ALL: &'static [Role] =
        &[Role::Reader, Role::Writer, Role::Reviewer, Role::Integrator, Role::Runner, Role::Delegator, Role::Admin];

    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Reader => "reader",
            Role::Writer => "writer",
            Role::Reviewer => "reviewer",
            Role::Integrator => "integrator",
            Role::Runner => "runner",
            Role::Delegator => "delegator",
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
        let writer: &[Cap] = &[
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
        ];
        let mut set: BTreeSet<Cap> = BTreeSet::new();
        match self {
            Role::Reader => set.extend(reader),
            Role::Writer => {
                set.extend(reader);
                set.extend(writer);
            }
            Role::Delegator => {
                set.extend(reader);
                set.extend(writer);
                set.extend([CredentialsDelegate, AgentOperate]);
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

/// Matches a single pattern against a string (`*` wildcards). An invalid pattern matches nothing.
pub fn glob_match(pattern: &str, value: &str) -> bool {
    Glob::new(pattern).map(|g| g.compile_matcher().is_match(value)).unwrap_or(false)
}

/// Checks that `pattern` is a glob this server can compile. Role bindings are validated with it when they are
/// created, so a typo is an error at once and not a permanently inert binding.
pub fn validate_pattern(pattern: &str) -> Result<(), String> {
    if pattern.is_empty() {
        return Err("a pattern must not be empty".into());
    }
    Glob::new(pattern).map(|_| ()).map_err(|e| format!("bad pattern '{pattern}': {e}"))
}

/// A pattern whose set of matches is easy to compare with another's: letters, digits and `._-/`, optionally
/// ending in one `*`. Containment ([`pattern_within`]) is only decided for these.
fn is_simple_pattern(p: &str) -> bool {
    !p.is_empty()
        && p.bytes()
            .enumerate()
            .all(|(i, b)| b.is_ascii_alphanumeric() || b"._-/".contains(&b) || (b == b'*' && i == p.len() - 1))
}

/// Whether every name `inner` matches is also matched by `outer`.
///
/// Deliberately conservative: `true` only when it can be proved — the patterns are equal, `outer` is `*`, or
/// both are "simple" (see `is_simple_pattern`) and `outer` is a prefix pattern covering `inner`'s prefix.
/// Anything else (`?`, character classes, alternation, a `*` in the middle) is `false`, so a delegated
/// credential can never be wider than the one that minted it. (Matching `inner`'s *source text* against
/// `outer`, which this replaces, is unsound: `acme/*b*` "contains" `acme/[ab]*`, yet the latter also matches
/// `acme/alpha`.)
pub fn pattern_within(outer: &str, inner: &str) -> bool {
    if outer == "*" || outer == inner {
        return true;
    }
    if !is_simple_pattern(outer) || !is_simple_pattern(inner) {
        return false;
    }
    match outer.strip_suffix('*') {
        // `outer` covers every name starting with its prefix; `inner` (its own `*` dropped) must start with it too.
        Some(prefix) => inner.trim_end_matches('*').starts_with(prefix),
        // A literal outer contains only itself, and equality was handled above.
        None => false,
    }
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
    /// The run the credential is bound to (ADR-0010), if any. `repos` then holds exactly one literal repository
    /// name: the one that holds the run.
    pub run_id: Option<String>,
}

/// Whether `id` can be a run id (what `ag_crypto::random_id("run")` makes, and what the protocol accepts in a path).
pub fn is_run_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl Scope {
    /// Parses and validates the wire form.
    pub fn from_wire(s: &ag_protocol::v1::CredentialScope) -> Result<Self, ApiError> {
        let mut operations = BTreeSet::new();
        for op in &s.operations {
            operations.insert(Cap::parse(op).ok_or_else(|| ApiError::invalid(format!("unknown operation '{op}'")))?);
        }
        let run_id = (!s.run_id.is_empty()).then(|| s.run_id.clone());
        if let Some(run) = &run_id {
            if !is_run_id(run) {
                return Err(ApiError::invalid(format!("'{run}' is not a run id")));
            }
            // A run lives in one repository: the credential is pinned to it, by its literal name.
            match s.repos.as_slice() {
                [repo] if crate::repos::parse_repo_name(repo).is_ok() => {}
                _ => {
                    return Err(ApiError::invalid(
                        "a credential bound to a run must name exactly one repository (`owner/name`, no wildcards) in `repos`",
                    ));
                }
            }
        }
        Ok(Self {
            repos: Patterns::new(&s.repos).map_err(ApiError::invalid)?,
            streams: Patterns::new(&s.streams).map_err(ApiError::invalid)?,
            operations,
            run_id,
        })
    }

    /// The wire form.
    pub fn to_wire(&self) -> ag_protocol::v1::CredentialScope {
        ag_protocol::v1::CredentialScope {
            repos: self.repos.raw().to_vec(),
            streams: self.streams.raw().to_vec(),
            operations: self.operations.iter().map(|c| c.as_str().to_string()).collect(),
            run_id: self.run_id.clone().unwrap_or_default(),
        }
    }

    /// The `(repository, run id)` the credential is bound to, if it is bound to a run.
    pub fn bound_run(&self) -> Option<(&str, &str)> {
        let run = self.run_id.as_deref()?;
        // `from_wire` guarantees exactly one literal repository whenever a run is set.
        Some((self.repos.raw().first()?.as_str(), run))
    }

    /// True if this scope puts no restriction at all.
    pub fn is_unrestricted(&self) -> bool {
        self.repos.is_empty() && self.streams.is_empty() && self.operations.is_empty() && self.run_id.is_none()
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

    /// Like [`Scope::permits`] for a whole *pattern* of repositories (the repositories a credential being
    /// minted or revoked is scoped to): every repository the pattern could match must be inside the scope.
    pub fn permits_pattern(&self, repo_pattern: &str, cap: Cap) -> bool {
        (self.operations.is_empty() || self.operations.contains(&cap))
            && (self.repos.is_empty() || self.repos.raw().iter().any(|o| pattern_within(o, repo_pattern)))
    }

    /// True if `narrower` is contained in `self` (a delegated credential must
    /// never exceed the scope of the credential that minted it).
    pub fn contains(&self, narrower: &Scope) -> bool {
        let list_ok = |outer: &Patterns, inner: &Patterns| {
            outer.is_empty()
                || (!inner.is_empty() && inner.raw().iter().all(|p| outer.raw().iter().any(|o| pattern_within(o, p))))
        };
        let ops_ok = self.operations.is_empty()
            || (!narrower.operations.is_empty() && narrower.operations.iter().all(|o| self.operations.contains(o)));
        // A credential bound to a run can only hand out credentials bound to that same run.
        let run_ok = self.run_id.is_none() || self.run_id == narrower.run_id;
        list_ok(&self.repos, &narrower.repos) && list_ok(&self.streams, &narrower.streams) && ops_ok && run_ok
    }
}

/// A role granted on repositories matching `repo`.
///
/// The patterns are compiled once, when the binding is loaded, not for every request. A pattern that does not
/// compile makes the binding inert (it matches nothing, so never more than was granted); bindings are
/// validated when they are created, so that only happens with a row written by something other than this server.
#[derive(Clone, Debug)]
pub struct Binding {
    /// Subject pattern the binding applies to.
    pub subject: String,
    /// Repository pattern.
    pub repo: String,
    /// The role.
    pub role: Role,
    subject_glob: Option<GlobMatcher>,
    repo_glob: Option<GlobMatcher>,
}

impl PartialEq for Binding {
    fn eq(&self, other: &Self) -> bool {
        (&self.subject, &self.repo, self.role) == (&other.subject, &other.repo, other.role)
    }
}

impl Eq for Binding {}

impl Binding {
    /// A binding with its patterns compiled.
    pub fn new(subject: impl Into<String>, repo: impl Into<String>, role: Role) -> Self {
        let (subject, repo) = (subject.into(), repo.into());
        let compile = |p: &str| Glob::new(p).ok().map(|g| g.compile_matcher());
        Self { subject_glob: compile(&subject), repo_glob: compile(&repo), subject, repo, role }
    }

    /// False if one of the patterns does not compile (the binding can never apply).
    pub fn is_usable(&self) -> bool {
        self.subject_glob.is_some() && self.repo_glob.is_some()
    }

    fn subject_matches_text(&self, text: &str) -> bool {
        self.subject_glob.as_ref().is_some_and(|g| g.is_match(text))
    }

    fn repo_matches(&self, repo: &str) -> bool {
        self.repo_glob.as_ref().is_some_and(|g| g.is_match(repo))
    }

    /// Whether this binding's subject covers `principal`: by identity, by its base principal (a run of an
    /// agent is covered by the agent's binding), or by a group the identity provider reported.
    pub fn applies_to(&self, principal: &Principal) -> bool {
        self.subject_matches_text(principal.identity.as_str())
            || self.subject_matches_text(principal.identity.principal().as_str())
            || principal.groups.iter().any(|g| self.subject.strip_prefix("group:") == Some(g.as_str()))
    }
}

/// Capabilities a delegator never passes on to the credentials it mints, whatever roles it holds: the ones that
/// administer the installation (credentials, roles, policy, repositories, maintenance), and the operator powers
/// over other agents. A test pins that every capability only administrators have is listed here.
pub const NEVER_DELEGATED: &[Cap] = &[
    Cap::RepoCreate,
    Cap::RepoConfigure,
    Cap::StreamCas,
    Cap::StreamForce,
    Cap::PolicyWrite,
    Cap::PinsWrite,
    Cap::AuditRead,
    Cap::GcRun,
    Cap::FsckRun,
    Cap::CredentialsMint,
    Cap::CredentialsDelegate,
    Cap::CredentialsList,
    Cap::CredentialsRevoke,
    Cap::RolesManage,
    Cap::AgentOperate,
];

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
        for b in bindings {
            if b.applies_to(principal) && b.repo_matches(repo) {
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

    /// The roles `principal` holds on *every* repository `repo_pattern` could match: the bindings whose
    /// repository pattern provably covers it (see [`pattern_within`]).
    pub fn roles_covering(principal: &Principal, bindings: &[Binding], repo_pattern: &str) -> BTreeSet<Role> {
        let mut roles = BTreeSet::new();
        if principal.bootstrap {
            roles.insert(Role::Admin);
            return roles;
        }
        for b in bindings {
            if b.applies_to(principal) && pattern_within(&b.repo, repo_pattern) {
                roles.insert(b.role);
            }
        }
        for (role, pattern) in &principal.group_roles {
            if pattern_within(pattern, repo_pattern) {
                roles.insert(*role);
            }
        }
        roles
    }

    /// The capabilities `principal` may pass on to an agent credential for `repo_pattern` (ADR-0010): those its
    /// own roles grant on every repository the pattern covers, and its own credential's scope permits, less
    /// [`NEVER_DELEGATED`]. A delegator can hand down what it holds, never more.
    pub fn delegable(principal: &Principal, bindings: &[Binding], repo_pattern: &str) -> BTreeSet<Cap> {
        let mut caps: BTreeSet<Cap> =
            Self::roles_covering(principal, bindings, repo_pattern).iter().flat_map(|r| r.caps()).collect();
        if !principal.scope.operations.is_empty() {
            caps.retain(|c| principal.scope.operations.contains(c));
        }
        caps.retain(|c| !NEVER_DELEGATED.contains(c));
        caps
    }

    /// Checks `cap` for `principal` on every repository a *pattern* could match: some role the caller holds
    /// must cover the whole pattern (see [`pattern_within`]) and the credential presented must be scoped for it.
    /// Used where a credential is minted or revoked for a set of repositories, where the pattern is not itself
    /// a repository name.
    pub fn check_pattern(principal: &Principal, bindings: &[Binding], repo_pattern: &str, cap: Cap) -> ApiResult<()> {
        let roles = Self::roles_covering(principal, bindings, repo_pattern);
        if roles.is_empty() {
            return Err(ApiError::not_found(format!("repository '{repo_pattern}' not found")));
        }
        if !roles.iter().any(|r| r.caps().contains(&cap)) {
            return Err(ApiError::denied(format!(
                "this identity ({}) may not perform '{}' on '{repo_pattern}' (roles: {})",
                principal.identity,
                cap.as_str(),
                roles.iter().map(|r| r.as_str()).collect::<Vec<_>>().join(", ")
            )));
        }
        if !principal.scope.permits_pattern(repo_pattern, cap) {
            return Err(ApiError::denied(format!(
                "the credential presented is not scoped for '{}' on '{repo_pattern}'",
                cap.as_str()
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
        Binding::new(subject, repo, role)
    }

    /// A small universe of repository names, enough to tell prefix patterns apart.
    fn universe() -> Vec<String> {
        let mut names = Vec::new();
        for owner in ["acme", "acme2", "ac", "other", "a"] {
            names.push(owner.to_string());
            for name in ["web", "w", "wb", "api", "alpha", "beta", "b", "ab", "web/x", "web.y", ""] {
                names.push(format!("{owner}/{name}"));
            }
        }
        names
    }

    #[test]
    fn containment_never_claims_more_than_is_true() {
        // Every pair of patterns from a pool: whenever `pattern_within` says yes, no name of the universe may
        // match the inner pattern without matching the outer one. (The reverse is allowed to be unknown.)
        let pool = [
            "*",
            "acme/*",
            "acme/web",
            "acme/web*",
            "acme/w*",
            "acme/*b*",
            "acme/[ab]*",
            "acme/{web,api}",
            "acme/?eb",
            "acme*",
            "ac*",
            "acme/",
            "acme/alpha",
            "a*",
            "acme/web/*",
            "other/*",
            "acme/web**",
            "acme/*-x",
            "ACME/*",
        ];
        let names = universe();
        let mut proved = 0;
        for outer in pool {
            for inner in pool {
                if pattern_within(outer, inner) {
                    proved += 1;
                    for n in &names {
                        assert!(
                            !glob_match(inner, n) || glob_match(outer, n),
                            "pattern_within({outer:?}, {inner:?}) is true, but {n:?} matches only the inner pattern"
                        );
                    }
                }
            }
        }
        assert!(proved > pool.len(), "the check must prove something besides reflexivity ({proved})");
        // The case the old source-text comparison got wrong.
        assert!(!pattern_within("acme/*b*", "acme/[ab]*"));
        assert!(pattern_within("acme/*", "acme/web"));
        assert!(pattern_within("acme/*", "acme/web*"));
        assert!(!pattern_within("acme/web*", "acme/w*"));
        assert!(!pattern_within("acme/web", "acme/web*"));
        assert!(pattern_within("*", "acme/[ab]*"), "everything is within *");
    }

    #[test]
    fn a_scoped_credential_cannot_mint_a_wider_one_through_glob_tricks() {
        let outer = Scope { repos: Patterns::new(&["acme/*b*".into()]).unwrap(), ..Scope::default() };
        let wider = Scope { repos: Patterns::new(&["acme/[ab]*".into()]).unwrap(), ..Scope::default() };
        assert!(wider.repos.matches("acme/alpha") && !outer.repos.matches("acme/alpha"));
        assert!(!outer.contains(&wider));
        let same = Scope { repos: Patterns::new(&["acme/*b*".into()]).unwrap(), ..Scope::default() };
        assert!(outer.contains(&same), "a scope always contains itself");
    }

    #[test]
    fn pattern_checks_need_a_role_that_covers_the_whole_pattern() {
        let p = Principal::for_tests("user:alice");
        let bindings = vec![binding("user:alice", "acme/*", Role::Admin)];
        assert!(Authorizer::check_pattern(&p, &bindings, "acme/web", Cap::CredentialsMint).is_ok());
        assert!(Authorizer::check_pattern(&p, &bindings, "acme/web*", Cap::CredentialsMint).is_ok());
        // `*` and `other/*` are not inside `acme/*`.
        assert!(Authorizer::check_pattern(&p, &bindings, "*", Cap::CredentialsMint).is_err());
        assert!(Authorizer::check_pattern(&p, &bindings, "other/*", Cap::CredentialsMint).is_err());
        // A binding with a clever glob proves nothing about a pattern it does not provably cover.
        let tricky = vec![binding("user:alice", "acme/*b*", Role::Admin)];
        assert!(Authorizer::check_pattern(&p, &tricky, "acme/[ab]*", Cap::CredentialsMint).is_err());
        // A weaker role is not enough.
        let reader = vec![binding("user:alice", "acme/*", Role::Reader)];
        assert_eq!(
            Authorizer::check_pattern(&p, &reader, "acme/web", Cap::CredentialsMint).unwrap_err().code(),
            ag_protocol::ErrorCode::PermissionDenied
        );
    }

    #[test]
    fn bindings_are_validated_and_an_unusable_one_matches_nothing() {
        assert!(validate_pattern("acme/*").is_ok());
        assert!(validate_pattern("user:alice").is_ok());
        assert!(validate_pattern("acme/[").is_err());
        assert!(validate_pattern("").is_err());
        let inert = Binding::new("user:alice", "acme/[", Role::Admin);
        assert!(!inert.is_usable());
        let p = Principal::for_tests("user:alice");
        assert!(Authorizer::roles(&p, &[inert], "acme/web").is_empty());
        assert!(Binding::new("agent:*", "*", Role::Reader).is_usable());
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
            run_id: None,
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
            run_id: None,
        };
        assert!(wide.contains(&narrow));
        assert!(!narrow.contains(&wide), "an unrestricted credential cannot be minted from a scoped one");
        let other = Scope { repos: Patterns::new(&["acme/api".into()]).unwrap(), ..Scope::default() };
        assert!(!narrow.contains(&other));
        let sub = Scope {
            repos: Patterns::new(&["acme/web".into()]).unwrap(),
            streams: Patterns::default(),
            operations: [Cap::RepoRead].into_iter().collect(),
            run_id: None,
        };
        assert!(narrow.contains(&sub));
    }

    fn wire(repos: &[&str], run: &str) -> ag_protocol::v1::CredentialScope {
        ag_protocol::v1::CredentialScope {
            repos: repos.iter().map(|r| r.to_string()).collect(),
            run_id: run.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn a_scope_bound_to_a_run_names_one_literal_repository_and_one_run() {
        let bound = Scope::from_wire(&wire(&["acme/web"], "run_ab12")).unwrap();
        assert_eq!(bound.bound_run(), Some(("acme/web", "run_ab12")));
        assert!(!bound.is_unrestricted());
        assert_eq!(bound.to_wire().run_id, "run_ab12", "the wire form carries the binding back");
        assert_eq!(Scope::from_wire(&wire(&["acme/web"], "")).unwrap().bound_run(), None);
        // The repository must be exactly one name, not a pattern: a run lives in one repository.
        for repos in [&[][..], &["acme/*"], &["*"], &["acme/web", "acme/api"], &["acme/we?"], &["acme"], &["acme/web*"]]
        {
            assert!(Scope::from_wire(&wire(repos, "run_ab12")).is_err(), "{repos:?} cannot hold a run binding");
        }
        for run in ["a b", "run/1", "run#1", &"r".repeat(81)] {
            assert!(Scope::from_wire(&wire(&["acme/web"], run)).is_err(), "{run:?} is not a run id");
        }
    }

    #[test]
    fn a_run_bound_scope_only_contains_scopes_bound_to_the_same_run() {
        let bound = Scope::from_wire(&wire(&["acme/web"], "run_a")).unwrap();
        assert!(bound.contains(&Scope::from_wire(&wire(&["acme/web"], "run_a")).unwrap()));
        // Not the same agent's other run, and not the agent without a run: that would be wider.
        assert!(!bound.contains(&Scope::from_wire(&wire(&["acme/web"], "run_b")).unwrap()));
        assert!(!bound.contains(&Scope::from_wire(&wire(&["acme/web"], "")).unwrap()));
        // An unbound scope contains a bound one (the binding only narrows).
        let wide = Scope { repos: Patterns::new(&["acme/*".into()]).unwrap(), ..Scope::default() };
        assert!(wide.contains(&bound));
    }

    #[test]
    fn the_delegator_holds_the_writers_powers_plus_delegation_and_nothing_administrative() {
        let delegator = Role::Delegator.caps();
        assert!(delegator.is_superset(&Role::Writer.caps()));
        assert!(delegator.contains(&Cap::CredentialsDelegate) && delegator.contains(&Cap::AgentOperate));
        for cap in [
            Cap::CredentialsMint,
            Cap::CredentialsList,
            Cap::CredentialsRevoke,
            Cap::RolesManage,
            Cap::PolicyWrite,
            Cap::RepoCreate,
            Cap::RepoConfigure,
            Cap::IntegrateRun,
            Cap::ProposalReview,
            Cap::GcRun,
        ] {
            assert!(!delegator.contains(&cap), "a delegator must not hold {}", cap.as_str());
        }
        for role in [Role::Reader, Role::Writer, Role::Reviewer, Role::Integrator, Role::Runner] {
            assert!(!role.caps().contains(&Cap::CredentialsDelegate) && !role.caps().contains(&Cap::AgentOperate));
        }
        assert_eq!(Role::parse("delegator"), Some(Role::Delegator));
    }

    #[test]
    fn every_capability_only_administrators_hold_is_never_delegated() {
        let others: BTreeSet<Cap> = Role::ALL
            .iter()
            .filter(|r| **r != Role::Admin)
            .flat_map(|r| r.caps())
            .filter(|c| ![Cap::CredentialsDelegate, Cap::AgentOperate].contains(c))
            .collect();
        for cap in Cap::ALL {
            if !others.contains(cap) {
                assert!(NEVER_DELEGATED.contains(cap), "{} is administrative but could be delegated", cap.as_str());
            }
        }
        // Nothing a plain agent needs is on the list.
        for cap in Role::Writer.caps() {
            assert!(!NEVER_DELEGATED.contains(&cap), "{} is a writer's capability", cap.as_str());
        }
    }

    #[test]
    fn a_delegator_can_pass_on_what_it_holds_where_it_holds_it_and_no_more() {
        let p = Principal::for_tests("service:orchestrator");
        let b = vec![
            binding("service:orchestrator", "acme/*", Role::Delegator),
            binding("service:orchestrator", "acme/web", Role::Integrator),
        ];
        let on_web = Authorizer::delegable(&p, &b, "acme/web");
        assert!(on_web.contains(&Cap::ObjectsWrite) && on_web.contains(&Cap::IntegrateRun), "integrator too, there");
        assert!(!on_web.contains(&Cap::CredentialsDelegate) && !on_web.contains(&Cap::AgentOperate));
        let on_api = Authorizer::delegable(&p, &b, "acme/api");
        assert!(on_api.contains(&Cap::ObjectsWrite) && !on_api.contains(&Cap::IntegrateRun), "only a delegator there");
        // The pattern covering both repositories is only as strong as the weaker role there.
        let both = Authorizer::delegable(&p, &b, "acme/*");
        assert!(both.contains(&Cap::ObjectsWrite) && !both.contains(&Cap::IntegrateRun));
        // Outside every binding, nothing; and the delegator's own credential scope narrows it further.
        assert!(Authorizer::delegable(&p, &b, "other/web").is_empty());
        let mut narrow = p.clone();
        narrow.scope.operations = [Cap::ObjectsWrite, Cap::Publish, Cap::IntegrateRun].into_iter().collect();
        let got = Authorizer::delegable(&narrow, &b, "acme/web");
        assert_eq!(got, [Cap::ObjectsWrite, Cap::Publish, Cap::IntegrateRun].into_iter().collect::<BTreeSet<_>>());
    }

    #[test]
    fn group_bindings_apply() {
        let mut p = Principal::for_tests("user:carol");
        p.groups = vec!["platform".into()];
        let b = vec![binding("group:platform", "acme/*", Role::Integrator)];
        assert!(Authorizer::check(&p, &b, "acme/x", Cap::IntegrateRun, None).is_ok());
    }
}
