//! The Unix implementation of the `ag-worker` command line (see `main.rs`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ag_client::{Client, Options as ClientOptions};
use ag_crypto::KeyPair;
use ag_sandbox::{Availability, IsolationLevel, Mount, NamespaceSandbox, OciConfig, OciSandbox};
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
    Doctor(DoctorArgs),
}

#[derive(Args, Debug)]
struct DoctorArgs {
    /// Container runtime to probe: its daemon is asked (`<runtime> info`), not just looked for on PATH.
    #[arg(long, default_value = "docker")]
    oci_runtime: String,
    /// An image (pinned by digest) to look for in the runtime's local store. The worker never pulls.
    #[arg(long)]
    oci_image: Option<String>,
    /// Seconds to wait for each call to the container runtime.
    #[arg(long, default_value_t = 10)]
    oci_timeout_secs: u64,
    /// A cgroup v2 directory to try for kernel-exact memory and task limits (see `--cgroup-base` of `run`).
    #[arg(long, env = "AG_SANDBOX_CGROUP_BASE")]
    cgroup_base: Option<PathBuf>,
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
    /// Seconds a claim lasts without a heartbeat (at least 5).
    #[arg(long, default_value_t = 120)]
    lease_secs: u64,
    /// Seconds one claim request waits for work (at most 30, which is also what the server allows).
    #[arg(long, default_value_t = 20)]
    wait_secs: u64,
    /// Seconds a job this worker could not run waits before the server offers it again.
    #[arg(long, default_value_t = 10)]
    retry_after_secs: u64,
    /// The longest, in seconds, to wait between two failed claim requests (the wait starts at 1 and doubles).
    #[arg(long, default_value_t = 30)]
    backoff_max_secs: u64,
    /// Seconds before a call to the server times out (this includes downloading one file of the tree).
    #[arg(long, default_value_t = 120)]
    client_timeout_secs: u64,
    /// After the second stop signal, seconds the running checks get to be killed and cleaned up before the
    /// worker exits anyway.
    #[arg(long, default_value_t = 15)]
    shutdown_grace_secs: u64,

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
    /// Image used when the policy names none (pinned by digest, and already in the runtime's local store: the
    /// worker never pulls). Checked at start-up.
    #[arg(long)]
    oci_image: Option<String>,
    /// Container user, numeric `uid[:gid]`. Default: the one that can write the checked-out tree: 65534 when
    /// this worker is root (it gives the tree to that user), this worker's own ids with a rootful runtime, `0:0`
    /// with a rootless one (container root is then this user).
    #[arg(long)]
    oci_user: Option<String>,
    /// Seconds to wait for a housekeeping call to the container runtime (`info`, `inspect`, `kill`).
    #[arg(long, default_value_t = 10)]
    oci_timeout_secs: u64,
    /// For --sandbox namespace: the cgroup v2 directory under which each check gets a cgroup with kernel-exact
    /// memory and task limits. It must be writable by this user, have the memory and pids controllers enabled in
    /// its cgroup.subtree_control, and hold no processes (see docs/architecture/sandbox.md). Default: look in
    /// this process's own cgroup and its parent; without one, memory and tasks are limited by sampling and
    /// rlimits.
    #[arg(long, env = "AG_SANDBOX_CGROUP_BASE")]
    cgroup_base: Option<PathBuf>,
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
    /// Name the content of the default system mounts (an OS release, an image digest). Without it the
    /// environment digest binds only their host paths, which says nothing about what is in them.
    #[arg(long, value_name = "IDENTITY")]
    system_mounts_identity: Option<String>,
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
    /// CPU seconds (per process) for checks whose policy leaves it at 0 (default: the check's wall-clock limit
    /// times the number of CPUs, up to --max-cpu-secs).
    #[arg(long)]
    default_cpu_secs: Option<u32>,
    /// Largest accepted checkout, in MiB.
    #[arg(long, default_value_t = 4096)]
    max_checkout_mb: u64,
    /// Output kept from each of stdout and stderr, in MiB; a check that prints more is ended (default 8).
    #[arg(long)]
    output_mb: Option<u64>,
    /// Size of each check's private /tmp, in MiB (default 512). It lives in memory.
    #[arg(long)]
    tmp_mb: Option<u64>,
    /// How much of each output stream is stored with the job's result, in KiB (default 8).
    #[arg(long)]
    result_tail_kb: Option<usize>,
    /// Open files per process for every check (default 1024).
    #[arg(long)]
    open_files: Option<u64>,
    /// The largest single file a check may write, in MiB (default 1024).
    #[arg(long)]
    file_size_mb: Option<u64>,
}

