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
    /// Print the default configuration (TOML).
    DefaultConfig,
    /// Print the OpenAPI document of the HTTP/JSON API.
    Openapi,
    /// Offline administration directly on a data directory (works while the server is stopped).
    Admin {
        /// Data directory of the server.
        #[arg(long, env = "AG_DATA_DIR", default_value = "./ag-data")]
        data_dir: PathBuf,
        #[command(subcommand)]
        action: AdminAction,
    },
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
            let app = App::new(cfg.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
            if !app.auth.bootstrap_enabled() && !app.auth.oidc_enabled() {
                tracing::warn!(
                    "no bootstrap token ({}) and no OIDC provider are configured: nobody can authenticate until you \
                     create a credential with `ag-server admin mint`",
                    cfg.auth.bootstrap_token_env
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
        Command::Admin { data_dir, action } => {
            let mut cfg = ServerConfig::default();
            cfg.server.data_dir = data_dir;
            let app = App::new(cfg).map_err(|e| anyhow::anyhow!("{e}"))?;
            admin(app, action).await
        }
    }
}

async fn admin(app: App, action: AdminAction) -> Result<()> {
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
    }
    Ok(())
}
