//! Server configuration: a TOML file. Secrets (the bootstrap and metrics tokens, database URLs, collector headers,
//! S3 keys) are never in the file; it names the environment variables that hold them.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Errors from loading the configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read.
    #[error("cannot read configuration {path}: {source}")]
    Read {
        /// File path.
        path: PathBuf,
        /// Cause.
        source: std::io::Error,
    },
    /// The file is not valid TOML for this schema.
    #[error("invalid configuration: {0}")]
    Parse(#[from] toml::de::Error),
    /// A value is not acceptable.
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

/// The whole configuration. Unknown keys are rejected so typos do not
/// silently weaken a setting.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ServerConfig {
    /// Listener, storage and TLS.
    pub server: ServerSection,
    /// Where credentials, roles and the repository registry live (SQLite file, or PostgreSQL for several replicas).
    pub control: ControlSection,
    /// Authentication.
    pub auth: AuthSection,
    /// Rate limits and sizes.
    pub limits: LimitsSection,
    /// Repository handling.
    pub repos: ReposSection,
    /// Prometheus metrics.
    pub metrics: MetricsSection,
    /// The Web UI.
    pub ui: crate::ui::UiSection,
    /// Logging and tracing.
    pub telemetry: TelemetrySection,
}

/// `[server]`
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ServerSection {
    /// Address for gRPC and HTTP (one port; routed by content type).
    pub listen: SocketAddr,
    /// Directory holding the control database and all repositories.
    pub data_dir: PathBuf,
    /// TLS certificate chain (PEM). Together with `tls_key` enables HTTPS and gRPC over TLS.
    pub tls_cert: Option<PathBuf>,
    /// TLS private key (PEM).
    pub tls_key: Option<PathBuf>,
    /// Allow plaintext on a non-loopback address. Off by default: credentials
    /// would cross the network in the clear.
    pub allow_insecure_remote: bool,
    /// Deprecated spelling of `ui.dir`: directory with the built Web UI, served at `/` when set.
    pub web_dir: Option<PathBuf>,
    /// Origins allowed by CORS for browser clients served from elsewhere.
    pub cors_origins: Vec<String>,
    /// On SIGTERM, report "not ready" for this many seconds before closing the listener, so a load balancer
    /// stops sending new requests first (0 = stop at once). Set it just above the balancer's probe interval
    /// when running several replicas; keep it below the orchestrator's termination grace period.
    pub drain_secs: u64,
    /// Name of this replica in logs and metrics. Default: `$HOSTNAME` (the pod name on Kubernetes), else random.
    pub replica_id: Option<String>,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8080".parse().expect("valid"),
            data_dir: PathBuf::from("./ag-data"),
            tls_cert: None,
            tls_key: None,
            allow_insecure_remote: false,
            web_dir: None,
            cors_origins: Vec::new(),
            drain_secs: 0,
            replica_id: None,
        }
    }
}

/// `[control]` — the control plane database: credentials, role bindings, the repository registry.
///
/// Without `url`/`url_env` it is the SQLite file `<data_dir>/control.db`, which only one server process may use.
/// With one of them it is a schema of a PostgreSQL database, shared by any number of replicas
/// (see `docs/operations/high-availability.md`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ControlSection {
    /// PostgreSQL connection URL without a password (for example `postgres://ag@db.internal/agentgit?sslmode=verify-full`,
    /// or `host=/var/run/postgresql dbname=agentgit`). A URL with a password is refused: use `url_env`.
    pub url: Option<String>,
    /// Name of the environment variable that holds the connection URL (preferred: it usually has a password).
    pub url_env: Option<String>,
    /// Schema that holds the control tables (created on first start). Lowercase letters, digits and `_`.
    pub schema: String,
    /// Pooled connections to the control database per replica.
    pub pool_size: usize,
    /// How long a starting replica keeps retrying an unreachable control database before giving up (seconds).
    pub startup_timeout_secs: u64,
    /// How long a replica may use cached role bindings (milliseconds). A role granted or revoked through
    /// another replica is effective within this time at the latest; usually at once (cluster notice).
    pub bindings_ttl_ms: u64,
}

