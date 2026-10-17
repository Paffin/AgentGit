#![allow(missing_docs)]
//! `ag` — the AgentGit command-line interface.

mod cmds;
mod ctx;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::ctx::Ctx;

/// AgentGit: an agent-native source control system.
#[derive(Parser, Debug)]
#[command(name = "ag", version, about, long_about = None, propagate_version = true)]
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

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a repository in the current directory.
    Init(cmds::repo::InitArgs),
    /// Show uncommitted changes in the working directory.
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
    /// Manage streams.
    #[command(subcommand)]
    Stream(cmds::flow::StreamCmd),
    /// Publish a snapshot and open a proposal to integrate it.
    Propose(cmds::flow::ProposeArgs),
    /// List, review, update or close proposals.
    #[command(subcommand)]
    Proposal(cmds::flow::ProposalCmd),
    /// Integrate a proposal into its stream.
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
    #[command(subcommand)]
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
    #[command(subcommand)]
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(exit::ERROR)
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<u8> {
    use Command::*;
    // `init` is the only command that does not need an existing repository.
    if let Init(args) = &cli.command {
        return cmds::repo::init(&cli, args);
    }
    // Cloning creates the repository; the remote subcommands read their own configuration.
    if let Clone(args) = &cli.command {
        return cmds::remote::clone(&cli, args);
    }
    if let Remote(args) = &cli.command {
        return cmds::remote::remote(&cli, args);
    }
    let ctx = Ctx::open(&cli)?;
    match &cli.command {
        Init(_) | Clone(_) | Remote(_) => unreachable!(),
        Fetch(a) => cmds::remote::fetch(&ctx, a),
        Pull(a) => cmds::remote::pull(&ctx, a),
        Push(a) => cmds::remote::push(&ctx, a),
        Status(a) => cmds::repo::status(&ctx, a),
        Snapshot(a) => cmds::repo::snapshot(&ctx, a),
        Sync(a) => cmds::repo::sync(&ctx, a),
        Log(a) => cmds::repo::log(&ctx, a),
        Show(a) => cmds::repo::show(&ctx, a),
        Diff(a) => cmds::repo::diff(&ctx, a),
        Cat(a) => cmds::repo::cat(&ctx, a),
        Blame(a) => cmds::repo::blame(&ctx, a),
        Ls(a) => cmds::repo::ls(&ctx, a),
        Checkout(a) => cmds::repo::checkout(&ctx, a),
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