/// Runs the worker command line; exits the process with status 1 on failure.
pub fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .with_ansi(
            std::io::IsTerminal::is_terminal(&std::io::stderr())
                && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()),
        )
        .init();
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Keygen { out, identity } => keygen(&out, &identity),
        Command::Doctor(args) => doctor(&args),
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

/// What the container runtime says about itself and, if asked, the image: one line each.
fn describe_oci(runtime: &str, image: Option<&str>, timeout: Duration) -> Vec<String> {
    let mut lines = Vec::new();
    match ag_sandbox::probe_runtime(runtime, timeout) {
        Err(e) => {
            lines.push(format!("unavailable: {e}"));
            return lines;
        }
        Ok(info) => {
            let yes_no = |v: Option<bool>| match v {
                Some(true) => "yes",
                Some(false) => "no",
                None => "not reported",
            };
            lines.push(format!(
                "`{runtime}` answers (version {}; rootless: {}; user namespaces: {}; default seccomp profile: {})",
                info.version.as_deref().unwrap_or("not reported"),
                yes_no(info.rootless),
                yes_no(info.user_ns),
                yes_no(info.seccomp)
            ));
            if info.seccomp == Some(false) {
                lines.push(
                    "WARNING: the runtime applies no seccomp profile by default; containers are less confined".into(),
                );
            }
        }
    }
    if let Some(image) = image {
        let mut cfg = OciConfig::new(runtime, image);
        cfg.control_timeout = timeout;
        cfg.allow_unpinned_image = true;
        match OciSandbox::with_config(cfg).and_then(|sb| sb.image_present().map(|()| sb)) {
            Ok(_) if image.contains("@sha256:") => lines.push(format!("image {image}: in the local store")),
            Ok(_) => lines.push(format!(
                "image {image}: in the local store, but NOT pinned by digest (a policy's `image` must be `name@sha256:...`)"
            )),
            Err(e) => lines.push(format!("image {image}: {e}")),
        }
    } else {
        lines.push("no image given (--oci-image): the daemon was asked, an image was not looked for".into());
    }
    lines
}

