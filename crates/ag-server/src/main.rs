//! `ag-server` — the AgentGit network server.

use std::path::PathBuf;

use ag_protocol::v1;
use ag_server::app::App;
use ag_server::authz::Role;
use ag_server::config::ServerConfig;
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

/// The AgentGit server: AGWP over gRPC and HTTP/JSON on one port.
#[derive(Parser, Debug)]
#[command(name = "ag-server", version = env!("AG_LONG_VERSION"), about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the server.
    Serve {
        /// Configuration file (TOML). Without one, safe defaults on 127.0.0.1:8080 are used.
        #[arg(short, long, env = "AG_SERVER_CONFIG")]
        config: Option<PathBuf>,
        /// Override the listen address.
        #[arg(long)]
        listen: Option<std::net::SocketAddr>,
        /// Override the data directory.
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Probe a running server's /readyz and exit 0 (ready) or 1. For container health checks,
    /// where the image has no curl.
    Healthcheck {
        /// Base URL to probe.
        #[arg(long, env = "AG_HEALTHCHECK_URL", default_value = "http://127.0.0.1:8080")]
        url: String,
        /// Give up after this many seconds.
        #[arg(long, default_value_t = 3)]
        timeout: u64,
    },
    /// Print the default configuration (TOML).
    DefaultConfig,
    /// Print the OpenAPI document of the HTTP/JSON API.
    Openapi,
    /// Offline administration directly on the control database (works while the server is stopped).
    Admin {
        /// Data directory of the server (default: `server.data_dir` of the configuration, else `./ag-data`).
        #[arg(long, env = "AG_DATA_DIR")]
        data_dir: Option<PathBuf>,
        /// The server's configuration. `[control]` (a PostgreSQL control database), `[repos]` (storage and the
        /// default stream), `[limits]` (the quota a new repository starts with), `[auth]` (credential lifetimes), `[agents]` (when a run counts as stale)
        /// and `server.data_dir` are used: a repository created here gets the same PostgreSQL schema, S3 prefix,
        /// default stream and quota as one created through the API, and a minted credential obeys the same
        /// lifetime caps. Without it, the control database is `<data-dir>/control.db` and `create-repo` makes a
        /// local SQLite repository with built-in defaults. The container images set it for you.
        #[arg(short, long, env = "AG_SERVER_CONFIG")]
        config: Option<PathBuf>,
        #[command(subcommand)]
        action: AdminAction,
    },
}

/// The two places a control database can live.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum ControlBackend {
    /// `<data-dir>/control.db`
    Sqlite,
    /// The PostgreSQL schema named by `[control]` in the configuration.
    Postgres,
}