impl Default for ControlSection {
    fn default() -> Self {
        Self {
            url: None,
            url_env: None,
            schema: "ag_control".into(),
            pool_size: 16,
            startup_timeout_secs: 30,
            bindings_ttl_ms: 5000,
        }
    }
}

impl ControlSection {
    /// The PostgreSQL URL when the control plane is on PostgreSQL (`None`: SQLite). Reads the environment
    /// variable `url_env` names.
    pub fn resolve_url(&self) -> Result<Option<String>, ConfigError> {
        if let Some(u) = self.url.as_deref().filter(|u| !u.trim().is_empty()) {
            return Ok(Some(u.trim().to_string()));
        }
        let Some(var) = self.url_env.as_deref().filter(|v| !v.is_empty()) else { return Ok(None) };
        match std::env::var(var) {
            Ok(u) if !u.trim().is_empty() => Ok(Some(u.trim().to_string())),
            _ => Err(ConfigError::Invalid(format!(
                "control.url_env names {var}, but that environment variable is not set (it must hold the PostgreSQL URL)"
            ))),
        }
    }

    /// True if this section selects PostgreSQL.
    pub fn is_postgres(&self) -> bool {
        self.url.as_deref().is_some_and(|u| !u.trim().is_empty())
            || self.url_env.as_deref().is_some_and(|v| !v.is_empty())
    }
}

/// `[auth]`
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AuthSection {
    /// Name of the environment variable holding the bootstrap administrator
    /// token. Remove it once real credentials exist.
    pub bootstrap_token_env: String,
    /// Longest lifetime of a minted agent credential.
    pub max_agent_ttl_secs: u32,
    /// Longest lifetime of other minted credentials.
    pub max_ttl_secs: u32,
    /// Lifetime of a minted agent credential when the request names none (capped by `max_agent_ttl_secs`).
    pub default_agent_ttl_secs: u32,
    /// Lifetime of any other minted credential when the request names none (capped by `max_ttl_secs`).
    pub default_ttl_secs: u32,
    /// Trusted OIDC providers.
    pub oidc: Vec<OidcProvider>,
}

impl Default for AuthSection {
    fn default() -> Self {
        Self {
            bootstrap_token_env: "AG_BOOTSTRAP_TOKEN".into(),
            max_agent_ttl_secs: 3600,
            max_ttl_secs: 30 * 24 * 3600,
            default_agent_ttl_secs: 3600,
            default_ttl_secs: 24 * 3600,
            oidc: Vec::new(),
        }
    }
}

impl AuthSection {
    /// The lifetime, in seconds, of a credential minted for an identity of `kind`: `requested` if it is within
    /// the cap for that kind, the configured default for the kind if `requested` is 0. One rule for the API and
    /// for `ag-server admin mint`.
    pub fn credential_ttl(&self, kind: ag_types::IdentityKind, requested: u32) -> Result<u32, String> {
        let agent = kind == ag_types::IdentityKind::Agent;
        let (max, default) = if agent {
            (self.max_agent_ttl_secs, self.default_agent_ttl_secs)
        } else {
            (self.max_ttl_secs, self.default_ttl_secs)
        };
        let ttl = if requested == 0 { default.min(max) } else { requested };
        if ttl > max {
            return Err(format!("ttl_secs may be at most {max} for {} credentials", kind.prefix()));
        }
        Ok(ttl)
    }
}

/// One trusted OpenID Connect provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcProvider {
    /// The `iss` claim tokens must carry.
    pub issuer: String,
    /// The `aud` claim tokens must carry.
    pub audience: String,
    /// JWKS URL. Defaults to discovery (`<issuer>/.well-known/openid-configuration`).
    #[serde(default)]
    pub jwks_uri: Option<String>,
    /// Static JWKS file (offline deployments, tests). Takes precedence over `jwks_uri`.
    #[serde(default)]
    pub jwks_file: Option<PathBuf>,
    /// Claim used for the user name (`user:<claim>`).
    #[serde(default = "default_username_claim")]
    pub username_claim: String,
    /// Claim listing the user's groups (array of strings).
    #[serde(default = "default_groups_claim")]
    pub groups_claim: String,
    /// Signature algorithms accepted from this provider.
    #[serde(default = "default_algs")]
    pub algorithms: Vec<String>,
    /// Maps provider groups to roles.
    #[serde(default)]
    pub group_roles: Vec<GroupRole>,
}

