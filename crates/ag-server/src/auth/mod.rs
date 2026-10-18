//! Authentication: who is calling?
//!
//! Three kinds of bearer credentials are accepted on both transports:
//!
//! * **scoped tokens** (`agt_…`) minted through the API: opaque, stored only
//!   as a hash, always expiring, revocable, and always at most as powerful as
//!   the identity that minted them — this is what agents and services use;
//! * **OIDC ID/access tokens** (JWTs) from configured providers — this is
//!   what people use;
//! * the **bootstrap token**, an administrator secret from the environment,
//!   for the very first setup.
//!
//! A credential only proves *who* is calling. What they may do is decided by
//! [`crate::authz`].

mod oidc;

use std::sync::Arc;

use ag_crypto::random_bytes;
use ag_storage::Clock;
use ag_types::{Identity, IdentityKind};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub use oidc::OidcValidator;

use crate::authz::{Role, Scope};
use crate::config::AuthSection;
use crate::control::Control;
use crate::error::{ApiError, ApiResult};

/// How the caller authenticated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMethod {
    /// A scoped token minted through the API.
    Token,
    /// An OIDC JWT.
    Oidc,
    /// The bootstrap administrator token.
    Bootstrap,
}

impl AuthMethod {
    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            AuthMethod::Token => "token",
            AuthMethod::Oidc => "oidc",
            AuthMethod::Bootstrap => "bootstrap",
        }
    }
}

/// An authenticated caller.
#[derive(Clone, Debug)]
pub struct Principal {
    /// The identity requests are attributed to.
    pub identity: Identity,
    /// How the caller authenticated.
    pub method: AuthMethod,
    /// Credential id (`cred_…`, the OIDC `jti`/`sub`, or `bootstrap`).
    pub credential_id: String,
    /// Expiry in ms (0 = none known).
    pub expires_at_ms: i64,
    /// Restrictions attached to the credential.
    pub scope: Scope,
    /// The bootstrap administrator (all powers, everywhere).
    pub bootstrap: bool,
    /// OIDC groups.
    pub groups: Vec<String>,
    /// Roles implied by OIDC group mappings: (role, repo pattern).
    pub group_roles: Vec<(Role, String)>,
}

impl Principal {
    /// `user`, `agent`, `service` or `system`.
    pub fn kind(&self) -> &'static str {
        self.identity.kind().prefix()
    }

    /// Whether this is an agent identity.
    pub fn is_agent(&self) -> bool {
        self.identity.kind() == IdentityKind::Agent
    }

    /// A principal for unit tests.
    #[cfg(test)]
    pub fn for_tests(identity: &str) -> Self {
        Self {
            identity: Identity::new(identity).expect("valid identity"),
            method: AuthMethod::Token,
            credential_id: "cred_test".into(),
            expires_at_ms: 0,
            scope: Scope::default(),
            bootstrap: false,
            groups: Vec::new(),
            group_roles: Vec::new(),
        }
    }
}

/// Token prefix of scoped tokens.
pub const TOKEN_PREFIX: &str = "agt_";

/// A freshly generated token and what is stored for it.
pub struct MintedToken {
    /// `cred_<16 hex>`.
    pub credential_id: String,
    /// The secret token to hand to the user (shown once).
    pub token: String,
    /// What to store (hash of the secret part).
    pub secret_hash: String,
}

/// Generates a new scoped token.
pub fn mint_token() -> ApiResult<MintedToken> {
    let id = hex::encode(random_bytes::<8>().map_err(ApiError::internal)?);
    let secret = URL_SAFE_NO_PAD.encode(random_bytes::<32>().map_err(ApiError::internal)?);
    Ok(MintedToken {
        credential_id: format!("cred_{id}"),
        secret_hash: hex::encode(Sha256::digest(secret.as_bytes())),
        token: format!("{TOKEN_PREFIX}{id}_{secret}"),
    })
}

fn split_token(token: &str) -> Option<(String, &str)> {
    let rest = token.strip_prefix(TOKEN_PREFIX)?;
    if rest.len() < 18 || rest.as_bytes()[16] != b'_' {
        return None;
    }
    let (id, secret) = (&rest[..16], &rest[17..]);
    if !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some((format!("cred_{id}"), secret))
}

fn unauthenticated() -> ApiError {
    ApiError::new(ag_protocol::ErrorCode::Unauthenticated, "missing, invalid or expired credentials")
}

/// Verifies bearer credentials.
pub struct Authenticator {
    control: Arc<Control>,
    clock: Arc<dyn Clock>,
    bootstrap_hash: Option<[u8; 32]>,
    oidc: Vec<Arc<OidcValidator>>,
}

impl Authenticator {
    /// Builds the authenticator. The bootstrap token is read from the
    /// environment variable named in the configuration.
    pub fn new(control: Arc<Control>, clock: Arc<dyn Clock>, cfg: &AuthSection) -> ApiResult<Self> {
        Self::with_bootstrap(control, clock, cfg, std::env::var(&cfg.bootstrap_token_env).ok())
    }

