//! `ag-mcp`: serve the remote AgentGit workflow to an MCP client over stdio.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use ag_mcp::config::{Config, DEFAULT_GROUPS, Group, RunCredentials, TokenSource};
use ag_mcp::protocol::supported_strings;
use ag_mcp::{Registry, Server, log};
use clap::Parser;
use serde_json::{Value, json};

/// A Model Context Protocol server for AgentGit (stdio).
///
/// The credential is never taken from the command line: use --token-file (re-read after a 401, so it can be
/// rotated) or an environment variable. Its scope on the AgentGit server is the security boundary; the options
/// below can only narrow what the agent may do with it.
#[derive(Parser, Debug)]
#[command(name = "ag-mcp", version, about, long_about)]
struct Cli {
    /// Base URL of ag-server, e.g. https://git.example.com.
    #[arg(long, env = "AG_MCP_SERVER")]
    server: String,

    /// File holding the credential (agt_…). Re-read after a 401.
    #[arg(long, env = "AG_MCP_TOKEN_FILE", conflicts_with = "token_env")]
    token_file: Option<PathBuf>,

    /// Environment variable holding the credential.
    #[arg(long, env = "AG_MCP_TOKEN_ENV", default_value = "AG_MCP_TOKEN")]
    token_env: String,

    /// Repositories this process may touch: owner/name or owner/* (repeatable). With exactly one, tools default to it.
    #[arg(long = "repo", env = "AG_MCP_REPO", value_delimiter = ',')]
    repos: Vec<String>,

    /// Tool groups to switch on, comma-separated: read, workspace, propose, coordinate, discuss, review, issue.
    /// Default: read,workspace,propose,coordinate,discuss.
    #[arg(long, env = "AG_MCP_ALLOW", value_delimiter = ',')]
    allow: Vec<String>,

    /// Also switch on the `integrate` tool, which can land a proposal on a stream if policy allows. Off by default.
    #[arg(long)]
    enable_integrate: bool,

    /// Writes must match one of these globs (repeatable); `*` stays inside a directory, `**` crosses.
    #[arg(long = "write-path", env = "AG_MCP_WRITE_PATHS", value_delimiter = ',')]
    write_paths: Vec<String>,

    /// Refuse writes to paths not covered by an active lease that the agent holds.
    #[arg(long, env = "AG_MCP_REQUIRE_LEASE")]
    require_lease: bool,

    /// Whether each run gets a credential of its own, bound to that run: auto (when the server supports it and the
    /// credential is an agent's), required (refuse to start a run otherwise) or off.
    #[arg(long, env = "AG_MCP_RUN_CREDENTIALS", default_value = "auto")]
    run_credentials: String,

    /// Most repository-derived text in one response, in bytes.
    #[arg(long, env = "AG_MCP_MAX_OUTPUT_BYTES", default_value_t = 64 * 1024)]
    max_output_bytes: usize,

    /// Timeout of each request to the AgentGit server, in seconds.
    #[arg(long, default_value_t = 30)]
    timeout_secs: u64,

    /// Longest lease a tool may ask for, in seconds.
    #[arg(long, default_value_t = 3600)]
    max_lease_seconds: u64,

    /// Tool calls allowed to run at the same time.
    #[arg(long, default_value_t = 8)]
    max_concurrent: usize,

    /// Longest JSON-RPC message accepted from the client, in bytes. Longer lines are discarded with an error.
    #[arg(long, default_value_t = 8 * 1024 * 1024)]
    max_message_bytes: usize,

    /// Allow http:// to a host that is not loopback (the credential travels in clear text).
    #[arg(long)]
    insecure_http: bool,

    /// PEM file of certificate authorities to trust instead of the built-in roots (private CA).
    #[arg(long, env = "AG_MCP_CA_FILE")]
    ca_file: Option<PathBuf>,

    /// error, warn, info or debug. Logs go to stderr.
    #[arg(long, env = "AG_MCP_LOG", default_value = "info")]
    log_level: String,

    /// Check the credential against the server, print what the agent would be allowed to do, and exit.
    #[arg(long)]
    check: bool,

    /// Print the tools that would be offered (as JSON) and exit. Needs no server.
    #[arg(long)]
    list_tools: bool,
}