#[derive(Subcommand, Debug)]
enum AdminAction {
    /// Grant a role (reader, writer, reviewer, integrator, runner, delegator, admin).
    Grant {
        /// Identity or pattern, e.g. user:alice, agent:*, group:platform.
        #[arg(long)]
        subject: String,
        /// Repository pattern, e.g. acme/web, acme/*, *.
        #[arg(long, default_value = "*")]
        repo: String,
        /// The role.
        #[arg(long)]
        role: String,
    },
    /// Mint a credential and print its token (shown once).
    Mint {
        /// Identity the credential authenticates as.
        #[arg(long)]
        subject: String,
        /// Lifetime in seconds (default and maximum: `[auth]` of the configuration, per kind of identity).
        #[arg(long)]
        ttl: Option<u32>,
        /// Restrict to these repository patterns (repeatable).
        #[arg(long = "repo")]
        repos: Vec<String>,
        /// Restrict to these stream patterns (repeatable).
        #[arg(long = "stream")]
        streams: Vec<String>,
        /// Restrict to these operations (repeatable), e.g. objects.write.
        #[arg(long = "operation")]
        operations: Vec<String>,
        /// Description.
        #[arg(long, default_value = "created with ag-server admin")]
        description: String,
    },
    /// Create a repository.
    CreateRepo {
        /// owner/name
        name: String,
        /// First stream (default: `repos.default_stream` of the configuration).
        #[arg(long)]
        default_stream: Option<String>,
    },
    /// List repositories.
    Repos,
    /// Store a ready-made policy (solo, team, agents, strict) as the next policy version of a stream, and say in
    /// plain language what it allows and refuses. `ag policy preset list` shows what each is for.
    PolicyPreset {
        /// owner/name
        repo: String,
        /// solo, team, agents or strict.
        name: String,
        /// Stream (default: the repository's default stream).
        #[arg(long)]
        stream: Option<String>,
        /// Identity pattern allowed to change the policy afterwards (repeatable). Default: the identity in --as.
        #[arg(long = "admin")]
        admins: Vec<String>,
        /// Trusted CI issuer for the `tests` check: IDENTITY=PUBLIC_KEY (required by `strict`).
        #[arg(long)]
        issuer: Option<String>,
        /// Who is recorded as changing the policy. The engine only lets an administrator of the current policy do
        /// it: with none configured any `user:` identity is one.
        #[arg(long = "as", default_value = "user:cli-admin")]
        actor: String,
    },
    /// End the runs of a repository that stopped heartbeating, release the leases of ended runs and revoke the
    /// credentials bound to them (what `POST /v1/repos/{repo}/runs/reap` does; audited as `system:cli-admin`).
    ReapRuns {
        /// owner/name
        repo: String,
        /// A running run whose last heartbeat is older than this is abandoned (default: `agents.run_stale_after_secs`).
        #[arg(long)]
        stale_after_secs: Option<u64>,
        /// Say what would be reaped without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Write a logical backup of the control database (credential hashes, roles, repository registry) to a file.
    /// Works for SQLite and PostgreSQL alike. The file is as sensitive as the database: it is created with mode 0600.
    BackupControl {
        /// Where to write it (must not exist).
        #[arg(long)]
        out: PathBuf,
    },
    /// Restore a backup written by `backup-control` into the (empty) control database.
    RestoreControl {
        /// The backup file.
        #[arg(long)]
        from: PathBuf,
        /// Overwrite what the control database holds now (in one transaction).
        #[arg(long)]
        replace: bool,
    },
    /// Move the control database between SQLite (`<data-dir>/control.db`) and PostgreSQL (`[control]` in the
    /// configuration): `--from sqlite --to postgres` is the step from a single server to several replicas.
    MigrateControl {
        /// Where the data is now.
        #[arg(long, value_enum)]
        from: ControlBackend,
        /// Where it should go (must be empty unless `--replace`).
        #[arg(long, value_enum)]
        to: ControlBackend,
        /// Overwrite what the target holds now.
        #[arg(long)]
        replace: bool,
    },
}

/// Colour only for a person at a terminal: never in a file or pipe (a service's log would fill with escape
/// codes), never with `NO_COLOR`, and on Windows only if the console accepts virtual-terminal sequences.
fn use_ansi() -> bool {
    use std::io::IsTerminal;
    let wanted = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    #[cfg(windows)]
    let wanted = wanted && nu_ansi_term::enable_ansi_support().is_ok();
    wanted
}

fn init_tracing(cfg: &ServerConfig) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cfg.telemetry.log_filter));
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_target(false).with_ansi(use_ansi());
    if cfg.telemetry.log_format == "json" {
        builder.json().init();
    } else {
        builder.init();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Healthcheck { url, timeout } => {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_global(Some(std::time::Duration::from_secs(timeout.max(1))))
                .build()
                .into();
            let target = format!("{}/readyz", url.trim_end_matches('/'));
            let probe =
                tokio::task::spawn_blocking(move || agent.get(&target).call().map(|r| r.status().as_u16())).await?;
            match probe {
                Ok(200) => Ok(()),
                Ok(code) => bail!("not ready: HTTP {code}"),
                Err(e) => bail!("unreachable: {e}"),
            }
        }
        Command::DefaultConfig => {
            println!("{}", toml::to_string_pretty(&ServerConfig::default())?);
            Ok(())
        }
        Command::Openapi => {
            let mut cfg = ServerConfig::default();
            cfg.server.data_dir = std::env::temp_dir().join(format!("ag-openapi-{}", std::process::id()));
            let app = App::new(cfg.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
            let (_, routes) = ag_server::server::build_router(app);
            println!(
                "{}",
                serde_json::to_string_pretty(&ag_server::routes::openapi(&routes, env!("CARGO_PKG_VERSION")))?
            );
            let _ = std::fs::remove_dir_all(&cfg.server.data_dir);
            Ok(())
        }
        Command::Serve { config, listen, data_dir } => {
            let mut cfg = match config {
                Some(path) => ServerConfig::load(&path)?,
                None => ServerConfig::default(),
            };
            if let Some(l) = listen {
                cfg.server.listen = l;
            }
            if let Some(d) = data_dir {
                cfg.server.data_dir = d;
            }
            cfg.validate()?;
            init_tracing(&cfg);
            // Opening the control database may talk to PostgreSQL, which must not happen on an async thread.
            let app = {
                let cfg = cfg.clone();
                tokio::task::spawn_blocking(move || App::new(cfg)).await?.map_err(|e| anyhow::anyhow!("{e}"))?
            };
            if !app.auth.bootstrap_enabled() && !app.auth.oidc_enabled() {
                tracing::warn!(
                    "no bootstrap token ({}) and no OIDC provider are configured: nobody can authenticate until you \
                     create a credential with `ag-server admin mint`",
                    cfg.auth.bootstrap_token_env
                );
            }
            if cfg.metrics.enabled
                && !cfg.server.listen.ip().is_loopback()
                && std::env::var(&cfg.metrics.token_env).map(|t| t.is_empty()).unwrap_or(true)
            {
                tracing::warn!(
                    "/metrics is served on {} without a token ({} is not set): anyone who can reach the port can read it",
                    cfg.server.listen,
                    cfg.metrics.token_env
                );
            }
            // Registered before the server says it is listening: a stop request that arrives right after that line
            // (a supervisor, a test) must be a graceful stop, not the default action of the signal.
            let stop = shutdown_signal().context("installing the signal handlers")?;
            let listener = tokio::net::TcpListener::bind(cfg.server.listen)
                .await
                .with_context(|| format!("binding {}", cfg.server.listen))?;
            let shutdown = async {
                stop.await;
                tracing::info!("shutting down");
            };
            ag_server::server::serve(app, listener, shutdown).await
        }
        Command::Admin { data_dir, config, action } => {
            let mut cfg = ServerConfig::default();
            cfg.server.data_dir = PathBuf::from("./ag-data");
            if let Some(path) = &config {
                let loaded = ServerConfig::load(path)?;
                cfg.repos = loaded.repos;
                cfg.control = loaded.control;
                cfg.limits = loaded.limits;
                cfg.auth = loaded.auth;
                cfg.agents = loaded.agents;
                cfg.server.data_dir = loaded.server.data_dir;
            }
            if let Some(dir) = data_dir {
                cfg.server.data_dir = dir;
            }
            // The administration commands print their own diagnostics; keep the library's quiet unless asked.
            let filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(false)
                .with_writer(std::io::stderr)
                .try_init();
            // Everything below talks to the control database (possibly PostgreSQL): not on an async thread.
            tokio::task::spawn_blocking(move || match action {
                AdminAction::BackupControl { out } => backup_control(&cfg, &out),
                AdminAction::RestoreControl { from, replace } => restore_control(&cfg, &from, replace),
                AdminAction::MigrateControl { from, to, replace } => migrate_control(&cfg, from, to, replace),
                other => {
                    let app = App::new(cfg).map_err(|e| anyhow::anyhow!("{e}"))?;
                    admin(app, other)
                }
            })
            .await?
        }
    }
}

