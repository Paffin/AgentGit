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

struct KeyCache {
    keys: HashMap<String, DecodingKey>,
    /// Keys without a `kid` (single-key sets).
    anonymous: Vec<DecodingKey>,
    fetched: Option<Instant>,
    /// The last time a fetch was started, successful or not: an unreachable provider is asked once per
    /// `jwks_refresh_secs`, not once per request.
    attempted: Option<Instant>,
}

/// Validates tokens of one provider.
pub struct OidcValidator {
    cfg: OidcProvider,
    algorithms: Vec<Algorithm>,
    /// The keys. Held only to read or to install them - never across a network call.
    cache: Mutex<KeyCache>,
    /// Lets one request at a time fetch the key set; the others wait for its result instead of fetching too.
    refresh_gate: Mutex<()>,
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
            cache: Mutex::new(KeyCache { keys: HashMap::new(), anonymous: Vec::new(), fetched: None, attempted: None }),
            refresh_gate: Mutex::new(()),
        })
    }

    fn load_jwks(&self) -> ApiResult<JwkSet> {
        if let Some(path) = &self.cfg.jwks_file {
            let text = std::fs::read_to_string(path)
                .map_err(|e| ApiError::internal(format!("cannot read JWKS file {}: {e}", path.display())))?;
            return serde_json::from_str(&text).map_err(|e| ApiError::internal(format!("bad JWKS file: {e}")));
        }
        let timeout = Duration::from_secs(self.cfg.http_timeout_secs);
        let agent = ureq::Agent::config_builder().timeout_global(Some(timeout)).build().new_agent();
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

    /// Replaces the cached keys with a freshly fetched set.
    fn install(&self, set: &JwkSet) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
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
    }

    /// Looks up the verification key for `kid`, refreshing the key set when the key is unknown (rate limited by
    /// `jwks_refresh_secs`) or the cache is older than `jwks_max_age_secs`.
    ///
    /// The provider is contacted without holding the cache: a slow or unreachable identity provider delays the
    /// requests that need a refresh (one fetch at a time), but never those that find their key in the cache.
    fn key_for(&self, kid: Option<&str>) -> ApiResult<DecodingKey> {
        let (max_age, min_interval) =
            (Duration::from_secs(self.cfg.jwks_max_age_secs), Duration::from_secs(self.cfg.jwks_refresh_secs));
        let lookup = |c: &KeyCache| match kid {
            Some(k) => c.keys.get(k).cloned(),
            None => c
                .anonymous
                .first()
                .cloned()
                .or_else(|| (c.keys.len() == 1).then(|| c.keys.values().next().cloned()).flatten()),
        };
        // Returns the key if it is cached and fresh, else whether a refresh is allowed now.
        let cached = || {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            let stale = cache.fetched.is_none_or(|t| t.elapsed() > max_age);
            match lookup(&cache) {
                Some(k) if !stale => Ok(k),
                _ => Err(cache.attempted.is_none_or(|t| t.elapsed() > min_interval)),
            }
        };
        let may_refresh = match cached() {
            Ok(k) => return Ok(k),
            Err(may) => may,
        };
        if may_refresh {
            let _gate = self.refresh_gate.lock().unwrap_or_else(|e| e.into_inner());
            // Someone else may have refreshed while this request waited for the gate.
            match cached() {
                Ok(k) => return Ok(k),
                Err(true) => {
                    self.cache.lock().unwrap_or_else(|e| e.into_inner()).attempted = Some(Instant::now());
                    let set = self.load_jwks()?;
                    self.install(&set);
                }
                Err(false) => {}
            }
        }
        lookup(&self.cache.lock().unwrap_or_else(|e| e.into_inner()))
            .ok_or_else(|| ApiError::new(ag_protocol::ErrorCode::Unauthenticated, "unknown signing key"))
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
        validation.leeway = self.cfg.leeway_secs;
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

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::sync::Arc;

    use jsonwebtoken::EncodingKey;
    use jsonwebtoken::jwk::Jwk;

    use super::*;

    /// A provider whose key endpoint accepts connections and never answers (a hung identity provider), with
    /// one key already cached under "k1". Returns the validator and how many connections the endpoint saw.
    fn hung_provider(http_timeout_secs: u64) -> (Arc<OidcValidator>, Arc<std::sync::atomic::AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/jwks", listener.local_addr().unwrap());
        let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = seen.clone();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for conn in listener.incoming().flatten() {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                held.push(conn); // never answered, never closed
            }
        });
        let validator = OidcValidator::new(OidcProvider {
            issuer: "https://idp.example".into(),
            audience: "agentgit".into(),
            jwks_uri: Some(url),
            jwks_file: None,
            username_claim: "preferred_username".into(),
            groups_claim: "groups".into(),
            algorithms: vec!["EdDSA".into()],
            group_roles: vec![],
            http_timeout_secs,
            jwks_refresh_secs: 30,
            jwks_max_age_secs: 3600,
            leeway_secs: 30,
        })
        .unwrap();
        let pem = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/oidc-test-ed25519.pem")).unwrap();
        let mut jwk = Jwk::from_encoding_key(&EncodingKey::from_ed_pem(&pem).unwrap(), Algorithm::EdDSA).unwrap();
        jwk.common.key_id = Some("k1".into());
        validator.install(&JwkSet { keys: vec![jwk] });
        (Arc::new(validator), seen)
    }

    #[test]
    fn a_hung_identity_provider_does_not_stall_requests_whose_key_is_cached() {
        let (validator, seen) = hung_provider(2);
        // A token with an unknown key id makes this thread fetch the key set - and wait for the provider.
        let fetching = {
            let v = validator.clone();
            std::thread::spawn(move || v.key_for(Some("rotated")).is_err())
        };
        while seen.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
        // While it waits, a request with a cached key is answered at once (it used to queue behind the lock).
        let started = Instant::now();
        assert!(validator.key_for(Some("k1")).is_ok());
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "waited {:?} for a provider it did not need",
            started.elapsed()
        );
        assert!(fetching.join().unwrap(), "the request that needed a fresh key set gets the provider's failure");
    }

    #[test]
    fn an_unreachable_provider_is_asked_once_per_interval_not_once_per_request() {
        let (validator, seen) = hung_provider(1);
        let started = Instant::now();
        assert!(validator.key_for(Some("rotated")).is_err());
        assert!(started.elapsed() >= Duration::from_millis(900), "the first request really asked the provider");
        let second = Instant::now();
        for _ in 0..5 {
            assert!(validator.key_for(Some("rotated")).is_err());
        }
        assert!(second.elapsed() < Duration::from_millis(500), "later requests did not queue for the provider again");
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1, "one attempt in the interval");
    }
}
