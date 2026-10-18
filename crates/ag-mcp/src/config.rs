//! Operator configuration: where the server is, which credential to use, and what the agent may do with it.

use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

/// A set of tools that are switched on or off together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    /// History, trees, files, blame, diffs, proposals, tasks, leases, the queue: nothing is changed.
    Read,
    /// Create a workspace, write files into it, sync it.
    Workspace,
    /// Open, update and close proposals; ask for validation.
    Propose,
    /// Runs, task claims and completion, leases.
    Coordinate,
    /// Approve, request changes or comment on a proposal.
    Review,
    /// Land a proposal on its stream. Off unless the operator passes `--enable-integrate`.
    Integrate,
}

impl Group {
    /// Every group.
    pub const ALL: [Group; 6] =
        [Group::Read, Group::Workspace, Group::Propose, Group::Coordinate, Group::Review, Group::Integrate];

    /// The name used in flags and messages.
    pub fn as_str(self) -> &'static str {
        match self {
            Group::Read => "read",
            Group::Workspace => "workspace",
            Group::Propose => "propose",
            Group::Coordinate => "coordinate",
            Group::Review => "review",
            Group::Integrate => "integrate",
        }
    }

    /// Parses the name of a group that `--allow` accepts. `integrate` and `admin` get an explanation.
    pub fn parse_allowed(s: &str) -> Result<Group, String> {
        match s.trim() {
            "read" => Ok(Group::Read),
            "workspace" => Ok(Group::Workspace),
            "propose" => Ok(Group::Propose),
            "coordinate" => Ok(Group::Coordinate),
            "review" => Ok(Group::Review),
            "integrate" => Err("`integrate` lands code on a stream; it cannot be switched on through --allow. \
                                Pass --enable-integrate explicitly if you really want the integrate tool"
                .into()),
            "admin" => Err("there is no `admin` group: policy, roles and credentials are deliberately not reachable \
                            through MCP"
                .into()),
            other => Err(format!("unknown group `{other}` (read, workspace, propose, coordinate, review)")),
        }
    }
}

impl fmt::Display for Group {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The groups switched on when `--allow` is not given.
pub const DEFAULT_GROUPS: [Group; 4] = [Group::Read, Group::Workspace, Group::Propose, Group::Coordinate];

/// Whether the calls of a run use a credential of their own (ADR-0010 in the AgentGit repository).
///
/// A credential bound to a run acts only as that run and stops working when the run ends, so two runs of one agent
/// (two sessions of an agent platform, say) cannot act for each other, and a credential that leaks from a finished
/// run is worthless. `ag-mcp` mints it for each run it starts, from the agent's own credential, and keeps it in
/// memory: the model never sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunCredentials {
    /// Bind each run when the server supports it and the credential is an agent's; otherwise carry on with the
    /// agent's own credential and say so in the result of `run_start` / `task_claim`.
    #[default]
    Auto,
    /// Never leave a run unbound: if no bound credential can be had, the run is ended as failed and the call
    /// refused.
    Required,
    /// Never bind: every call uses the agent's own credential.
    Off,
}

impl RunCredentials {
    /// Parses the value of `--run-credentials`.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim() {
            "auto" => Ok(Self::Auto),
            "required" => Ok(Self::Required),
            "off" => Ok(Self::Off),
            other => Err(format!("--run-credentials must be auto, required or off, not `{other}`")),
        }
    }
}

/// Where the credential comes from. It is never part of the command line.
#[derive(Clone)]
pub enum TokenSource {
    /// Read from a file each time the server answers `401` (so an operator can rotate it) and at start.
    File(PathBuf),
    /// Read from an environment variable once, at start.
    Env(String),
    /// Given directly (library use and tests).
    Fixed(String),
}

impl fmt::Debug for TokenSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenSource::File(p) => write!(f, "File({})", p.display()),
            TokenSource::Env(n) => write!(f, "Env({n})"),
            TokenSource::Fixed(_) => f.write_str("Fixed(<redacted>)"),
        }
    }
}