    /// Like [`Authenticator::new`] with the bootstrap token given explicitly.
    pub fn with_bootstrap(
        control: Arc<Control>,
        clock: Arc<dyn Clock>,
        cfg: &AuthSection,
        bootstrap: Option<String>,
    ) -> ApiResult<Self> {
        let bootstrap = bootstrap.filter(|t| !t.is_empty());
        if let Some(t) = &bootstrap
            && t.len() < 16
        {
            return Err(ApiError::invalid(format!(
                "the bootstrap token in {} is too short (use at least 16 characters, ideally 32+ random bytes)",
                cfg.bootstrap_token_env
            )));
        }
        let oidc =
            cfg.oidc.iter().map(|p| OidcValidator::new(p.clone()).map(Arc::new)).collect::<ApiResult<Vec<_>>>()?;
        Ok(Self { control, clock, bootstrap_hash: bootstrap.map(|t| Sha256::digest(t.as_bytes()).into()), oidc })
    }

    /// Whether the bootstrap administrator is enabled.
    pub fn bootstrap_enabled(&self) -> bool {
        self.bootstrap_hash.is_some()
    }

    /// Whether any OIDC provider is configured.
    pub fn oidc_enabled(&self) -> bool {
        !self.oidc.is_empty()
    }

    /// Authenticates a bearer token.
    pub async fn authenticate(&self, token: &str) -> ApiResult<Principal> {
        let token = token.trim();
        if token.is_empty() {
            return Err(unauthenticated());
        }
        if let Some(expected) = &self.bootstrap_hash {
            let got: [u8; 32] = Sha256::digest(token.as_bytes()).into();
            if bool::from(got.ct_eq(expected)) {
                return Ok(Principal {
                    identity: Identity::new("user:bootstrap-admin").expect("valid"),
                    method: AuthMethod::Bootstrap,
                    credential_id: "bootstrap".into(),
                    expires_at_ms: 0,
                    scope: Scope::default(),
                    bootstrap: true,
                    groups: Vec::new(),
                    group_roles: Vec::new(),
                });
            }
        }
        if token.starts_with(TOKEN_PREFIX) {
            return self.authenticate_token(token).await;
        }
        if token.matches('.').count() == 2 && !self.oidc.is_empty() {
            return self.authenticate_oidc(token).await;
        }
        Err(unauthenticated())
    }

    /// Checks that an already authenticated principal is still allowed to be one: a scoped token must not
    /// have been revoked (on any replica) or expired since. Used by long-lived streams.
    pub async fn still_active(&self, p: &Principal) -> ApiResult<()> {
        if p.method != AuthMethod::Token {
            return Ok(());
        }
        let (control, id) = (self.control.clone(), p.credential_id.clone());
        let cred = tokio::task::spawn_blocking(move || control.credential(&id)).await.map_err(ApiError::internal)??;
        match cred {
            Some(c) if c.revoked_at_ms.is_none() && c.expires_at_ms > self.clock.now().as_millis() => Ok(()),
            _ => Err(ApiError::new(ag_protocol::ErrorCode::Unauthenticated, "the credential was revoked or expired")),
        }
    }

    async fn authenticate_token(&self, token: &str) -> ApiResult<Principal> {
        let Some((id, secret)) = split_token(token) else {
            return Err(unauthenticated());
        };
        let control = self.control.clone();
        let cred = tokio::task::spawn_blocking(move || control.credential(&id)).await.map_err(ApiError::internal)??;
        let presented: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
        // Compare against a dummy when the credential does not exist, so the
        // time taken does not reveal whether the id is known.
        let stored = cred.as_ref().and_then(|c| hex::decode(&c.secret_hash).ok()).unwrap_or_else(|| vec![0u8; 32]);
        let secret_ok = bool::from(presented.as_slice().ct_eq(stored.as_slice()));
        let Some(cred) = cred else {
            return Err(unauthenticated());
        };
        let now = self.clock.now().as_millis();
        if !secret_ok || cred.revoked_at_ms.is_some() || cred.expires_at_ms <= now {
            return Err(unauthenticated());
        }
        let identity = Identity::new(cred.subject.clone()).map_err(|_| unauthenticated())?;
        let scope = Scope::from_wire(&cred.scope)?;
        let control = self.control.clone();
        let id = cred.id.clone();
        // Usage tracking is best effort and must not fail the request.
        let _ = tokio::task::spawn_blocking(move || control.touch_credential(&id, now)).await;
        Ok(Principal {
            identity,
            method: AuthMethod::Token,
            credential_id: cred.id,
            expires_at_ms: cred.expires_at_ms,
            scope,
            bootstrap: false,
            groups: Vec::new(),
            group_roles: Vec::new(),
        })
    }

    async fn authenticate_oidc(&self, token: &str) -> ApiResult<Principal> {
        for provider in &self.oidc {
            match provider.verify(token).await {
                Ok(Some(p)) => return Ok(p),
                Ok(None) => continue, // not this provider's token
                Err(e) => return Err(e),
            }
        }
        Err(unauthenticated())
    }
}

#[cfg(test)]
mod tests {
    use ag_storage::SystemClock;