fn default_username_claim() -> String {
    "preferred_username".into()
}

fn default_groups_claim() -> String {
    "groups".into()
}

fn default_algs() -> Vec<String> {
    vec!["RS256".into(), "ES256".into(), "EdDSA".into()]
}

/// Grants `role` on repositories matching `repo` to members of `group`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupRole {
    /// Group name as it appears in the groups claim.
    pub group: String,
    /// One of reader, writer, reviewer, integrator, runner, admin.
    pub role: String,
    /// Repository pattern, e.g. `acme/*` or `*`.
    #[serde(default = "star")]
    pub repo: String,
}

fn star() -> String {
    "*".into()
}

/// `[limits]`
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LimitsSection {
    /// Sustained requests per second per caller (0 = unlimited).
    pub requests_per_second: u32,
    /// Burst size per caller.
    pub burst: u32,
    /// Largest non-streaming request body (HTTP body and gRPC message alike).
    pub max_request_bytes: u64,
    /// Largest object envelope. At most what the engine can store (`engine_max_object_bytes`).
    pub max_object_bytes: u64,
    /// Requests handled concurrently; more are shed with `unavailable`.
    pub max_concurrent_requests: usize,
    /// Default per-request deadline when the client sets none (ms, 0 = none).
    pub default_deadline_ms: u64,
    /// Per-repository default for open proposals (0 = unlimited).
    pub max_open_proposals: u32,
    /// Per-repository default for total object bytes (0 = unlimited).
    pub max_total_object_bytes: u64,
    /// Per-repository default for the largest single object (0 = only the server-wide `max_object_bytes`).
    pub repo_max_object_bytes: u64,
    /// Per-repository default for the number of streams (0 = unlimited).
    pub repo_max_streams: u32,
    /// Per-repository default for requests per second per caller (0 = only the server-wide limit).
    pub repo_requests_per_second: u32,
    /// Per-repository default burst for `repo_requests_per_second`.
    pub repo_burst: u32,
}

/// The largest object envelope the engine can store: its blob limit plus the envelope header. The one source
/// of the number the server defaults, validates and advertises as `max_object_bytes`.
pub fn engine_max_object_bytes() -> u64 {
    ag_types::Limits::default().max_blob_size + ag_object::ENVELOPE_HEADER_LEN as u64
}

impl Default for LimitsSection {
    fn default() -> Self {
        Self {
            requests_per_second: 200,
            burst: 400,
            max_request_bytes: ag_protocol::limits::MAX_REQUEST_BYTES,
            max_object_bytes: engine_max_object_bytes(),
            max_concurrent_requests: 512,
            default_deadline_ms: 60_000,
            max_open_proposals: 0,
            max_total_object_bytes: 0,
            repo_max_object_bytes: 0,
            repo_max_streams: 0,
            repo_requests_per_second: 0,
            repo_burst: 0,
        }
    }
}

/// `[repos]`
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ReposSection {
    /// Database connections per open repository.
    pub pool_size: usize,
    /// Repositories kept open at once (least recently used are closed).
    pub max_open: usize,
    /// Default stream for new repositories.
    pub default_stream: String,
    /// Storage for new repositories. Absent: a SQLite file and a directory per
    /// repository under the data directory. With PostgreSQL, `meta.schema` is a
    /// *prefix* and each repository gets its own schema (`<prefix>_<owner>_<name>`);
    /// with S3, each repository gets the key prefix `<prefix>/<owner>/<name>`.
    /// Credentials are read from the environment variables the section names.
    pub storage: Option<ag_integrate::StorageConfig>,
}