/// Starts listening for a request to stop and returns the future that resolves when one arrives, so the server
/// drains in-flight requests and flushes telemetry. The handlers are installed by this call, not by the first poll
/// of the future: a signal that comes in between must not take the default action (ending the process).
///
/// Unix: SIGINT (Ctrl+C) and SIGTERM. Windows, where a console process gets control events instead of
/// signals: Ctrl+C, Ctrl+Break, closing the console window, and logoff/shutdown of the machine.
fn shutdown_signal() -> std::io::Result<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        Ok(Box::pin(async move {
            tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {} }
        }))
    }
    #[cfg(not(unix))]
    {
        Ok(Box::pin(wait_for_shutdown_signal()))
    }
}

/// The console control events of Windows (and, elsewhere without signals, nothing but Ctrl+C).
#[cfg(not(unix))]
async fn wait_for_shutdown_signal() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.ok() };
    #[cfg(windows)]
    let term = async {
        use tokio::signal::windows;
        // A stream that cannot be registered never fires; the others still do.
        async fn wait<F: std::future::Future>(s: std::io::Result<F>) {
            match s {
                Ok(f) => {
                    f.await;
                }
                Err(_) => std::future::pending::<()>().await,
            }
        }
        tokio::select! {
            _ = async { wait(windows::ctrl_break().map(|mut s| async move { s.recv().await })).await } => {}
            _ = async { wait(windows::ctrl_close().map(|mut s| async move { s.recv().await })).await } => {}
            _ = async { wait(windows::ctrl_shutdown().map(|mut s| async move { s.recv().await })).await } => {}
            _ = async { wait(windows::ctrl_logoff().map(|mut s| async move { s.recv().await })).await } => {}
        }
    };
    #[cfg(not(windows))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}

