#![allow(missing_docs)]
//! `ag` — the AgentGit command-line interface.

mod cmds;
mod ctx;
mod errors;
mod ui;
mod userconf;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use crate::ctx::Ctx;
use crate::ui::{ColorEnv, ColorMode, Settings};

/// The version of the `--json` output shapes (see `docs/reference/cli-json.md`). Within a version keys are only
/// ever added.
pub const JSON_SCHEMA_VERSION: u32 = 1;

const ROOT_HELP: &str = "\
Common workflow:
  ag init --namespace me/app     create a repository in this directory
  ag snapshot -m \"message\"       record the working directory
  ag propose -t \"title\"          publish it and open a proposal
  ag integrate PROPOSAL          land it (policy decides; any unique id prefix works)

Where am I, is it healthy:
  ag status                      workspace, your proposals, leases, task and remote at a glance
  ag doctor                      check the installation, the repository, your identity and every remote

Exit codes: 0 ok, 1 error, 2 usage, 3 blocked (provide what is missing and run it again),
4 refused (conflict, policy denial; the stream was not changed). Add --json for machine-readable output.

Help topics (ag help TOPIC): workflow, policy, leases, identities, revisions, exit-codes, json, config
Shell completion: ag completions bash|zsh|fish|powershell|elvish     Man pages: ag man --dir DIR
Documentation: https://github.com/Paffin/AgentGit/tree/main/docs";