impl TokenSource {
    /// Loads the credential, trimmed and checked for plausibility.
    pub fn load(&self) -> Result<String, String> {
        let raw = match self {
            TokenSource::File(p) => {
                check_file_mode(p);
                std::fs::read_to_string(p)
                    .map_err(|e| format!("cannot read the credential file {}: {e}", p.display()))?
            }
            TokenSource::Env(name) => std::env::var(name).map_err(|_| {
                format!("the environment variable {name} is not set (it must hold the AgentGit credential)")
            })?,
            TokenSource::Fixed(t) => t.clone(),
        };
        let t = raw.trim().to_string();
        if t.is_empty() {
            return Err("the credential is empty".into());
        }
        if t.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err("the credential contains whitespace or control characters".into());
        }
        Ok(t)
    }

    /// True if the credential can be re-read after a `401`.
    pub fn refreshable(&self) -> bool {
        matches!(self, TokenSource::File(_))
    }
}

#[cfg(unix)]
fn check_file_mode(p: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(m) = std::fs::metadata(p)
        && m.permissions().mode() & 0o077 != 0
    {
        eprintln!(
            "ag-mcp: warning: {} is readable by other users (mode {:o}); a credential file should be 0600",
            p.display(),
            m.permissions().mode() & 0o777
        );
    }
}

#[cfg(not(unix))]
fn check_file_mode(_: &std::path::Path) {}

/// Everything the server needs to run.
#[derive(Clone, Debug)]
pub struct Config {
    /// Base URL of `ag-server`, e.g. `https://git.example.com`.
    pub server: String,
    /// The credential.
    pub token: TokenSource,
    /// Switched-on tool groups.
    pub groups: BTreeSet<Group>,
    /// Repositories this process may touch (`owner/name`, `owner/*`); empty = whatever the credential allows.
    pub repos: Vec<String>,
    /// Globs that every written path must match; empty = any path.
    pub write_paths: Vec<String>,
    /// Writes only under an active lease that the caller holds.
    pub require_lease: bool,
    /// Whether each run gets a credential of its own.
    pub run_credentials: RunCredentials,
    /// Bound on the repository-derived text in one response, in bytes.
    pub max_output_bytes: usize,
    /// Per-request timeout towards the server.
    pub timeout: Duration,
    /// Longest accepted JSON-RPC message.
    pub max_message_bytes: usize,
    /// Tool calls that may run at the same time.
    pub max_concurrent: usize,
    /// Longest lease a tool may ask for, in seconds.
    pub max_lease_seconds: u64,
    /// Allow plain `http://` to hosts other than loopback.
    pub insecure_http: bool,
    /// An extra PEM bundle of trusted certificate authorities.
    pub ca_file: Option<PathBuf>,
    /// Log level for stderr: `error`, `warn`, `info` or `debug`.
    pub log_level: String,
}

impl Config {
    /// A configuration with the defaults and the given server and credential.
    pub fn new(server: impl Into<String>, token: TokenSource) -> Self {
        Self {
            server: server.into(),
            token,
            groups: DEFAULT_GROUPS.into_iter().collect(),
            repos: Vec::new(),
            write_paths: Vec::new(),
            require_lease: false,
            run_credentials: RunCredentials::Auto,
            max_output_bytes: 64 * 1024,
            timeout: Duration::from_secs(30),
            max_message_bytes: 8 * 1024 * 1024,
            max_concurrent: 8,
            max_lease_seconds: 3600,
            insecure_http: false,
            ca_file: None,
            log_level: "info".into(),
        }
    }

    /// True if the group is switched on.
    pub fn allows(&self, g: Group) -> bool {
        self.groups.contains(&g)
    }