fn open_control_for_admin(cfg: &ServerConfig) -> Result<ag_server::control::Control> {
    std::fs::create_dir_all(&cfg.server.data_dir).context("creating the data directory")?;
    ag_server::control::Control::from_config(cfg, std::sync::Arc::new(ag_storage::SystemClock))
        .map_err(|e| anyhow::anyhow!("{e}"))
}

fn backup_control(cfg: &ServerConfig, out: &std::path::Path) -> Result<()> {
    let control = open_control_for_admin(cfg)?;
    let dump = control.export().map_err(|e| anyhow::anyhow!("{e}"))?;
    dump.write_file(out).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "wrote {} ({} credential(s), {} role binding(s), {} repositor(ies); control schema {}, from {})",
        out.display(),
        dump.credentials.len(),
        dump.roles.len(),
        dump.repos.len(),
        dump.schema_version,
        dump.source
    );
    eprintln!("the file holds credential secret hashes: store it like the database itself");
    Ok(())
}

fn restore_control(cfg: &ServerConfig, from: &std::path::Path, replace: bool) -> Result<()> {
    use ag_server::control::{ControlDump, ImportMode};
    let dump = ControlDump::read_file(from).map_err(|e| anyhow::anyhow!("{e}"))?;
    let control = open_control_for_admin(cfg)?;
    let mode = if replace { ImportMode::Replace } else { ImportMode::RequireEmpty };
    let s = control.import(&dump, mode).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "restored {} credential(s), {} role binding(s), {} repositor(ies), {} idempotency record(s), {} setting(s)",
        s.credentials, s.roles, s.repos, s.idempotency, s.kv
    );
    Ok(())
}