/// AgentGit: an agent-native source control system.
#[derive(Parser, Debug)]
#[command(
    name = "ag",
    version = env!("AG_LONG_VERSION"),
    about,
    long_about = "AgentGit: an agent-native source control system. People, agents and CI change one codebase at \
the same time; every change lands through a proposal, and a per-stream policy decides what must be true first \
(validation of the exact tree, approvals, leases, an autonomy envelope for agents).",
    after_help = ROOT_HELP,
    propagate_version = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Operate on the repository at this path (default: discovered from the current directory).
    #[arg(short = 'C', long = "repo", global = true, value_name = "DIR")]
    pub repo: Option<PathBuf>,

    /// Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY.
    #[arg(long = "as", global = true, value_name = "IDENTITY")]
    pub identity: Option<String>,

    /// Machine-readable JSON output.
    #[arg(long, global = true)]
    pub json: bool,

    /// Colour the output: auto (a terminal, unless NO_COLOR is set), always or never.
    #[arg(long, global = true, value_enum, value_name = "WHEN", hide_possible_values = true)]
    pub color: Option<ColorMode>,

    /// Print only what was asked for: no confirmations or hints. Errors and refusals are still shown.
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Explain what is happening on standard error: repository, identity, remote, timing.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a repository in the current directory.
    Init(cmds::repo::InitArgs),
    /// Summarize the workspace, your proposals, leases, task and remote at a glance.
    Status(cmds::repo::StatusArgs),
    /// Record the working directory as a new snapshot.
    Snapshot(cmds::repo::SnapshotArgs),
    /// Bring the stream's latest head into the workspace.
    Sync(cmds::repo::SyncArgs),
    /// Show history.
    Log(cmds::repo::LogArgs),
    /// Show one changeset and what it changed.
    Show(cmds::repo::ShowArgs),
    /// Show differences between two revisions, or the working directory and its snapshot.
    Diff(cmds::repo::DiffArgs),
    /// Print a file as of a revision: `ag cat main:src/lib.rs`.
    Cat(cmds::repo::CatArgs),
    /// Show which changeset introduced each line of a file.
    Blame(cmds::repo::BlameArgs),
    /// List a directory as of a revision.
    Ls(cmds::repo::LsArgs),
    /// Write a revision's files into a new directory.
    Checkout(cmds::repo::CheckoutArgs),
    /// List what a revision contains that Windows, macOS or Linux file systems cannot hold.
    Portability(cmds::repo::PortabilityArgs),
    /// Manage streams.
    #[command(subcommand)]
    Stream(cmds::flow::StreamCmd),
    /// Publish a snapshot and open a proposal to integrate it.
    #[command(
        after_help = "Examples:\n  ag propose -t \"Fix the parser\"            propose the workspace head\n  ag propose @docs -t \"Document usage\"      propose the head of workspace 'docs'\n  ag propose --task task_… -t \"…\"           attach the proposal to a task"
    )]
    Propose(cmds::flow::ProposeArgs),
    /// List, review, update or close proposals.
    #[command(subcommand)]
    Proposal(cmds::flow::ProposalCmd),
    /// Integrate a proposal into its stream.
    #[command(
        after_help = "Exit codes: 0 integrated, 3 blocked (provide what is missing and run it again), 4 conflict, denied or invalid.\nThe stream is untouched unless the exit code is 0. Examples:\n  ag integrate prop_8f3a1c                        any unique id prefix works\n  ag integrate prop_8f3a1c --fencing-token 7      present the token of your exclusive lease\n  ag --json integrate prop_8f3a1c                 stable blocker codes for scripts and agents"
    )]
    Integrate(cmds::flow::IntegrateArgs),
    /// Show what integrating a proposal would do, without doing it.
    Preview(cmds::flow::PreviewArgs),
    /// Inspect or resume operations.
    #[command(subcommand)]
    Op(cmds::flow::OpCmd),
    /// Resume interrupted operations and expire stale ones.
    Recover(cmds::flow::RecoverArgs),
    /// The ordered integration queue.
    #[command(subcommand)]
    Queue(cmds::flow::QueueCmd),
    /// Show or change stream policy.
    #[command(subcommand, after_help = "Concepts and fields: ag help policy")]
    Policy(cmds::policy::PolicyCmd),
    /// Run checks and issue signed attestations.
    #[command(subcommand)]
    Validate(cmds::policy::ValidateCmd),
    /// Agent identities and runs.
    #[command(subcommand)]
    Agent(cmds::agents::AgentCmd),
    /// Tasks and their dependencies.
    #[command(subcommand)]
    Task(cmds::agents::TaskCmd),
    /// Soft and exclusive leases with fencing tokens.
    #[command(subcommand, after_help = "Concepts: ag help leases")]
    Lease(cmds::agents::LeaseCmd),
    /// Signed provenance of agent-made changes.
    #[command(subcommand)]
    Provenance(cmds::agents::ProvCmd),
    /// Predict overlapping work.
    Predict(cmds::agents::PredictArgs),
    /// Create, list and remove workspaces (isolated lines of work in their own directories).
    #[command(subcommand)]
    Workspace(cmds::repo::WorkspaceCmd),
    /// Show the audit trail of an object, proposal, run or stream.
    Trace(cmds::admin::TraceArgs),
    /// Verify the tamper-evident audit chain.
    Audit(cmds::admin::AuditArgs),
    /// Check the integrity of the whole repository.
    Verify,
    /// Delete unreachable objects (safe: fails closed, honours a grace period).
    Gc(cmds::admin::GcArgs),
    /// Rebuild derived indexes from the object store.
    Reindex,
    /// Write a verified, consistent copy of the repository to a new directory.
    Backup(cmds::admin::BackupArgs),
    /// Import from or export to Git (needs the `git` command).
    #[command(subcommand)]
    Git(cmds::gitbridge::GitCmd),
    /// Clone a repository from an `ag-server`.
    Clone(cmds::remote::CloneArgs),
    /// Download new history from the server.
    Fetch(cmds::remote::SyncArgs),
    /// Fetch, then fast-forward the local stream and bring the workspace up to date.
    Pull(cmds::remote::SyncArgs),
    /// Upload your snapshot and open a proposal on the server.
    Push(cmds::remote::PushArgs),
    /// Manage remotes and work with proposals, events and policy on a server.
    Remote(cmds::remote::RemoteArgs),
    /// Pin a revision so garbage collection keeps it.
    Pin(cmds::admin::PinArgs),
    /// Remove a pin.
    Unpin(cmds::admin::UnpinArgs),
    /// List pins.
    Pins,
    /// Check the installation, the repository, your identity and every remote's credential.
    Doctor(cmds::doctor::DoctorArgs),
    /// Show, get, set and locate configuration (the user file; the repository's settings are shown read-only).
    #[command(subcommand, after_help = "Files, environment variables and precedence: ag help config")]
    Config(cmds::config::ConfigCmd),
    /// Print a shell completion script: bash, zsh, fish, powershell or elvish.
    #[command(
        after_help = "Examples:\n  ag completions bash > ~/.local/share/bash-completion/completions/ag\n  ag completions zsh  > \"${fpath[1]}/_ag\"\n  ag completions fish > ~/.config/fish/completions/ag.fish"
    )]
    Completions(cmds::shell::CompletionsArgs),
    /// Print the man page, or write one page per command with --dir.
    Man(cmds::shell::ManArgs),
    /// Print the version, the protocol and the JSON schema version.
    Version,
    /// Show help for a command, or a topic: workflow, policy, leases, identities, revisions, exit-codes, json, config.
    Help(cmds::topics::HelpArgs),
}

/// Process exit codes (stable; agents may rely on them).
pub mod exit {
    /// Success.
    pub const OK: u8 = 0;
    /// An error occurred.
    pub const ERROR: u8 = 1;
    // 2 is reserved: the argument parser uses it for usage errors.
    /// The operation is blocked on something that can be provided (approval, validation, lease).
    pub const BLOCKED: u8 = 3;
    /// Refused: conflict, policy denial or invalid request.
    pub const REFUSED: u8 = 4;
}

/// Finds `--color WHEN` / `--color=WHEN` before the argument parser runs, so that even its own help and usage
/// errors follow the flag.
fn prescan_color(args: &[OsString]) -> Option<ColorMode> {
    let mut it = args.iter().skip(1).filter_map(|a| a.to_str());
    while let Some(a) = it.next() {
        if a == "--" {
            return None;
        }
        if let Some(v) = a.strip_prefix("--color=") {
            return ColorMode::parse(v);
        }
        if a == "--color" {
            return it.next().and_then(ColorMode::parse);
        }
    }
    None
}