impl Default for ReposSection {
    fn default() -> Self {
        Self { pool_size: 4, max_open: 256, default_stream: "main".into(), storage: None }
    }
}

/// `[metrics]`
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MetricsSection {
    /// Serve `/metrics`.
    pub enabled: bool,
    /// Environment variable with a bearer token required for `/metrics`
    /// (unset or empty variable = open to anyone who can reach the port).
    pub token_env: String,
}

impl Default for MetricsSection {
    fn default() -> Self {
        Self { enabled: true, token_env: "AG_METRICS_TOKEN".into() }
    }
}

/// `[telemetry]`
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TelemetrySection {
    /// `text` or `json`.
    pub log_format: String,
    /// `tracing` filter directive, e.g. `info,ag_server=debug`.
    pub log_filter: String,
    /// OTLP/HTTP endpoint for traces (e.g. `http://otel-collector:4318`); empty = off.
    pub otlp_endpoint: String,
    /// Service name reported to the collector.
    pub service_name: String,
    /// Fraction of new traces to record, 0.0 to 1.0. A trace that arrives with a
    /// `traceparent` follows the caller's sampling decision instead.
    pub otlp_sample_ratio: f64,
    /// Environment variable with extra collector headers, `name=value,name2=value2`
    /// (the standard `OTEL_EXPORTER_OTLP_HEADERS`). The value is a secret and is never logged.
    pub otlp_headers_env: String,
}

impl Default for TelemetrySection {
    fn default() -> Self {
        Self {
            log_format: "text".into(),
            log_filter: "info".into(),
            otlp_endpoint: String::new(),
            service_name: "agentgit-server".into(),
            otlp_sample_ratio: 1.0,
            otlp_headers_env: "OTEL_EXPORTER_OTLP_HEADERS".into(),
        }
    }
}

/// A CORS origin is `scheme://host[:port]`: http or https, a host, no path, query or credentials. (Browsers send
/// exactly that in `Origin`, so anything else could never match and would only look like a working setting.)
fn check_cors_origin(origin: &str) -> Result<(), &'static str> {
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .ok_or("must start with http:// or https:// (list explicit origins; '*' is not supported)")?;
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', ' ']) {
        return Err("must be scheme://host[:port] with no path, query or credentials");
    }
    if axum::http::HeaderValue::from_str(origin).is_err() {
        return Err("is not a valid header value");
    }
    Ok(())
}