fn migrate_control(cfg: &ServerConfig, from: ControlBackend, to: ControlBackend, replace: bool) -> Result<()> {
    use ag_server::control::{Control, ImportMode};
    if from == to {
        bail!("--from and --to name the same backend");
    }
    let clock: std::sync::Arc<dyn ag_storage::Clock> = std::sync::Arc::new(ag_storage::SystemClock);
    let sqlite_path = cfg.server.data_dir.join("control.db");
    let open = |which: ControlBackend, must_exist: bool| -> Result<Control> {
        match which {
            ControlBackend::Sqlite => {
                if must_exist && !sqlite_path.is_file() {
                    bail!("there is no SQLite control database at {}", sqlite_path.display());
                }
                std::fs::create_dir_all(&cfg.server.data_dir).context("creating the data directory")?;
                Control::open(&sqlite_path, clock.clone()).map_err(|e| anyhow::anyhow!("{e}"))
            }
            ControlBackend::Postgres => {
                if !cfg.control.is_postgres() {
                    bail!("the PostgreSQL side needs [control] url or url_env in the configuration (--config)");
                }
                Control::from_config(cfg, clock.clone()).map_err(|e| anyhow::anyhow!("{e}"))
            }
        }
    };
    let source = open(from, true)?;
    let mut dump = source.export().map_err(|e| anyhow::anyhow!("{e}"))?;
    let filled = if from == ControlBackend::Sqlite { dump.fill_repo_configs(&cfg.server.data_dir) } else { 0 };
    let target = open(to, false)?;
    let mode = if replace { ImportMode::Replace } else { ImportMode::RequireEmpty };
    let s = target.import(&dump, mode).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "moved {} credential(s), {} role binding(s), {} repositor(ies), {} idempotency record(s), {} setting(s) from {from:?} to {to:?}",
        s.credentials, s.roles, s.repos, s.idempotency, s.kv
    );
    let local_only: Vec<&str> = dump
        .repos
        .iter()
        .filter(|r| !ag_server::repos::config_is_shared(&r.config_json))
        .map(|r| r.name.as_str())
        .collect();
    if filled > 0 {
        eprintln!("took the stored configuration of {filled} repositor(ies) from {}", cfg.server.data_dir.display());
    }
    if !local_only.is_empty() {
        eprintln!(
            "note: {} repositor(ies) keep their data on this host's disk (not PostgreSQL + S3) and can only be served by a server \
             that has {}: {}",
            local_only.len(),
            cfg.server.data_dir.display(),
            local_only.join(", ")
        );
    }
    Ok(())
}