fn build_config(cli: Cli) -> Result<Config, String> {
    let token = match &cli.token_file {
        Some(p) => TokenSource::File(p.clone()),
        None => TokenSource::Env(cli.token_env.clone()),
    };
    let mut cfg = Config::new(cli.server, token);
    if !cli.allow.is_empty() {
        cfg.groups = cli.allow.iter().map(|g| Group::parse_allowed(g)).collect::<Result<_, _>>()?;
    } else {
        cfg.groups = DEFAULT_GROUPS.into_iter().collect();
    }
    if cli.enable_integrate {
        cfg.groups.insert(Group::Integrate);
    }
    cfg.repos = cli.repos.into_iter().filter(|r| !r.is_empty()).collect();
    cfg.write_paths = cli.write_paths.into_iter().filter(|r| !r.is_empty()).collect();
    cfg.require_lease = cli.require_lease;
    cfg.run_credentials = RunCredentials::parse(&cli.run_credentials)?;
    cfg.max_output_bytes = cli.max_output_bytes;
    cfg.timeout = Duration::from_secs(cli.timeout_secs.max(1));
    cfg.max_lease_seconds = cli.max_lease_seconds;
    cfg.max_concurrent = cli.max_concurrent;
    cfg.max_message_bytes = cli.max_message_bytes;
    cfg.insecure_http = cli.insecure_http;
    cfg.ca_file = cli.ca_file;
    cfg.log_level = cli.log_level;
    cfg.validate()?;
    Ok(cfg)
}

fn banner(cfg: &Config) {
    let groups: Vec<&str> = Group::ALL.iter().filter(|g| cfg.allows(**g)).map(|g| g.as_str()).collect();
    log::info(&format!(
        "ag-mcp {} serving MCP over stdio (protocol {}); AgentGit at {}; groups: {}",
        env!("CARGO_PKG_VERSION"),
        supported_strings().join(", "),
        cfg.base_url(),
        groups.join(",")
    ));
    if cfg.repos.is_empty() {
        log::warn("no --repo given: the agent may use every repository the credential can reach");
    } else {
        log::info(&format!("repositories: {}", cfg.repos.join(", ")));
    }
    if !cfg.write_paths.is_empty() {
        log::info(&format!("writes limited to: {}", cfg.write_paths.join(", ")));
    }
    if cfg.require_lease {
        log::info("writes require an active lease held by the agent");
    }
    if cfg.allows(Group::Integrate) {
        log::warn(
            "THE INTEGRATE TOOL IS ENABLED: this agent can land proposals on streams whenever policy allows it. \
                   Enable it only for a credential that a human would also trust to integrate.",
        );
    }
    if cfg.insecure_http {
        log::warn("--insecure-http: the credential may travel over plain http");
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let (check, list_tools) = (cli.check, cli.list_tools);
    log::set_level(&cli.log_level)?;
    let cfg = build_config(cli)?;
    if list_tools {
        let reg = Registry::standard();
        let tools: Vec<Value> = reg.enabled(&cfg).map(|t| t.to_json(true)).collect();
        println!("{}", serde_json::to_string_pretty(&json!({"tools": tools})).unwrap_or_default());
        return Ok(());
    }
    let server = Server::new(cfg, Registry::standard(), Box::new(std::io::stdout()))?;
    if check {
        return check_credential(&server);
    }
    banner(server.config());
    let stdin = std::io::stdin();
    server.serve(stdin.lock()).map_err(|e| format!("stdin: {e}"))
}

fn check_credential(server: &Server) -> Result<(), String> {
    let cfg = server.config();
    let opts = ag_mcp::api::Opts::default();
    let caps = server
        .api()
        .get("/v1/capabilities", &[], &opts)
        .map_err(|e| format!("cannot reach the server: {}", e.message))?;
    let who = server
        .api()
        .get("/v1/whoami", &[], &opts)
        .map_err(|e| format!("the server rejected the credential: {} ({})", e.message, e.code))?;
    let ident = who.get("identity").and_then(Value::as_str).unwrap_or("?");
    let exp = who.get("expires_at_ms").and_then(Value::as_i64).unwrap_or(0);
    println!(
        "server    {} (AGWP {})",
        cfg.base_url(),
        caps.get("protocol_version").and_then(Value::as_str).unwrap_or("?")
    );
    println!("identity  {ident}");
    println!("roles     {}", who.get("roles").map(|r| r.to_string()).unwrap_or_default());
    println!("scope     {}", who.get("scope").map(|r| r.to_string()).unwrap_or_default());
    match ag_mcp::tools::seconds_until(exp) {
        Some(s) if s <= 0 => return Err("the credential has expired".into()),
        Some(s) => println!("expires   in {} min", s / 60),
        None => println!("expires   never"),
    }
    let groups: Vec<&str> = Group::ALL.iter().filter(|g| cfg.allows(**g)).map(|g| g.as_str()).collect();
    println!("groups    {}", groups.join(","));
    println!("tools     {}", server.registry().enabled(cfg).count());
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ag-mcp: error: {e}");
            ExitCode::FAILURE
        }
    }
}