impl ServerConfig {
    /// Loads a configuration file.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text =
            std::fs::read_to_string(path).map_err(|source| ConfigError::Read { path: path.to_path_buf(), source })?;
        let cfg: ServerConfig = toml::from_str(&text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Checks cross-field constraints.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let bad = |m: &str| Err(ConfigError::Invalid(m.to_string()));
        if self.server.tls_cert.is_some() != self.server.tls_key.is_some() {
            return bad("tls_cert and tls_key must be set together");
        }
        if !self.tls_enabled() && !self.server.listen.ip().is_loopback() && !self.server.allow_insecure_remote {
            return bad("refusing to listen on a non-loopback address without TLS: set tls_cert/tls_key, \
                 or allow_insecure_remote = true if TLS is terminated in front of the server");
        }
        if self.repos.pool_size == 0 || self.repos.max_open == 0 {
            return bad("repos.pool_size and repos.max_open must be at least 1");
        }
        let c = &self.control;
        if c.url.as_deref().is_some_and(|u| !u.trim().is_empty()) && c.url_env.as_deref().is_some_and(|v| !v.is_empty())
        {
            return bad("set either control.url or control.url_env, not both");
        }
        if let Some(u) = c.url.as_deref()
            && ag_storage::meta::url_has_password(u)
        {
            return bad(
                "control.url must not contain a password: put the URL in an environment variable and name it with control.url_env",
            );
        }
        if ag_storage::meta::validate_schema_name(&c.schema).is_err() {
            return bad(
                "control.schema must be lowercase letters, digits and '_' (at most 63 bytes, not starting with a digit or 'pg_')",
            );
        }
        if c.pool_size == 0 || c.pool_size > 512 {
            return bad("control.pool_size must be between 1 and 512");
        }
        if c.bindings_ttl_ms > 300_000 {
            return bad("control.bindings_ttl_ms must be at most 300000 (5 minutes)");
        }
        if self.server.drain_secs > 300 {
            return bad("server.drain_secs must be at most 300");
        }
        if let Some(id) = &self.server.replica_id
            && (id.is_empty() || id.len() > 63 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)))
        {
            return bad("server.replica_id must be 1-63 characters of [A-Za-z0-9._-]");
        }
        if self.limits.max_concurrent_requests == 0 {
            return bad("limits.max_concurrent_requests must be at least 1");
        }
        let l = &self.limits;
        if l.max_request_bytes == 0 {
            return bad("limits.max_request_bytes must be positive");
        }
        let engine = engine_max_object_bytes();
        if l.max_object_bytes == 0 || l.max_object_bytes > engine {
            return Err(ConfigError::Invalid(format!(
                "limits.max_object_bytes must be between 1 and {engine} (the largest object the engine can store)"
            )));
        }
        if !matches!(self.telemetry.log_format.as_str(), "text" | "json") {
            return Err(ConfigError::Invalid(format!(
                "telemetry.log_format must be 'text' or 'json', not '{}'",
                self.telemetry.log_format
            )));
        }
        if let Err(e) = ag_types::StreamName::new(&self.repos.default_stream) {
            return Err(ConfigError::Invalid(format!("repos.default_stream '{}': {e}", self.repos.default_stream)));
        }
        for origin in &self.server.cors_origins {
            if let Err(why) = check_cors_origin(origin) {
                return Err(ConfigError::Invalid(format!("server.cors_origins entry '{origin}': {why}")));
            }
        }
        let a = &self.auth;
        if a.max_ttl_secs == 0 || a.max_agent_ttl_secs == 0 || a.default_ttl_secs == 0 || a.default_agent_ttl_secs == 0
        {
            return bad(
                "auth.max_ttl_secs, max_agent_ttl_secs, default_ttl_secs and default_agent_ttl_secs must be positive",
            );
        }
        if !self.telemetry.otlp_endpoint.is_empty() {
            let e = &self.telemetry.otlp_endpoint;
            let rest = e.strip_prefix("http://").or_else(|| e.strip_prefix("https://"));
            match rest {
                None => return bad("telemetry.otlp_endpoint must be an http:// or https:// URL"),
                Some(r) if r.is_empty() || r.contains('@') => {
                    return bad(
                        "telemetry.otlp_endpoint needs a host and must not contain credentials (use otlp_headers_env)",
                    );
                }
                Some(_) => {}
            }
            if !(0.0..=1.0).contains(&self.telemetry.otlp_sample_ratio) {
                return bad("telemetry.otlp_sample_ratio must be between 0 and 1");
            }
        }
        crate::ui::validate(self)?;
        for p in &self.auth.oidc {
            if p.issuer.is_empty() || p.audience.is_empty() {
                return bad("every [[auth.oidc]] needs issuer and audience");
            }
            for g in &p.group_roles {
                if crate::authz::Role::parse(&g.role).is_none() {
                    return Err(ConfigError::Invalid(format!("unknown role '{}' in group_roles", g.role)));
                }
            }
        }
        Ok(())
    }

    /// True if new repositories keep their metadata in PostgreSQL *and* their objects in S3, i.e. if every
    /// replica can open them. Anything else lives on the local disk of one host.
    pub fn repos_are_shared(&self) -> bool {
        use ag_integrate::{MetaConfig, ObjectsConfig};
        self.repos.storage.as_ref().is_some_and(|s| {
            matches!(s.meta, MetaConfig::Postgres { .. }) && matches!(s.objects, ObjectsConfig::S3 { .. })
        })
    }

    /// Whether TLS is configured.
    pub fn tls_enabled(&self) -> bool {
        self.server.tls_cert.is_some() && self.server.tls_key.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_unknown_keys_rejected() {
        ServerConfig::default().validate().unwrap();
        assert!(toml::from_str::<ServerConfig>("[server]\nlisen = \"x\"").is_err());
    }

    #[test]
    fn plaintext_on_a_public_address_is_refused() {
        let mut c = ServerConfig::default();
        c.server.listen = "0.0.0.0:8080".parse().unwrap();
        assert!(c.validate().is_err());
        c.server.allow_insecure_remote = true;
        c.validate().unwrap();
    }

    fn invalid(mutate: impl FnOnce(&mut ServerConfig)) -> String {
        let mut c = ServerConfig::default();
        mutate(&mut c);
        c.validate().unwrap_err().to_string()
    }

    #[test]
    fn a_setting_that_does_nothing_is_not_accepted_silently() {
        // `public_url` was declared, documented and never read; with `deny_unknown_fields` it is now a typo error.
        let e = toml::from_str::<ServerConfig>("[server]\npublic_url = \"https://ag.example\"").unwrap_err();
        assert!(e.to_string().contains("public_url"), "{e}");
    }

    #[test]
    fn values_that_would_misbehave_are_refused_at_start_up() {
        assert!(invalid(|c| c.telemetry.log_format = "yaml".into()).contains("log_format"));
        assert!(invalid(|c| c.repos.default_stream = "bad stream!".into()).contains("default_stream"));
        assert!(invalid(|c| c.limits.max_request_bytes = 0).contains("max_request_bytes"));
        assert!(invalid(|c| c.limits.max_object_bytes = 0).contains("max_object_bytes"));
        let too_big = engine_max_object_bytes() + 1;
        assert!(
            invalid(|c| c.limits.max_object_bytes = too_big).contains(&engine_max_object_bytes().to_string()),
            "the error names the engine's limit"
        );
        assert!(invalid(|c| c.auth.max_ttl_secs = 0).contains("positive"));
        for bad in ["ag.example", "https://", "https://ag.example/ui", "ftp://ag.example", "*", "https://u@ag.example"]
        {
            assert!(invalid(|c| c.server.cors_origins = vec![bad.into()]).contains("cors_origins"), "{bad}");
        }
        let mut ok = ServerConfig::default();
        ok.server.cors_origins = vec!["https://ag.example".into(), "http://localhost:5173".into()];
        ok.validate().unwrap();
    }

    #[test]
    fn the_largest_object_is_what_the_engine_can_store() {
        let engine = ag_types::Limits::default();
        assert_eq!(engine_max_object_bytes(), engine.max_blob_size + ag_object::ENVELOPE_HEADER_LEN as u64);
        assert_eq!(LimitsSection::default().max_object_bytes, engine_max_object_bytes());
        // The protocol crate's constant (what clients may assume at most) must not be below it.
        assert!(ag_protocol::limits::MAX_OBJECT_BYTES >= engine_max_object_bytes());
    }

    #[test]
    fn credential_lifetimes_follow_one_rule_for_every_caller() {
        use ag_types::IdentityKind::{Agent, User};
        let a = AuthSection::default();
        assert_eq!(a.credential_ttl(Agent, 0), Ok(a.default_agent_ttl_secs));
        assert_eq!(a.credential_ttl(User, 0), Ok(a.default_ttl_secs));
        assert_eq!(a.credential_ttl(Agent, 60), Ok(60));
        assert!(a.credential_ttl(Agent, a.max_agent_ttl_secs + 1).unwrap_err().contains("at most"));
        assert_eq!(a.credential_ttl(User, a.max_ttl_secs), Ok(a.max_ttl_secs));
        // A default above the cap is brought down to it, not refused.
        let low = AuthSection { default_ttl_secs: 10_000, max_ttl_secs: 600, ..AuthSection::default() };
        assert_eq!(low.credential_ttl(User, 0), Ok(600));
    }

    #[test]
    fn tls_files_must_come_as_a_pair() {
        let mut c = ServerConfig::default();
        c.server.tls_cert = Some("a.pem".into());
        assert!(c.validate().is_err());
    }
}
