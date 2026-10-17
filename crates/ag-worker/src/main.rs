//! `ag-worker` — claims validation jobs from an `ag-server`, runs them in a sandbox, signs attestations.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ag_client::{Client, Options as ClientOptions};
use ag_crypto::KeyPair;
use ag_sandbox::{Availability, IsolationLevel, Mount};
use ag_worker::{Backend, Ceilings, DEFAULT_SANDBOX_ID, Defaults, Options, Profile, Worker};
use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};

/// The AgentGit execution-plane worker.
#[derive(Parser, Debug)]
#[command(name = "ag-worker", version = env!("AG_LONG_VERSION"), about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Claim and run jobs until stopped (SIGINT/SIGTERM finishes the current job first).
    Run(Box<RunArgs>),
    /// Create the signing key and print what to put in the policy's `trusted_issuers`.
    Keygen {
        /// Where to write the key (mode 0600; an existing file is never overwritten).
        #[arg(long)]
        out: PathBuf,
        /// The identity the worker's credential authenticates as, for the printed policy snippet.
        #[arg(long, default_value = "service:worker")]
        identity: String,
    },
    /// Report which sandboxes work on this machine and why not, then exit.
    Doctor,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum SandboxChoice {
    /// The namespace sandbox if this machine can run it; otherwise an error (never a silent downgrade).
    Auto,
    /// Rootless namespaces (Linux).
    Namespace,
    /// A container runtime (`--oci-runtime`).
    Oci,
    /// Limits only. Not isolation; needs --insecure-allow-process-sandbox.
    Process,
}

#[derive(Args, Debug)]
struct RunArgs {
    /// Server URL, e.g. https://ag.example.com (gRPC over HTTP/2).
    #[arg(long, env = "AG_SERVER")]
    server: String,
    /// Environment variable holding the credential (a token with the `runner` role).
    #[arg(long, default_value = "AG_TOKEN")]
    token_env: String,
    /// Read the credential from this file instead.
    #[arg(long)]
    token_file: Option<PathBuf>,
    /// Extra PEM CA certificate to trust.
    #[arg(long)]
    ca_file: Option<PathBuf>,
    /// The signing key (see `keygen`). Keep it out of reach of anything that runs checks.
    #[arg(long, env = "AG_WORKER_KEY")]
    key: PathBuf,
    /// The identity of the credential. Default: asked from the server (`Whoami`).
    #[arg(long)]
    issuer: Option<String>,
    /// Only claim jobs of this repository.
    #[arg(long)]
    repo: Option<String>,
    /// This worker's name ([A-Za-z0-9._-], at most 64). Default: the host name and process id.
    #[arg(long)]
    label: Option<String>,
    /// Number of jobs run at once.
    #[arg(long, default_value_t = 1)]
    concurrency: usize,
    /// Process one job per lane, then exit.
    #[arg(long)]
    once: bool,
    /// Seconds a claim lasts without a heartbeat.
    #[arg(long, default_value_t = 120)]
    lease_secs: u64,
    /// Seconds one claim request waits for work (at most 30).
    #[arg(long, default_value_t = 20)]
    wait_secs: u64,

    /// Which sandbox runs the checks.
    #[arg(long, value_enum, default_value_t = SandboxChoice::Auto)]
    sandbox: SandboxChoice,
    /// Allow --sandbox process. **That is not isolation**: the check runs with this user's file and network
    /// access. Only for code you already trust.
    #[arg(long)]
    insecure_allow_process_sandbox: bool,
    /// Container runtime for --sandbox oci.
    #[arg(long, default_value = "docker")]
    oci_runtime: String,
    /// Image used when the policy names none (pinned by digest).
    #[arg(long)]
    oci_image: Option<String>,
    /// Container user, `uid:gid`.
    #[arg(long, default_value = "65534:65534")]
    oci_user: String,
    /// When this worker runs as root, the unprivileged host user the check runs as.
    #[arg(long, default_value_t = DEFAULT_SANDBOX_ID)]
    sandbox_uid: u32,
    /// ... and its group.
    #[arg(long, default_value_t = DEFAULT_SANDBOX_ID)]
    sandbox_gid: u32,
    /// Allow policies to ask for network access (the host's network, unrestricted).
    #[arg(long)]
    allow_network: bool,
    /// Mount nothing but --mount entries (the default mounts /usr, /bin, /lib*, ... read-only).
    #[arg(long)]
    no_system_mounts: bool,
    /// A read-only mount: HOST[:SANDBOX][=IDENTITY]. IDENTITY names *what* is mounted (a toolchain version,
    /// an image id) and goes into the environment digest.
    #[arg(long = "mount", value_name = "HOST[:SANDBOX][=IDENTITY]")]
    mounts: Vec<String>,
    /// Directory for job working trees (default: the system temp dir, below `ag-worker`).
    #[arg(long)]
    scratch_dir: Option<PathBuf>,