fn doctor(args: &DoctorArgs) -> Result<()> {
    let (euid, egid) = ag_sandbox::effective_ids();
    println!(
        "running as uid {euid}, gid {egid}{}",
        if euid == 0 { " (root: checks will be mapped to an unprivileged user)" } else { "" }
    );
    let timeout = Duration::from_secs(args.oci_timeout_secs);
    for (name, availability) in ag_sandbox::detect() {
        match (name, availability) {
            ("oci", Availability::Unavailable(why)) => println!("  {name:<10} unavailable: {why}"),
            ("oci", Availability::Available) => {
                println!(
                    "  {name:<10} a docker or podman executable was found on PATH; asking `{}`:",
                    args.oci_runtime
                );
                for line in describe_oci(&args.oci_runtime, args.oci_image.as_deref(), timeout) {
                    println!("             {line}");
                }
            }
            (_, Availability::Available) => println!("  {name:<10} available"),
            (_, Availability::Unavailable(why)) => println!("  {name:<10} unavailable: {why}"),
        }
        if name == "namespace" {
            let mut sandbox = NamespaceSandbox::new();
            if let Some(base) = &args.cgroup_base {
                sandbox = sandbox.with_cgroup_base(base);
            }
            match sandbox.cgroup_availability() {
                Ok(r) => println!(
                    "             cgroup v2: kernel-exact limits for {}{}",
                    if r.memory { "memory" } else { "" },
                    match (r.memory, r.pids) {
                        (true, true) => " and tasks",
                        (false, true) => "tasks only",
                        _ => " only",
                    }
                ),
                Err(why) => println!(
                    "             cgroup v2: not used ({why}); memory is limited by sampling and tasks by rlimits or sampling"
                ),
            }
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
    let defaults = Profile::new(Backend::Process);
    let unnamed = defaults.mounts_without_identity().len();
    println!(
        "the {unnamed} default system mounts carry no identity: the environment digest binds only their host paths (see --system-mounts-identity and --mount HOST=IDENTITY)"
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

/// A namespace sandbox the way this worker would build it, for the start-up check of `--cgroup-base`.
fn namespace_sandbox(cgroup_base: &Option<PathBuf>) -> NamespaceSandbox {
    match cgroup_base {
        Some(base) => NamespaceSandbox::new().with_cgroup_base(base),
        None => NamespaceSandbox::new(),
    }
}

fn namespace_backend(args: &RunArgs) -> Result<Backend> {
    if let Availability::Unavailable(why) = NamespaceSandbox::new().availability() {
        bail!("the namespace sandbox is unavailable: {why}");
    }
    // A base the operator named has to work: it is the difference between kernel-exact and sampled limits.
    if args.cgroup_base.is_some()
        && let Err(why) = namespace_sandbox(&args.cgroup_base).cgroup_availability()
    {
        bail!(
            "--cgroup-base {} cannot be used: {why}",
            args.cgroup_base.as_deref().map(|p| p.display().to_string()).unwrap_or_default()
        );
    }
    Ok(Backend::Namespace { host_ids: (args.sandbox_uid, args.sandbox_gid), cgroup_base: args.cgroup_base.clone() })
}

/// The container backend, checked against the real runtime at start-up instead of failing every job later:
/// the daemon answers, the default image is a pinned one that is in the local store, the container user can
/// write the checked-out tree.
fn oci_backend(args: &RunArgs) -> Result<Backend> {
    let (euid, egid) = ag_sandbox::effective_ids();
    let timeout = Duration::from_secs(args.oci_timeout_secs);
    let info = ag_sandbox::probe_runtime(&args.oci_runtime, timeout).map_err(|e| anyhow::anyhow!("{e}"))?;
    let rootless = info.rootless == Some(true);
    if info.seccomp == Some(false) {
        tracing::warn!(runtime = %args.oci_runtime, "the container runtime applies no seccomp profile by default");
    }
    let user = match &args.oci_user {
        None => info.user_for_tree(euid, egid, &format!("{DEFAULT_SANDBOX_ID}:{DEFAULT_SANDBOX_ID}")),
        Some(user) => {
            let Some((uid, _)) = ag_worker::parse_numeric_user(user) else {
                bail!("--oci-user '{user}' must be a numeric uid[:gid]");
            };
            // A user who is not the one that owns the tree can write it only if the worker hands it over, and
            // only root can.
            if euid != 0 && !(if rootless { uid == 0 } else { uid == euid }) {
                bail!(
                    "--oci-user {user} could not write the checked-out tree: this worker runs as uid {euid}, and with a {} runtime the container user that owns the tree is {}. Leave --oci-user out to use it.",
                    if rootless { "rootless" } else { "rootful" },
                    if rootless { "0:0 (container root is this user)".to_string() } else { format!("{euid}:{egid}") }
                );
            }
            user.clone()
        }
    };
    if let Some(image) = &args.oci_image {
        let mut cfg = OciConfig::new(&args.oci_runtime, image);
        cfg.control_timeout = timeout;
        cfg.user = user.clone();
        let sandbox = OciSandbox::with_config(cfg).map_err(|e| anyhow::anyhow!("--oci-image: {e}"))?;
        sandbox.image_present().map_err(|e| anyhow::anyhow!("--oci-image: {e}"))?;
    }
    // Containers a crashed predecessor left behind (and the memory and processes in them) are removed now.
    match ag_sandbox::reap_orphans(&args.oci_runtime, timeout, timeout * 2) {
        Ok(removed) if removed.is_empty() => {}
        Ok(removed) => tracing::warn!(containers = ?removed, "removed containers a previous worker left behind"),
        Err(e) => tracing::warn!(error = %e, "could not look for containers a previous worker left behind"),
    }
    Ok(Backend::Oci {
        runtime: args.oci_runtime.clone(),
        default_image: args.oci_image.clone(),
        user,
        control_timeout: timeout,
    })
}

fn backend(args: &RunArgs) -> Result<Backend> {
    Ok(match args.sandbox {
        SandboxChoice::Auto => match NamespaceSandbox::new().availability() {
            Availability::Available => namespace_backend(args)?,
            Availability::Unavailable(why) => bail!(
                "no isolating sandbox is available here ({why}). Fix that (see `ag-worker doctor`), use --sandbox oci, or, for code you already trust, --sandbox process --insecure-allow-process-sandbox"
            ),
        },
        SandboxChoice::Namespace => namespace_backend(args)?,
        SandboxChoice::Oci => oci_backend(args)?,
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
    if let Some(problem) = p.backend.user_problem() {
        bail!("{problem}");
    }
    if args.insecure_allow_process_sandbox {
        p.min_isolation = IsolationLevel::None;
    }
    p.check_isolation()?;
    p.allow_network = args.allow_network;
    if args.no_system_mounts {
        p.ro_mounts.clear();
    }
    if let Some(identity) = &args.system_mounts_identity {
        if identity.is_empty() || args.no_system_mounts {
            bail!("--system-mounts-identity needs a name and the system mounts (not --no-system-mounts)");
        }
        p.ro_mounts = p.ro_mounts.into_iter().map(|m| m.with_identity(identity.clone())).collect();
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
        cpu_secs: args.default_cpu_secs.unwrap_or(d.cpu_secs),
        max_pids: args.default_pids.unwrap_or(d.max_pids),
    };
    p.max_checkout_bytes = args.max_checkout_mb << 20;
    // Every one of these is a limit of the sandbox, which refuses a zero.
    let positive = |what: &str, v: Option<u64>, unit: u64, into: &mut u64| -> Result<()> {
        if let Some(v) = v {
            if v == 0 {
                bail!("--{what} must be greater than zero");
            }
            *into = v.checked_mul(unit).with_context(|| format!("--{what} is unreasonably large"))?;
        }
        Ok(())
    };
    positive("output-mb", args.output_mb, 1 << 20, &mut p.output_bytes)?;
    positive("tmp-mb", args.tmp_mb, 1, &mut p.tmp_mb)?;
    positive("open-files", args.open_files, 1, &mut p.open_files)?;
    positive("file-size-mb", args.file_size_mb, 1 << 20, &mut p.file_size_bytes)?;
    if let Some(kb) = args.result_tail_kb {
        if kb == 0 {
            bail!("--result-tail-kb must be greater than zero");
        }
        p.tail_bytes = kb.checked_mul(1024).context("--result-tail-kb is unreasonably large")?;
    }
    if let Some(problem) = p.scratch_problem() {
        bail!("{problem}");
    }
    Ok(p)
}

async fn run(args: RunArgs) -> Result<()> {
    if args.lease_secs < 5 {
        bail!(
            "--lease-secs {} is below the minimum of 5: a claim shorter than that cannot be kept alive",
            args.lease_secs
        );
    }
    if args.wait_secs > 30 {
        tracing::warn!(asked = args.wait_secs, used = 30, "--wait-secs is capped at 30 (the server's own limit)");
    }
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
        timeout: Some(Duration::from_secs(args.client_timeout_secs)),
        user_agent: Some(format!("ag-worker/{}", env!("CARGO_PKG_VERSION"))),
        ..Default::default()
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
    let unnamed: Vec<String> = profile.mounts_without_identity().iter().map(|m| m.host.display().to_string()).collect();
    if !unnamed.is_empty() {
        tracing::warn!(
            mounts = ?unnamed,
            "these read-only mounts carry no identity: the environment digest binds only their host paths, not their content, so a policy that pins required_environment_digest cannot tell this worker's toolchain from another's (--system-mounts-identity, --mount HOST=IDENTITY)"
        );
    }
    if let Backend::Namespace { cgroup_base, .. } = &profile.backend {
        match namespace_sandbox(cgroup_base).cgroup_availability() {
            Ok(r) => tracing::info!(memory = r.memory, pids = r.pids, "cgroup v2 limits: enforced by the kernel"),
            Err(why) => tracing::info!(
                reason = %why,
                "no cgroup v2 limits: memory is limited by sampling and tasks by rlimits or sampling (see docs/architecture/sandbox.md)"
            ),
        }
    }

    let opts = Options {
        label,
        repo: args.repo.clone(),
        lease: Duration::from_secs(args.lease_secs),
        wait: Duration::from_secs(args.wait_secs.min(30)),
        concurrency: args.concurrency.max(1),
        retry_after: Duration::from_secs(args.retry_after_secs),
        backoff_max: Duration::from_secs(args.backoff_max_secs.max(1)),
    };
    let worker = Arc::new(Worker::new(client, profile, key, issuer, opts));

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let grace = Duration::from_secs(args.shutdown_grace_secs);
    let stopper = worker.clone();
    tokio::spawn(async move {
        wait_for_signal().await;
        tracing::info!("stopping after the current job (send the signal again to stop the running checks now)");
        let _ = stop_tx.send(true);
        wait_for_signal().await;
        tracing::warn!(?grace, "stopping the running checks now; their containers are killed and removed");
        stopper.cancel();
        tokio::time::sleep(grace).await;
        tracing::error!(?grace, "the checks did not stop in time; exiting anyway");
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

    fn args(extra: &[&str]) -> RunArgs {
        let mut argv = vec!["ag-worker", "run", "--server", "http://x", "--key", "/k", "--sandbox", "process"];
        argv.extend_from_slice(extra);
        match Cli::try_parse_from(argv).unwrap().command {
            Command::Run(a) => *a,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn every_limit_a_worker_operator_may_want_to_change_is_a_flag() {
        let a = args(&[
            "--insecure-allow-process-sandbox",
            "--output-mb",
            "16",
            "--tmp-mb",
            "2048",
            "--result-tail-kb",
            "64",
            "--open-files",
            "4096",
            "--file-size-mb",
            "8192",
            "--default-cpu-secs",
            "900",
            "--max-checkout-mb",
            "100",
        ]);
        let p = profile(&a).unwrap();
        assert_eq!(p.output_bytes, 16 << 20);
        assert_eq!(p.tmp_mb, 2048);
        assert_eq!(p.tail_bytes, 64 * 1024);
        assert_eq!(p.open_files, 4096);
        assert_eq!(p.file_size_bytes, 8 << 30);
        assert_eq!(p.defaults.cpu_secs, 900);
        assert_eq!(p.max_checkout_bytes, 100 << 20);
        // nothing given: the profile's own defaults
        let d = profile(&args(&["--insecure-allow-process-sandbox"])).unwrap();
        let base = Profile::new(Backend::Process);
        assert_eq!((d.output_bytes, d.tmp_mb, d.tail_bytes), (base.output_bytes, base.tmp_mb, base.tail_bytes));
        assert_eq!((d.open_files, d.file_size_bytes), (base.open_files, base.file_size_bytes));
    }

    #[test]
    fn a_limit_of_zero_is_refused_not_passed_to_the_sandbox() {
        for flag in ["--output-mb", "--tmp-mb", "--result-tail-kb", "--open-files", "--file-size-mb"] {
            let a = args(&["--insecure-allow-process-sandbox", flag, "0"]);
            let err = profile(&a).unwrap_err().to_string();
            assert!(err.contains(flag) && err.contains("greater than zero"), "{err}");
        }
        let err = profile(&args(&["--insecure-allow-process-sandbox", "--file-size-mb", "18446744073709551615"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unreasonably large"), "{err}");
    }

    #[test]
    fn the_system_mounts_can_be_named_and_the_digest_then_binds_what_they_are() {
        let a = args(&["--insecure-allow-process-sandbox", "--system-mounts-identity", "debian-12.5"]);
        let p = profile(&a).unwrap();
        assert!(p.mounts_without_identity().is_empty());
        assert!(p.ro_mounts.iter().all(|m| m.identity.as_deref() == Some("debian-12.5")));
        let err = profile(&args(&[
            "--insecure-allow-process-sandbox",
            "--system-mounts-identity",
            "x",
            "--no-system-mounts",
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("--system-mounts-identity"), "{err}");
    }

    #[test]
    fn the_oci_user_is_numeric_and_the_flag_has_no_hidden_default() {
        let a = args(&["--oci-user", "nobody"]);
        assert_eq!(a.oci_user.as_deref(), Some("nobody"));
        assert_eq!(args(&[]).oci_user, None, "the right user depends on the runtime and is chosen at start-up");
        assert_eq!(args(&[]).oci_timeout_secs, 10);
    }
}
