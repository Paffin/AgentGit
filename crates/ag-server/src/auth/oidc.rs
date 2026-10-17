//! OpenID Connect bearer-token validation.
//!
//! The server never talks to users' browsers for login: clients obtain an
//! ID/access token from their identity provider and present it as a bearer
//! token. Validation is strict: the issuer and audience must match the
//! configuration, the algorithm must be on the provider's allow-list (so
//! `none` and algorithm-confusion attacks are impossible), `exp` is
//! required, and keys come only from the provider's own JWKS.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ag_types::Identity;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde_json::Value;

use super::{AuthMethod, Principal};
use crate::authz::{Role, Scope};
use crate::config::OidcProvider;
use crate::error::{ApiError, ApiResult};

const KEY_REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(30);
const KEY_MAX_AGE: Duration = Duration::from_secs(3600);

struct KeyCache {
    keys: HashMap<String, DecodingKey>,
    /// Keys without a `kid` (single-key sets).
    anonymous: Vec<DecodingKey>,
    fetched: Option<Instant>,
}

/// Validates tokens of one provider.
pub struct OidcValidator {
    cfg: OidcProvider,
    algorithms: Vec<Algorithm>,
    cache: Mutex<KeyCache>,
}

fn parse_alg(s: &str) -> Option<Algorithm> {
    Some(match s {
        "RS256" => Algorithm::RS256,
        "RS384" => Algorithm::RS384,
        "RS512" => Algorithm::RS512,
        "PS256" => Algorithm::PS256,
        "PS384" => Algorithm::PS384,
        "PS512" => Algorithm::PS512,
        "ES256" => Algorithm::ES256,
        "ES384" => Algorithm::ES384,
        "EdDSA" => Algorithm::EdDSA,
        _ => return None,
    })
}

impl OidcValidator {
    /// Creates a validator; algorithm names are checked up front.
    pub fn new(cfg: OidcProvider) -> ApiResult<Self> {
        let algorithms = cfg
            .algorithms
            .iter()
            .map(|a| parse_alg(a).ok_or_else(|| ApiError::invalid(format!("unsupported OIDC algorithm '{a}'"))))
            .collect::<ApiResult<Vec<_>>>()?;
        if algorithms.is_empty() {
            return Err(ApiError::invalid("an OIDC provider needs at least one algorithm"));
        }
        Ok(Self {
            cfg,
            algorithms,
            cache: Mutex::new(KeyCache { keys: HashMap::new(), anonymous: Vec::new(), fetched: None }),
        })
    }