    /// Checks the values that clap cannot: the URL, the globs, the bounds.
    pub fn validate(&self) -> Result<(), String> {
        let url = self.server.trim();
        let (scheme, rest) =
            url.split_once("://").ok_or_else(|| format!("--server must be an http(s) URL, got `{url}`"))?;
        if scheme != "http" && scheme != "https" {
            return Err(format!("--server must be an http(s) URL, got scheme `{scheme}`"));
        }
        let host_port = rest.split('/').next().unwrap_or("");
        if host_port.is_empty() || host_port.contains('@') {
            return Err("--server must not contain credentials (user:pass@host); use --token-file".into());
        }
        if scheme == "http" && !self.insecure_http && !is_loopback_host(host_port) {
            return Err(format!(
                "refusing to send the credential over plain http to `{host_port}`; use https, or pass \
                 --insecure-http if the network path is trusted"
            ));
        }
        for p in &self.write_paths {
            glob(p)?;
        }
        for r in &self.repos {
            check_repo_pattern(r)?;
        }
        if !(1024..=8 * 1024 * 1024).contains(&self.max_output_bytes) {
            return Err("--max-output-bytes must be between 1024 and 8388608".into());
        }
        if !(1024..=64 * 1024 * 1024).contains(&self.max_message_bytes) {
            return Err("--max-message-bytes must be between 1024 and 67108864".into());
        }
        if self.max_concurrent == 0 || self.max_concurrent > 64 {
            return Err("--max-concurrent must be between 1 and 64".into());
        }
        if self.groups.contains(&Group::Integrate) && !self.groups.contains(&Group::Read) {
            return Err("the integrate tool needs the read group (it reports outcomes by reading the proposal)".into());
        }
        Ok(())
    }

    /// The base URL without a trailing slash.
    pub fn base_url(&self) -> &str {
        self.server.trim().trim_end_matches('/')
    }

    /// True if `repo` (`owner/name`) may be touched by this process.
    pub fn repo_allowed(&self, repo: &str) -> bool {
        if self.repos.is_empty() {
            return true;
        }
        self.repos.iter().any(|p| match p.strip_suffix("/*") {
            Some(owner) => repo.split_once('/').is_some_and(|(o, _)| o == owner),
            None => p == repo,
        })
    }

    /// The single repository the operator pinned, if there is exactly one without wildcards.
    pub fn pinned_repo(&self) -> Option<&str> {
        match self.repos.as_slice() {
            [one] if !one.contains('*') => Some(one),
            _ => None,
        }
    }

    /// Compiled `--write-path` globs (`*` does not cross `/`, `**` does: the server's own matcher semantics).
    pub fn write_path_set(&self) -> Result<Option<GlobSet>, String> {
        if self.write_paths.is_empty() {
            return Ok(None);
        }
        let mut b = GlobSetBuilder::new();
        for p in &self.write_paths {
            b.add(glob(p)?);
        }
        b.build().map(Some).map_err(|e| e.to_string())
    }
}

/// Compiles a path glob with the semantics the server's policy matcher uses.
pub fn glob(p: &str) -> Result<globset::Glob, String> {
    GlobBuilder::new(p).literal_separator(true).build().map_err(|e| format!("bad glob `{p}`: {e}"))
}

fn is_loopback_host(host_port: &str) -> bool {
    let host = if let Some(rest) = host_port.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host_port
            .rsplit_once(':')
            .map_or(host_port, |(h, port)| if port.chars().all(|c| c.is_ascii_digit()) { h } else { host_port })
    };
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

fn check_repo_pattern(r: &str) -> Result<(), String> {
    let ok_seg = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    match r.split_once('/') {
        Some((o, "*")) if ok_seg(o) => Ok(()),
        Some((o, n)) if ok_seg(o) && ok_seg(n) => Ok(()),
        _ => Err(format!("--repo `{r}` must look like owner/name or owner/*")),
    }
}