    use super::*;
    use crate::control::CredentialRow;

    fn setup(ttl_ms: i64) -> (Authenticator, Arc<Control>, String, String) {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let control = Arc::new(Control::open_in_memory(clock.clone()).unwrap());
        let minted = mint_token().unwrap();
        let now = clock.now().as_millis();
        control
            .insert_credential(&CredentialRow {
                id: minted.credential_id.clone(),
                secret_hash: minted.secret_hash.clone(),
                subject: "agent:bot".into(),
                kind: "agent".into(),
                description: String::new(),
                scope: Default::default(),
                created_at_ms: now,
                expires_at_ms: now + ttl_ms,
                revoked_at_ms: None,
                last_used_ms: 0,
                created_by: "user:root".into(),
            })
            .unwrap();
        let auth = Authenticator::new(control.clone(), clock, &AuthSection::default()).unwrap();
        (auth, control, minted.token, minted.credential_id)
    }

    #[tokio::test]
    async fn a_bootstrap_token_that_is_too_short_to_be_a_secret_is_refused_at_start_up() {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let control = Arc::new(Control::open_in_memory(clock.clone()).unwrap());
        let cfg = AuthSection::default();
        let start =
            |token: &str| Authenticator::with_bootstrap(control.clone(), clock.clone(), &cfg, Some(token.into()));
        // 15 characters are refused, 16 are the shortest the server takes. (The deployment files ask for 32 random
        // characters; this is the floor below which the server refuses to start at all.)
        let err = start(&"x".repeat(15)).err().expect("a 15-character bootstrap token is refused");
        assert!(err.to_string().contains(&cfg.bootstrap_token_env) && err.to_string().contains("too short"), "{err}");
        let ok = start(&"x".repeat(16)).unwrap();
        assert!(ok.bootstrap_enabled());
        // an empty value is "not set", not "a token of length 0"
        assert!(!start("").unwrap().bootstrap_enabled());
    }

    #[tokio::test]
    async fn a_valid_token_authenticates_and_a_tampered_one_does_not() {
        let (auth, _c, token, id) = setup(60_000);
        let p = auth.authenticate(&token).await.unwrap();
        assert_eq!(p.identity.as_str(), "agent:bot");
        assert_eq!(p.credential_id, id);
        assert_eq!(p.method, AuthMethod::Token);

        let mut bad = token.clone();
        bad.pop();
        bad.push(if token.ends_with('A') { 'B' } else { 'A' });
        assert!(auth.authenticate(&bad).await.is_err());
        assert!(auth.authenticate("agt_0000000000000000_xxxx").await.is_err());
        assert!(auth.authenticate("").await.is_err());
        assert!(auth.authenticate("garbage").await.is_err());
    }

    #[tokio::test]
    async fn expired_and_revoked_tokens_are_rejected() {
        let (auth, c, token, id) = setup(-1);
        assert!(auth.authenticate(&token).await.is_err(), "expired");
        let (auth, c2, token2, id2) = setup(60_000);
        assert!(auth.authenticate(&token2).await.is_ok());
        assert!(c2.revoke_credential(&id2, 1).unwrap());
        assert!(auth.authenticate(&token2).await.is_err(), "revoked");
        let _ = (c, id);
    }

    #[tokio::test]
    async fn a_credential_whose_scope_cannot_be_read_never_authenticates_as_unrestricted() {
        let (auth, c, token, id) = setup(60_000);
        let set_scope = |json: &str| {
            c.database()
                .execute("UPDATE credentials SET scope_json = ?1 WHERE id = ?2", &ag_storage::sql_params![json, &id])
                .unwrap();
        };
        // A narrow credential authenticates, and stays narrow.
        set_scope(r#"{"repos":["acme/web"]}"#);
        let p = auth.authenticate(&token).await.unwrap();
        assert!(!p.scope.is_unrestricted() && p.scope.permits(Some("acme/web"), crate::authz::Cap::RepoRead, None));
        assert!(!p.scope.permits(Some("acme/api"), crate::authz::Cap::RepoRead, None));
        // The same row, damaged: the old code read it as the default scope - no restriction at all - and let the
        // token do everything its subject's roles allow.
        for damaged in ["not json", "", r#"{"repos": "acme/web"}"#, r#"{"repos": [1]}"#] {
            set_scope(damaged);
            assert!(auth.authenticate(&token).await.is_err(), "scope_json {damaged:?} must not authenticate");
            assert!(auth.still_active(&p).await.is_err(), "nor keep a stream open");
        }
    }

    #[test]
    fn minted_tokens_are_unique_and_stored_only_as_hashes() {
        let a = mint_token().unwrap();
        let b = mint_token().unwrap();
        assert_ne!(a.token, b.token);
        assert!(a.token.starts_with(TOKEN_PREFIX));
        assert!(!a.secret_hash.contains(&a.token));
        let (id, secret) = split_token(&a.token).unwrap();
        assert_eq!(id, a.credential_id);
        assert_eq!(hex::encode(Sha256::digest(secret.as_bytes())), a.secret_hash);
    }
}