    fn load_jwks(&self) -> ApiResult<JwkSet> {
        if let Some(path) = &self.cfg.jwks_file {
            let text = std::fs::read_to_string(path)
                .map_err(|e| ApiError::internal(format!("cannot read JWKS file {}: {e}", path.display())))?;
            return serde_json::from_str(&text).map_err(|e| ApiError::internal(format!("bad JWKS file: {e}")));
        }
        let agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).build().new_agent();
        let uri = match &self.cfg.jwks_uri {
            Some(u) => u.clone(),
            None => {
                let url = format!("{}/.well-known/openid-configuration", self.cfg.issuer.trim_end_matches('/'));
                let doc: Value = agent
                    .get(&url)
                    .call()
                    .map_err(|e| ApiError::internal(format!("OIDC discovery failed: {e}")))?
                    .body_mut()
                    .read_json()
                    .map_err(|e| ApiError::internal(format!("OIDC discovery returned bad JSON: {e}")))?;
                if doc["issuer"].as_str().map(|i| i.trim_end_matches('/'))
                    != Some(self.cfg.issuer.trim_end_matches('/'))
                {
                    return Err(ApiError::internal("OIDC discovery document names a different issuer"));
                }
                doc["jwks_uri"]
                    .as_str()
                    .ok_or_else(|| ApiError::internal("OIDC discovery document has no jwks_uri"))?
                    .to_string()
            }
        };
        agent
            .get(&uri)
            .call()
            .map_err(|e| ApiError::internal(format!("fetching JWKS failed: {e}")))?
            .body_mut()
            .read_json()
            .map_err(|e| ApiError::internal(format!("JWKS is not valid JSON: {e}")))
    }

    fn refresh(&self, cache: &mut KeyCache) -> ApiResult<()> {
        let set = self.load_jwks()?;
        cache.keys.clear();
        cache.anonymous.clear();
        for jwk in &set.keys {
            let Ok(key) = DecodingKey::from_jwk(jwk) else { continue };
            match &jwk.common.key_id {
                Some(kid) => {
                    cache.keys.insert(kid.clone(), key);
                }
                None => cache.anonymous.push(key),
            }
        }
        cache.fetched = Some(Instant::now());
        Ok(())
    }

    /// Looks up the verification key for `kid`, refreshing the key set when
    /// the key is unknown (rate limited) or the cache is old.
    fn key_for(&self, kid: Option<&str>) -> ApiResult<DecodingKey> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let stale = cache.fetched.is_none_or(|t| t.elapsed() > KEY_MAX_AGE);
        let lookup = |c: &KeyCache| match kid {
            Some(k) => c.keys.get(k).cloned(),
            None => c
                .anonymous
                .first()
                .cloned()
                .or_else(|| (c.keys.len() == 1).then(|| c.keys.values().next().cloned()).flatten()),
        };
        if !stale && let Some(k) = lookup(&cache) {
            return Ok(k);
        }
        let may_refresh = cache.fetched.is_none_or(|t| t.elapsed() > KEY_REFRESH_MIN_INTERVAL);
        if may_refresh {
            self.refresh(&mut cache)?;
        }
        lookup(&cache).ok_or_else(|| ApiError::new(ag_protocol::ErrorCode::Unauthenticated, "unknown signing key"))
    }

    /// Verifies `token`. `Ok(None)` means the token was issued by someone
    /// else (try the next provider); `Err` means it claims to be ours but is
    /// invalid.
    pub async fn verify(self: &std::sync::Arc<Self>, token: &str) -> ApiResult<Option<Principal>> {
        // Peek at the unverified issuer only to route to the right provider;
        // every claim is re-checked after the signature is verified.
        let Ok(peek) = jsonwebtoken::dangerous::insecure_decode_claims::<Value>(token) else {
            return Ok(None);
        };
        if peek["iss"].as_str().map(|i| i.trim_end_matches('/')) != Some(self.cfg.issuer.trim_end_matches('/')) {
            return Ok(None);
        }
        let me = self.clone();
        let token = token.to_string();
        tokio::task::spawn_blocking(move || me.verify_blocking(&token)).await.map_err(ApiError::internal)?.map(Some)
    }

    fn verify_blocking(&self, token: &str) -> ApiResult<Principal> {
        let bad = |why: &str| {
            tracing::debug!(issuer = %self.cfg.issuer, reason = why, "OIDC token rejected");
            ApiError::new(ag_protocol::ErrorCode::Unauthenticated, "missing, invalid or expired credentials")
        };
        let header = decode_header(token).map_err(|_| bad("bad header"))?;
        if !self.algorithms.contains(&header.alg) {
            return Err(bad("algorithm not allowed"));
        }
        let key = self.key_for(header.kid.as_deref())?;
        let mut validation = Validation::new(header.alg);
        validation.algorithms = self.algorithms.clone();
        validation.set_issuer(&[self.cfg.issuer.as_str()]);
        validation.set_audience(&[self.cfg.audience.as_str()]);
        validation.set_required_spec_claims(&["exp", "iss", "aud"]);
        validation.validate_nbf = true;
        validation.leeway = 30;
        let data = decode::<Value>(token, &key, &validation).map_err(|e| bad(&e.to_string()))?;
        let claims = data.claims;

        let raw_name = claims[self.cfg.username_claim.as_str()]
            .as_str()
            .or_else(|| claims["sub"].as_str())
            .ok_or_else(|| bad("no user name claim"))?;
        let name: String = raw_name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || "._@+-".contains(c) { c } else { '-' })
            .take(128)
            .collect();
        let identity = Identity::new(format!("user:{name}")).map_err(|_| bad("unusable user name"))?;
        let groups: Vec<String> = claims[self.cfg.groups_claim.as_str()]
            .as_array()
            .map(|a| a.iter().filter_map(|g| g.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let mut group_roles = Vec::new();
        for m in &self.cfg.group_roles {
            if groups.iter().any(|g| g == &m.group)
                && let Some(role) = Role::parse(&m.role)
            {
                group_roles.push((role, m.repo.clone()));
            }
        }
        let exp_ms = claims["exp"].as_i64().unwrap_or(0).saturating_mul(1000);
        Ok(Principal {
            identity,
            method: AuthMethod::Oidc,
            credential_id: claims["jti"].as_str().or_else(|| claims["sub"].as_str()).unwrap_or("oidc").to_string(),
            expires_at_ms: exp_ms,
            scope: Scope::default(),
            bootstrap: false,
            groups,
            group_roles,
        })
    }
}
