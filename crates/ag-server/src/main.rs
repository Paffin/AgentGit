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
#[command(name = "ag-server", version, about)]
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
        /// Server configuration, to find a PostgreSQL control database (`[control]`). Without one, the control
        /// database is `<data-dir>/control.db`.
        #[arg(short, long, env = "AG_SERVER_CONFIG")]
        config: Option<PathBuf>,
        /// Data directory of the server (default: the configuration's, else ./ag-data).
        #[arg(long, env = "AG_DATA_DIR")]
        data_dir: Option<PathBuf>,
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
    /// Grant a role (reader, writer, reviewer, integrator, runner, admin).
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
        /// Lifetime in seconds.
        #[arg(long, default_value_t = 3600)]
        ttl: u32,
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
        /// First stream.
        #[arg(long, default_value = "main")]
        default_stream: String,
    },
    /// List repositories.
    Repos,
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

fn init_tracing(cfg: &ServerConfig) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cfg.telemetry.log_filter));
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_target(false);
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
            let listener = tokio::net::TcpListener::bind(cfg.server.listen)
                .await
                .with_context(|| format!("binding {}", cfg.server.listen))?;
            let shutdown = async {
                let ctrl_c = async { tokio::signal::ctrl_c().await.ok() };
                #[cfg(unix)]
                let term = async {
                    if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                        s.recv().await;
                    }
                };
                #[cfg(not(unix))]
                let term = std::future::pending::<()>();
                tokio::select! { _ = ctrl_c => {}, _ = term => {} }
                tracing::info!("shutting down");
            };
            ag_server::server::serve(app, listener, shutdown).await
        }
        Command::Admin { config, data_dir, action } => {
            let mut cfg = match config {
                Some(path) => ServerConfig::load(&path)?,
                None => ServerConfig::default(),
            };
            if let Some(d) = data_dir {
                cfg.server.data_dir = d;
            }
            // The administration commands print their own diagnostics; keep the library's quiet unless asked.
            let filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
            let _ = tracing_subscriber::fmt().with_env_filter(filter).with_target(false).with_writer(std::io::stderr).try_init();
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
            let minted = ag_server::auth::mint_token().map_err(|e| anyhow::anyhow!("{e}"))?;
            let scope = v1::CredentialScope { repos, streams, operations };
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
            let row = app
                .repos
                .create(&name, &default_stream, "", &actor, v1::Quota::default())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("created {} (namespace {})", row.name, row.namespace);
        }
        AdminAction::Repos => {
            for r in app.control.repos("", "", 10_000).map_err(|e| anyhow::anyhow!("{e}"))? {
                println!("{}\t{}", r.name, r.description);
            }
        }
        AdminAction::BackupControl { .. } | AdminAction::RestoreControl { .. } | AdminAction::MigrateControl { .. } => {
            bail!("handled before an application is built")
        }
    }
    Ok(())
}