    /// The most wall-clock seconds any check may be given.
    #[arg(long)]
    max_timeout_secs: Option<u32>,
    /// The most memory (MiB) any check may be given.
    #[arg(long)]
    max_memory_mb: Option<u32>,
    /// The most CPU seconds (per process) any check may be given.
    #[arg(long)]
    max_cpu_secs: Option<u32>,
    /// The most processes and threads any check may be given.
    #[arg(long)]
    max_pids: Option<u32>,
    /// Memory (MiB) for checks whose policy leaves it at 0.
    #[arg(long)]
    default_memory_mb: Option<u32>,
    /// Processes for checks whose policy leaves it at 0.
    #[arg(long)]
    default_pids: Option<u32>,
    /// Largest accepted checkout, in MiB.
    #[arg(long, default_value_t = 4096)]
    max_checkout_mb: u64,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Keygen { out, identity } => keygen(&out, &identity),
        Command::Doctor => doctor(),
        Command::Run(args) => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("cannot start the async runtime")
            .and_then(|rt| rt.block_on(run(*args))),
    };
    if let Err(e) = result {
        eprintln!("ag-worker: {e:#}");
        std::process::exit(1);
    }
}

fn keygen(out: &std::path::Path, identity: &str) -> Result<()> {
    let key = KeyPair::generate()?;
    key.save(out).with_context(|| format!("cannot write {}", out.display()))?;
    println!("Wrote the worker key to {} (mode 0600).", out.display());
    println!("Add this to the policy's trusted_issuers (and give '{identity}' the `runner` role):");
    println!("  {{ \"identity\": \"{identity}\", \"public_key\": \"{}\" }}", key.public().to_hex());
    Ok(())
}

fn doctor() -> Result<()> {
    let (euid, egid) = ag_sandbox::effective_ids();
    println!(
        "running as uid {euid}, gid {egid}{}",
        if euid == 0 { " (root: checks will be mapped to an unprivileged user)" } else { "" }
    );
    for (name, availability) in ag_sandbox::detect() {
        match availability {
            Availability::Available => println!("  {name:<10} available"),
            Availability::Unavailable(why) => println!("  {name:<10} unavailable: {why}"),
        }
    }
    let best = ag_sandbox::best_available();
    println!(
        "`--sandbox auto` would choose: {} ({:?})",
        if best.isolation() >= IsolationLevel::Namespaces {
            best.name()
        } else {
            "nothing: no isolating sandbox is available"
        },
        best.isolation()
    );
    Ok(())
}

fn parse_mount(s: &str) -> Result<Mount> {
    let (paths, identity) = match s.split_once('=') {
        Some((p, i)) => (p, Some(i)),
        None => (s, None),
    };
    let (host, sandbox) = paths.split_once(':').unwrap_or((paths, paths));
    if !host.starts_with('/') || !sandbox.starts_with('/') {
        bail!("mount '{s}': both paths must be absolute");
    }
    let m = Mount::new(host, sandbox);
    Ok(match identity {
        Some(i) if !i.is_empty() => m.with_identity(i),
        _ => m,
    })
}

fn default_label() -> String {
    let host = std::fs::read_to_string("/etc/hostname").unwrap_or_default();
    let host: String =
        host.trim().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '.').take(40).collect();
    format!("{}-{}", if host.is_empty() { "worker" } else { &host }, std::process::id())
}

fn read_token(args: &RunArgs) -> Result<String> {
    let raw = match &args.token_file {
        Some(p) => std::fs::read_to_string(p).with_context(|| format!("cannot read the token file {}", p.display()))?,
        None => std::env::var(&args.token_env)
            .with_context(|| format!("no credential: set {} (or use --token-file)", args.token_env))?,
    };
    let t = raw.trim().to_string();
    if t.is_empty() {
        bail!("the credential is empty");
    }
    Ok(t)
}

fn backend(args: &RunArgs) -> Result<Backend> {
    let namespace = Backend::Namespace { host_ids: (args.sandbox_uid, args.sandbox_gid) };
    Ok(match args.sandbox {
        SandboxChoice::Auto => match ag_sandbox::NamespaceSandbox::new().availability() {
            Availability::Available => namespace,
            Availability::Unavailable(why) => bail!(
                "no isolating sandbox is available here ({why}). Fix that (see `ag-worker doctor`), use --sandbox oci, or, for code you already trust, --sandbox process --insecure-allow-process-sandbox"
            ),
        },
        SandboxChoice::Namespace => {
            if let Availability::Unavailable(why) = ag_sandbox::NamespaceSandbox::new().availability() {
                bail!("the namespace sandbox is unavailable: {why}");
            }
            namespace
        }
        SandboxChoice::Oci => Backend::Oci {
            runtime: args.oci_runtime.clone(),
            default_image: args.oci_image.clone(),
            user: args.oci_user.clone(),
        },
        SandboxChoice::Process => {
            if !args.insecure_allow_process_sandbox {
                bail!("--sandbox process is not isolation; add --insecure-allow-process-sandbox if you really want it");
            }
            Backend::Process
        }
    })
}