fn main() -> ExitCode {
    let started = std::time::Instant::now();
    let args: Vec<OsString> = std::env::args_os().collect();
    let flag = prescan_color(&args);
    let setting = userconf::load_lenient().get("color").and_then(ColorMode::parse);
    let env = ColorEnv::from_process();
    let color_out = ui::resolve_color(flag, setting, &env, ui::is_tty(false));
    let color_err = ui::resolve_color(flag, setting, &env, ui::is_tty(true));
    // Let the argument parser decide by itself unless the user chose; it understands NO_COLOR and terminals.
    let choice = match (flag, setting) {
        (None, None | Some(ColorMode::Auto)) | (Some(ColorMode::Auto), _) => clap::ColorChoice::Auto,
        _ if color_out => clap::ColorChoice::Always,
        _ => clap::ColorChoice::Never,
    };

    let mut command = Cli::command().color(choice);
    let matches = command.try_get_matches_from_mut(&args).unwrap_or_else(|e| e.exit());
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    ui::init(Settings { color_out, color_err, quiet: cli.quiet, verbose: cli.verbose });

    let json = cli.json;
    let code = match run(cli) {
        Ok(code) => code,
        Err(e) => {
            ui::eprint_line(&errors::render(&e, json, exit::ERROR));
            exit::ERROR
        }
    };
    ui::verbose(format!("exit code {code} after {} ms", started.elapsed().as_millis()));
    ExitCode::from(code)
}

fn run(cli: Cli) -> anyhow::Result<u8> {
    use Command::*;
    // Commands that work without a repository.
    match &cli.command {
        Init(args) => return cmds::repo::init(&cli, args),
        Clone(args) => return cmds::remote::clone(&cli, args),
        Remote(args) => return cmds::remote::remote(&cli, args),
        Doctor(args) => return cmds::doctor::doctor(&cli, args),
        Config(c) => return cmds::config::config(&cli, c),
        Completions(a) => return cmds::shell::completions(a),
        Man(a) => return cmds::shell::man(a),
        Version => return cmds::shell::version(&cli),
        Help(a) => return cmds::topics::help(a),
        _ => {}
    }
    let ctx = Ctx::open(&cli)?;
    match &cli.command {
        Init(_) | Clone(_) | Remote(_) | Doctor(_) | Config(_) | Completions(_) | Man(_) | Version | Help(_) => {
            unreachable!()
        }
        Fetch(a) => cmds::remote::fetch(&ctx, a),
        Pull(a) => cmds::remote::pull(&ctx, a),
        Push(a) => cmds::remote::push(&ctx, a),
        Status(a) => cmds::repo::status(&cli, &ctx, a),
        Snapshot(a) => cmds::repo::snapshot(&ctx, a),
        Sync(a) => cmds::repo::sync(&ctx, a),
        Log(a) => cmds::repo::log(&ctx, a),
        Show(a) => cmds::repo::show(&ctx, a),
        Diff(a) => cmds::repo::diff(&ctx, a),
        Cat(a) => cmds::repo::cat(&ctx, a),
        Blame(a) => cmds::repo::blame(&ctx, a),
        Ls(a) => cmds::repo::ls(&ctx, a),
        Checkout(a) => cmds::repo::checkout(&ctx, a),
        Portability(a) => cmds::repo::portability(&ctx, a),
        Stream(c) => cmds::flow::stream(&ctx, c),
        Propose(a) => cmds::flow::propose(&ctx, a),
        Proposal(c) => cmds::flow::proposal(&ctx, c),
        Integrate(a) => cmds::flow::integrate(&ctx, a),
        Preview(a) => cmds::flow::preview(&ctx, a),
        Op(c) => cmds::flow::op(&ctx, c),
        Recover(a) => cmds::flow::recover(&ctx, a),
        Queue(c) => cmds::flow::queue(&ctx, c),
        Policy(c) => cmds::policy::policy(&ctx, c),
        Validate(c) => cmds::policy::validate(&ctx, c),
        Agent(c) => cmds::agents::agent(&ctx, c),
        Task(c) => cmds::agents::task(&ctx, c),
        Lease(c) => cmds::agents::lease(&ctx, c),
        Provenance(c) => cmds::agents::provenance(&ctx, c),
        Predict(a) => cmds::agents::predict(&ctx, a),
        Workspace(c) => cmds::repo::workspace(&ctx, c),
        Trace(a) => cmds::admin::trace(&ctx, a),
        Audit(a) => cmds::admin::audit(&ctx, a),
        Verify => cmds::admin::verify(&ctx),
        Gc(a) => cmds::admin::gc(&ctx, a),
        Reindex => cmds::admin::reindex(&ctx),
        Backup(a) => cmds::admin::backup(&ctx, a),
        Git(c) => cmds::gitbridge::git(&ctx, c),
        Pin(a) => cmds::admin::pin(&ctx, a),
        Unpin(a) => cmds::admin::unpin(&ctx, a),
        Pins => cmds::admin::pins(&ctx),
    }
}