fn admin(app: App, action: AdminAction) -> Result<()> {
    let by = "system:cli-admin";
    match action {
        AdminAction::Grant { subject, repo, role } => {
            let Some(r) = Role::parse(&role) else { bail!("unknown role '{role}'") };
            app.control.grant_role(&subject, &repo, r, by).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("granted {role} to {subject} on {repo}");
        }
        AdminAction::Mint { subject, ttl, repos, streams, operations, description } => {
            let identity = ag_types::Identity::new(subject.clone())?;
            if identity.kind() == ag_types::IdentityKind::System {
                bail!("credentials cannot be minted for system identities");
            }
            // The same rule as `POST /v1/credentials`: the configured default, and never past the cap.
            let ttl = app
                .cfg
                .auth
                .credential_ttl(identity.kind(), ttl.unwrap_or(0))
                .map_err(|e| anyhow::anyhow!("--ttl: {e}"))?;
            let minted = ag_server::auth::mint_token().map_err(|e| anyhow::anyhow!("{e}"))?;
            let scope = v1::CredentialScope { repos, streams, operations, run_id: String::new() };
            ag_server::authz::Scope::from_wire(&scope).map_err(|e| anyhow::anyhow!("{e}"))?;
            let now = app.clock.now().as_millis();
            app.control
                .insert_credential(&ag_server::control::CredentialRow {
                    id: minted.credential_id.clone(),
                    secret_hash: minted.secret_hash,
                    subject,
                    kind: identity.kind().prefix().into(),
                    description,
                    scope,
                    created_at_ms: now,
                    expires_at_ms: now + i64::from(ttl) * 1000,
                    revoked_at_ms: None,
                    last_used_ms: 0,
                    created_by: by.into(),
                })
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            eprintln!("credential {} (expires in {ttl}s); the token is shown once:", minted.credential_id);
            println!("{}", minted.token);
        }
        AdminAction::CreateRepo { name, default_stream } => {
            let actor = ag_types::Identity::new(by)?;
            let default_stream = default_stream.unwrap_or_else(|| app.cfg.repos.default_stream.clone());
            let row = app
                .repos
                .create(&name, &default_stream, "", &actor, app.default_quota())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("created {} (namespace {})", row.name, row.namespace);
        }
        AdminAction::Repos => {
            for r in app.control.all_repos().map_err(|e| anyhow::anyhow!("{e}"))? {
                println!("{}\t{}", r.name, r.description);
            }
        }
        AdminAction::PolicyPreset { repo, name, stream, admins, issuer, actor } => {
            use ag_policy::presets::{Preset, PresetOptions, parse_issuer};
            let actor = ag_types::Identity::new(actor)?;
            let preset = Preset::parse(&name)?;
            let ci = issuer.as_deref().map(parse_issuer).transpose().map_err(|e| anyhow::anyhow!("--issuer: {e}"))?;
            let admins = if admins.is_empty() { vec![actor.principal().to_string()] } else { admins };
            let policy = preset.build(&PresetOptions { admins, ci }).map_err(|e| match e {
                ag_policy::presets::PresetError::NeedsIssuer(n) => anyhow::anyhow!(
                    "the preset '{n}' requires a passing check on every change: pass --issuer IDENTITY=PUBLIC_KEY"
                ),
                other => anyhow::anyhow!(other),
            })?;
            if app.control.repo(&repo).map_err(|e| anyhow::anyhow!("{e}"))?.is_none() {
                bail!("repository '{repo}' not found");
            }
            let handle = app.repos.handle(&repo).map_err(|e| anyhow::anyhow!("{e}"))?;
            let stream = stream.unwrap_or_else(|| handle.default_stream().to_string());
            let row = handle.set_policy(&stream, &policy.to_json(), &actor)?;
            println!(
                "policy of {repo} '{stream}' is version {} (the preset '{name}', hash {})",
                row.version,
                &row.doc_hash[..12]
            );
            println!("{}", ag_policy::explain::render(&ag_policy::explain::explain(&policy)));
        }
        AdminAction::ReapRuns { repo, stale_after_secs, dry_run } => {
            let cfg = &app.cfg.agents;
            let secs = stale_after_secs.unwrap_or(cfg.run_stale_after_secs);
            if secs < cfg.min_stale_after_secs {
                bail!("--stale-after-secs must be at least {} (agents.min_stale_after_secs)", cfg.min_stale_after_secs);
            }
            let stale_after_ms = i64::try_from(secs.saturating_mul(1000)).context("--stale-after-secs is too large")?;
            if app.control.repo(&repo).map_err(|e| anyhow::anyhow!("{e}"))?.is_none() {
                bail!("repository '{repo}' not found");
            }
            let handle = app.repos.handle(&repo).map_err(|e| anyhow::anyhow!("{e}"))?;
            let reaped = ag_agent::registry::reap_runs(handle.meta(), stale_after_ms, by, dry_run)?;
            let mut revoked = 0;
            for run in &reaped.runs {
                println!("{} {}\t{}", if dry_run { "would abandon" } else { "abandoned" }, run.run_id, run.agent);
                if !dry_run {
                    revoked += app
                        .control
                        .revoke_run_credentials(&repo, &run.run_id, app.clock.now().as_millis())
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                }
            }
            for lease in &reaped.released_leases {
                println!("{} lease {lease}", if dry_run { "would release" } else { "released" });
            }
            println!(
                "{} run(s), {} lease(s){}",
                reaped.runs.len(),
                reaped.released_leases.len(),
                if dry_run {
                    " (dry run: nothing changed)".to_string()
                } else {
                    format!(", {revoked} credential(s) revoked")
                }
            );
        }
        AdminAction::BackupControl { .. } | AdminAction::RestoreControl { .. } | AdminAction::MigrateControl { .. } => {
            bail!("handled before an application is built")
        }
    }
    Ok(())
}