fn profile(args: &RunArgs) -> Result<Profile> {
    let mut p = Profile::new(backend(args)?);
    if args.insecure_allow_process_sandbox {
        p.min_isolation = IsolationLevel::None;
    }
    p.check_isolation()?;
    p.allow_network = args.allow_network;
    if args.no_system_mounts {
        p.ro_mounts.clear();
    }
    for m in &args.mounts {
        p.ro_mounts.push(parse_mount(m)?);
    }
    if let Some(d) = &args.scratch_dir {
        p.scratch_dir = d.clone();
    }
    let c = Ceilings::default();
    p.ceilings = Ceilings {
        timeout_secs: args.max_timeout_secs.unwrap_or(c.timeout_secs),
        memory_mb: args.max_memory_mb.unwrap_or(c.memory_mb),
        cpu_secs: args.max_cpu_secs.unwrap_or(c.cpu_secs),
        max_pids: args.max_pids.unwrap_or(c.max_pids),
    };
    let d = Defaults::default();
    p.defaults = Defaults {
        memory_mb: args.default_memory_mb.unwrap_or(d.memory_mb),
        cpu_secs: d.cpu_secs,
        max_pids: args.default_pids.unwrap_or(d.max_pids),
    };
    p.max_checkout_bytes = args.max_checkout_mb << 20;
    if let Some(problem) = p.scratch_problem() {
        bail!("{problem}");
    }
    Ok(p)
}

async fn run(args: RunArgs) -> Result<()> {
    let profile = profile(&args)?;
    let key =
        KeyPair::load(&args.key).with_context(|| format!("cannot load the signing key {}", args.key.display()))?;
    let token = read_token(&args)?;
    let label = args.label.clone().unwrap_or_else(default_label);
    let client = Client::connect(ClientOptions {
        endpoint: args.server.clone(),
        token,
        run: None,
        ca_file: args.ca_file.clone(),
        timeout: Some(Duration::from_secs(120)),
        user_agent: Some(format!("ag-worker/{}", env!("CARGO_PKG_VERSION"))),
    })
    .await
    .context("cannot connect to the server")?;
    let issuer = match &args.issuer {
        Some(i) => ag_types::Identity::new(i.clone()).map_err(|e| anyhow::anyhow!("bad --issuer: {e}"))?,
        None => Worker::whoami(&client).await.context("cannot ask the server who this credential is")?,
    };

    tracing::info!(
        issuer = %issuer,
        public_key = %key.public().to_hex(),
        sandbox = ?profile.backend,
        isolation = ?profile.backend.isolation(),
        mounts = profile.ro_mounts.len(),
        label = %label,
        "worker starting; the policy must list this identity and public key under trusted_issuers"
    );
    if profile.backend.isolation() == IsolationLevel::None {
        tracing::warn!("the process sandbox is NOT isolation: checks run with this user's file and network access");
    }

    let opts = Options {
        label,
        repo: args.repo.clone(),
        lease: Duration::from_secs(args.lease_secs.max(5)),
        wait: Duration::from_secs(args.wait_secs.min(30)),
        concurrency: args.concurrency.max(1),
    };
    let worker = Arc::new(Worker::new(client, profile, key, issuer, opts));

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        wait_for_signal().await;
        tracing::info!("stopping after the current job (send the signal again to exit now)");
        let _ = stop_tx.send(true);
        wait_for_signal().await;
        std::process::exit(130);
    });
    let reports = worker.run(stop_rx, args.once).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    tracing::info!(jobs = reports.len(), "worker stopped");
    Ok(())
}

async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut term) => {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        }
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mounts_parse() {
        let m = parse_mount("/opt/rust=rust-1.89.0").unwrap();
        assert_eq!(
            (m.host.to_str(), m.sandbox.to_str(), m.identity.as_deref()),
            (Some("/opt/rust"), Some("/opt/rust"), Some("rust-1.89.0"))
        );
        let m = parse_mount("/srv/tools:/opt/tools").unwrap();
        assert_eq!((m.host.to_str(), m.sandbox.to_str(), m.identity), (Some("/srv/tools"), Some("/opt/tools"), None));
        assert!(parse_mount("relative/path").is_err());
        assert!(parse_mount("/a:relative").is_err());
    }

    #[test]
    fn the_default_label_is_acceptable_to_the_server() {
        let l = default_label();
        assert!(l.len() <= 64 && l.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)), "{l}");
    }
}