/// Validates the shape of a repository name before it is placed into a URL path.
pub fn check_repo_name(r: &str) -> Result<(), String> {
    let ok_seg = |s: &str| {
        !s.is_empty() && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    };
    match r.split_once('/') {
        Some((o, n)) if ok_seg(o) && ok_seg(n) => Ok(()),
        _ => Err(format!("`repo` must look like owner/name, got `{}`", crate::sanitize::attr_value(r))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::new("http://127.0.0.1:8080", TokenSource::Fixed("agt_x".into()))
    }

    #[test]
    fn defaults_are_least_surprising() {
        let c = cfg();
        assert!(
            c.allows(Group::Read)
                && c.allows(Group::Workspace)
                && c.allows(Group::Propose)
                && c.allows(Group::Coordinate)
        );
        assert!(!c.allows(Group::Review) && !c.allows(Group::Integrate));
        c.validate().unwrap();
    }

    #[test]
    fn integrate_and_admin_cannot_come_in_through_allow() {
        assert!(Group::parse_allowed("integrate").unwrap_err().contains("--enable-integrate"));
        assert!(Group::parse_allowed("admin").unwrap_err().contains("no `admin` group"));
        assert!(Group::parse_allowed("nonsense").unwrap_err().contains("unknown group"));
        assert_eq!(Group::parse_allowed(" review ").unwrap(), Group::Review);
    }

    #[test]
    fn plain_http_is_only_for_loopback() {
        let mut c = cfg();
        c.server = "http://git.example.com".into();
        assert!(c.validate().unwrap_err().contains("plain http"));
        c.insecure_http = true;
        c.validate().unwrap();
        for ok in ["http://localhost:1/", "http://[::1]:9000", "http://127.0.0.1"] {
            c.insecure_http = false;
            c.server = ok.into();
            c.validate().unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        c.server = "https://user:pw@git.example.com".into();
        assert!(c.validate().unwrap_err().contains("must not contain credentials"));
        c.server = "ftp://x".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn repo_allow_list() {
        let mut c = cfg();
        assert!(c.repo_allowed("any/thing"));
        c.repos = vec!["acme/web".into(), "labs/*".into()];
        assert!(c.repo_allowed("acme/web") && c.repo_allowed("labs/x"));
        assert!(!c.repo_allowed("acme/api") && !c.repo_allowed("labs") && !c.repo_allowed("other/web"));
        assert_eq!(c.pinned_repo(), None);
        c.repos = vec!["acme/web".into()];
        assert_eq!(c.pinned_repo(), Some("acme/web"));
        c.repos = vec!["acme/we b".into()];
        assert!(c.validate().is_err());
    }

    #[test]
    fn write_globs_use_server_semantics() {
        let mut c = cfg();
        c.write_paths = vec!["src/**".into(), "docs/*.md".into()];
        let set = c.write_path_set().unwrap().unwrap();
        assert!(set.is_match("src/a/b.rs") && set.is_match("docs/x.md"));
        assert!(!set.is_match("docs/sub/x.md") && !set.is_match("Cargo.toml"));
        c.write_paths = vec!["[".into()];
        assert!(c.validate().is_err());
    }

    #[test]
    fn the_token_source_never_prints_the_secret() {
        let t = TokenSource::Fixed("agt_0123456789abcdef_secretsecretsecretsecret".into());
        assert!(!format!("{t:?}").contains("secret"));
        assert!(!format!("{:?}", Config::new("http://localhost", t)).contains("secret"));
    }

    #[test]
    fn token_loading_rejects_garbage() {
        assert!(TokenSource::Fixed("  ".into()).load().is_err());
        assert!(TokenSource::Fixed("a b".into()).load().is_err());
        assert_eq!(TokenSource::Fixed(" tok \n".into()).load().unwrap(), "tok");
        assert!(TokenSource::Env("AG_MCP_SURELY_UNSET_VARIABLE".into()).load().unwrap_err().contains("not set"));
    }

    #[test]
    fn repo_names_are_checked_before_they_reach_a_url() {
        assert!(check_repo_name("acme/web").is_ok());
        for bad in ["acme", "a/b/c", "../x", "acme/..", "a b/c", "a/b?x=1", ""] {
            assert!(check_repo_name(bad).is_err(), "{bad}");
        }
    }
}
