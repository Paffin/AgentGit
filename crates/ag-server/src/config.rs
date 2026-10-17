//! Server configuration (TOML file + environment overrides).

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
    /// Authentication.
    pub auth: AuthSection,
    /// Rate limits and sizes.
    pub limits: LimitsSection,
    /// Repository handling.
    pub repos: ReposSection,
    /// Prometheus metrics.
    pub metrics: MetricsSection,
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
    /// Directory with the built Web UI, served at `/` when set.
    pub web_dir: Option<PathBuf>,
    /// Origins allowed by CORS for browser clients served from elsewhere.
    pub cors_origins: Vec<String>,
    /// Externally visible base URL (for links in responses and OIDC redirects).
    pub public_url: Option<String>,
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
            public_url: None,
        }
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
    /// Trusted OIDC providers.
    pub oidc: Vec<OidcProvider>,
}

impl Default for AuthSection {
    fn default() -> Self {
        Self {
            bootstrap_token_env: "AG_BOOTSTRAP_TOKEN".into(),
            max_agent_ttl_secs: 3600,
            max_ttl_secs: 30 * 24 * 3600,
            oidc: Vec::new(),
        }
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
    /// Largest non-streaming request body.
    pub max_request_bytes: u64,
    /// Largest object envelope.
    pub max_object_bytes: u64,
    /// Requests handled concurrently; more are shed with `unavailable`.
    pub max_concurrent_requests: usize,
    /// Default per-request deadline when the client sets none (ms, 0 = none).
    pub default_deadline_ms: u64,
    /// Per-repository default for open proposals (0 = unlimited).
    pub max_open_proposals: u32,
    /// Per-repository default for total object bytes (0 = unlimited).
    pub max_total_object_bytes: u64,
}

impl Default for LimitsSection {
    fn default() -> Self {
        Self {
            requests_per_second: 200,
            burst: 400,
            max_request_bytes: ag_protocol::limits::MAX_REQUEST_BYTES,
            max_object_bytes: ag_protocol::limits::MAX_OBJECT_BYTES,
            max_concurrent_requests: 512,
            default_deadline_ms: 60_000,
            max_open_proposals: 0,
            max_total_object_bytes: 0,
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
        if self.limits.max_concurrent_requests == 0 {
            return bad("limits.max_concurrent_requests must be at least 1");
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

    #[test]
    fn tls_files_must_come_as_a_pair() {
        let mut c = ServerConfig::default();
        c.server.tls_cert = Some("a.pem".into());
        assert!(c.validate().is_err());
    }
}
