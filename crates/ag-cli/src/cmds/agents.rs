//! Agent, task, lease, provenance and prediction commands.

use std::fmt::Write as _;
use std::path::PathBuf;

use ag_agent::provenance::{self, ProvenanceRecord};
use ag_agent::registry::{self, KeyStore};
use ag_agent::{leases, predict as predictor, tasks};
use ag_types::Identity;
use anyhow::{Context, Result, anyhow};
use clap::{Args, Subcommand};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::cmds::StreamSel;
use crate::ctx::{Ctx, parse_duration_ms};
use crate::exit;
use crate::ui;

// ---- agents ----------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum AgentCmd {
    /// Register an agent and create its signing key.
    Register {
        /// Agent name (`[a-z0-9][a-z0-9_-]*`).
        name: String,
        /// `agent` or `reviewer`.
        #[arg(long, default_value = "agent")]
        role: String,
    },
    /// List agents.
    List {
        /// Read a server's agents instead of the local repository (without a NAME: origin).
        #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "origin")]
        remote: Option<String>,
    },
    /// Start a run. Prints the identity the agent must use (`agent:name#run_…`).
    Start {
        /// Agent name.
        name: String,
        /// Task to work on (claims it; its dependencies must be done).
        #[arg(long)]
        task: Option<String>,
    },
    /// Finish a run and release its leases.
    Finish {
        /// Run id.
        run: String,
        /// `completed` or `failed`.
        #[arg(long, default_value = "completed")]
        status: String,
        /// Summary.
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Heartbeat a running run.
    Heartbeat {
        /// Run id.
        run: String,
    },
    /// List runs, newest first.
    Runs {
        /// Only this agent.
        name: Option<String>,
        /// Show at most this many (0 = all).
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: usize,
    },
    /// Abandon runs with no recent heartbeat and release their leases.
    Reap {
        /// Heartbeat timeout (default 10m here; with `--remote`, the server's `agents.run_stale_after_secs`).
        #[arg(long)]
        timeout: Option<String>,
        /// Say what would be abandoned without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Reap the runs of a server instead of this repository (needs `agent.operate` there; without a NAME: origin).
        /// The server also revokes the credentials bound to the reaped runs.
        #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "origin")]
        remote: Option<String>,
    },
    /// Stop an agent from starting new runs.
    Disable {
        /// Agent name.
        name: String,
        /// Change the server's agent instead (requires agent.operate; without a NAME: origin).
        #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "origin")]
        remote: Option<String>,
    },
    /// Allow a disabled agent to run again.
    Enable {
        /// Agent name.
        name: String,
        /// Change the server's agent instead (requires agent.operate; without a NAME: origin).
        #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "origin")]
        remote: Option<String>,
    },
}

pub fn agent(ctx: &Ctx, cmd: &AgentCmd) -> Result<u8> {
    let meta = ctx.repo.meta();
    match cmd {
        AgentCmd::Register { name, role } => {
            let keys = KeyStore::open(ctx.repo.layout().keys_dir())?;
            let row = registry::register_agent(meta, &keys, name, role, ctx.me.as_str())?;
            ctx.done(&row, || {
                format!(
                    "Registered {} (key {}). Its private key is {}.",
                    row.identity,
                    &row.public_key[..16],
                    keys.path(name).display()
                )
            })?;
        }
        AgentCmd::List { remote: Some(remote) } => {
            let rows = super::remote::list_agents(ctx, remote)?;
            ctx.out(&rows, || {
                rows.iter()
                    .map(|row| {
                        format!("{}  {}{}", row.identity, row.role, if row.disabled { "  [disabled]" } else { "" })
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })?;
        }
        AgentCmd::List { remote: None } => {
            let rows = meta.read(|tx| tx.agents())?;
            if rows.is_empty() {
                ctx.note("no agents yet; `ag agent register NAME` creates one");
            }
            ctx.out(&rows, || {
                let mut s = String::new();
                for r in &rows {
                    let _ = writeln!(
                        s,
                        "{:<20} {:<8} key {}{}",
                        r.identity,
                        r.role,
                        &r.public_key[..16],
                        if r.disabled { "  [disabled]" } else { "" }
                    );
                }
                s
            })?;
        }
        AgentCmd::Start { name, task } => {
            let run = registry::start_run(meta, name, task.as_deref())?;
            // With --quiet: just the identity to act as, so `export AG_IDENTITY=$(ag -q agent start NAME)` works.
            ctx.created(&run.identity.to_string(), &json!({"run_id": run.run_id, "identity": run.identity}), || {
                format!("Run {}\nAct as: {}\n  export AG_IDENTITY={}", run.run_id, run.identity, run.identity)
            })?;
        }
        AgentCmd::Finish { run, status, message } => {
            registry::finish_run(meta, run, status, message.as_deref())?;
            ctx.done(&json!({"run": run, "status": status}), || format!("Run {run} {status}; leases released"))?;
        }
        AgentCmd::Heartbeat { run } => {
            registry::heartbeat(meta, run)?;
            ctx.done(&json!({"run": run}), || format!("ok {run}"))?;
        }
        AgentCmd::Runs { name, limit } => {
            let rows = meta.read(|tx| tx.runs(name.as_deref(), Ctx::cap(*limit)))?;
            ctx.note_cap(rows.len(), *limit, "runs");
            if rows.is_empty() {
                ctx.note("no runs yet; `ag agent start NAME` starts one");
            }
            ctx.out(&rows, || {
                let mut s = String::new();
                for r in &rows {
                    let _ = writeln!(
                        s,
                        "{}  {:<10} {:<12} task {}  heartbeat {}",
                        r.run_id,
                        r.status,
                        r.agent,
                        r.task_id.clone().unwrap_or_else(|| "-".into()),
                        r.last_heartbeat
                    );
                }
                s
            })?;
        }
        AgentCmd::Reap { timeout, dry_run, remote } => {
            if let Some(name) = remote {
                let stale_after_ms = match timeout {
                    Some(t) => {
                        u64::try_from(parse_duration_ms(t)?).map_err(|_| anyhow!("the timeout must not be negative"))?
                    }
                    None => 0,
                };
                let r = crate::cmds::remote::reap_runs(ctx, name, stale_after_ms, *dry_run)?;
                ctx.out(&r, || {
                    let mut s = format!(
                        "{} {} run(s), {} lease(s) released, {} credential(s) revoked",
                        if r.dry_run { "Would abandon" } else { "Abandoned" },
                        r.runs.len(),
                        r.released_leases.len(),
                        r.revoked_credentials
                    );
                    for run in &r.runs {
                        let _ = write!(s, "\n  {}  {}", run.run_id, run.agent);
                    }
                    s
                })?;
                return Ok(exit::OK);
            }
            let timeout = parse_duration_ms(timeout.as_deref().unwrap_or("10m"))?;
            let reaped = registry::reap_runs(meta, timeout, ctx.me.as_str(), *dry_run)?;
            let ids: Vec<String> = reaped.runs.into_iter().map(|r| r.run_id).collect();
            ctx.out(&ids, || {
                format!(
                    "{} {} run(s){}",
                    if *dry_run { "Would abandon" } else { "Abandoned" },
                    ids.len(),
                    ids.iter().map(|r| format!("\n  {r}")).collect::<String>()
                )
            })?;
        }
        AgentCmd::Disable { name, remote } => {
            if let Some(remote) = remote {
                super::remote::set_agent_state(ctx, remote, name, true)?;
            } else {
                registry::set_disabled(meta, name, true, None, ctx.me.as_str())?;
            }
            ctx.done(&json!({"disabled": name}), || format!("Disabled {name}"))?;
        }
        AgentCmd::Enable { name, remote } => {
            if let Some(remote) = remote {
                super::remote::set_agent_state(ctx, remote, name, false)?;
            } else {
                registry::set_disabled(meta, name, false, None, ctx.me.as_str())?;
            }
            ctx.done(&json!({"enabled": name}), || format!("Enabled {name}"))?;
        }
    }
    Ok(exit::OK)
}

// ---- tasks -----------------------------------------------------------------

/// Partial instruction replacement shared by local and remote edits.
#[derive(Args, Debug)]
#[group(required = true, multiple = true)]
pub struct TaskEdit {
    /// New title; omitted keeps the current title.
    #[arg(long)]
    pub title: Option<String>,
    /// New description; an empty string clears it.
    #[arg(long)]
    pub description: Option<String>,
    /// Replace path scope (repeatable).
    #[arg(long, conflicts_with = "clear_scope")]
    pub scope: Vec<String>,
    /// Remove the expected file-path scope (repository policy still applies).
    #[arg(long)]
    pub clear_scope: bool,
}

impl TaskEdit {
    pub(super) fn apply(&self, current: &tasks::TaskContent) -> tasks::TaskContent {
        tasks::TaskContent {
            title: self.title.clone().unwrap_or_else(|| current.title.clone()),
            description: self.description.clone().unwrap_or_else(|| current.description.clone()),
            scope: if self.clear_scope || !self.scope.is_empty() { self.scope.clone() } else { current.scope.clone() },
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum TaskCmd {
    /// Create a task.
    Create {
        /// Title.
        title: String,
        /// Description.
        #[arg(short, long, default_value = "")]
        description: String,
        /// Path glob the task is expected to touch (repeatable).
        #[arg(long)]
        scope: Vec<String>,
        /// Original task for an independent follow-up.
        #[arg(long)]
        source_task: Option<String>,
        /// Failed or abandoned run of --source-task.
        #[arg(long, requires = "source_task")]
        source_run: Option<String>,
        /// Total runs permitted (0 leaves the server limit unset).
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u32).range(0..=10000))]
        max_attempts: u32,
    },
    /// Refine instructions before a task's first run; concurrent edits are refused.
    Edit {
        /// Task identifier.
        task: String,
        #[command(flatten)]
        changes: TaskEdit,
        /// Edit on a configured server instead of locally (default name: origin).
        #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "origin")]
        remote: Option<String>,
    },
    /// Inspect or change a task's durable execution-attempt limit.
    Attempts {
        /// Task identifier.
        task: String,
        /// New maximum total runs; zero leaves the server limit unset.
        #[arg(long, requires = "expected", value_parser = clap::value_parser!(u32).range(0..=10000))]
        max: Option<u32>,
        /// Limit you inspected; refuses a concurrent policy change.
        #[arg(long, requires = "max", value_parser = clap::value_parser!(u32).range(0..=10000))]
        expected: Option<u32>,
        /// Use a configured server instead of the local repository.
        #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "origin")]
        remote: Option<String>,
    },
    /// List tasks.
    List {
        /// Filter by state: open, in_progress, done, cancelled.
        #[arg(long)]
        state: Option<String>,
    },
    /// Declare that TASK cannot start until ON is done.
    Depend {
        /// The dependent task.
        task: String,
        /// The task it waits for.
        on: String,
    },
    /// Open tasks whose dependencies are all done.
    Ready,
    /// Mark a task done.
    Done {
        /// Task id.
        task: String,
    },
    /// Cancel a task.
    Cancel {
        /// Task id.
        task: String,
    },
}

fn task_rows(rows: &[ag_storage::meta::TaskRow]) -> String {
    let mut s = String::new();
    for t in rows {
        let _ = writeln!(
            s,
            "{}  {:<11} {}  owner {}  scope [{}]",
            t.id,
            t.state,
            t.title,
            t.owner.clone().unwrap_or_else(|| "-".into()),
            t.scope.join(", ")
        );
        if t.max_attempts > 0 {
            let _ = writeln!(s, "  attempts: {} of {}", t.attempts_started, t.max_attempts);
        }
        if let Some(source) = &t.source_task {
            let run = t.source_run.as_deref().map(|id| format!(" · run {id}")).unwrap_or_default();
            let _ = writeln!(s, "  from {source}{run}");
        }
    }
    s
}

pub fn task(ctx: &Ctx, cmd: &TaskCmd) -> Result<u8> {
    let meta = ctx.repo.meta();
    match cmd {
        TaskCmd::Create { title, description, scope, source_task, source_run, max_attempts } => {
            let (_, t) = tasks::create_task_with(meta, title, description, scope, &[], ctx.me.as_str(), |tx, task| {
                if *max_attempts > 0 {
                    tasks::set_attempt_limit_in(tx, &task.id, *max_attempts, 0, ctx.me.as_str(), false)?;
                }
                tasks::attach_source(
                    tx,
                    &task.id,
                    source_task.as_deref().unwrap_or(""),
                    source_run.as_deref().unwrap_or(""),
                    ctx.me.as_str(),
                )?;
                tx.task(&task.id)?.ok_or_else(|| ag_storage::meta::MetaError::NotFound("created task missing".into()))
            })?;
            ctx.created(&t.id, &t, || format!("Created task {}", t.id))?;
        }
        TaskCmd::Attempts { task, max, expected, remote } => {
            if let Some(remote) = remote {
                let row = super::remote::task_attempts(ctx, remote, task, max.zip(*expected))?;
                ctx.out(&row, || {
                    format!(
                        "Task {}: {} attempt(s) started; limit {} (0 = unset)",
                        row.id, row.attempts_started, row.max_attempts
                    )
                })?;
            } else {
                let row = meta.write(|tx| {
                    if let (Some(max), Some(expected)) = (max, expected) {
                        tasks::set_attempt_limit_in(tx, task, *max, *expected, ctx.me.as_str(), true)
                    } else {
                        tx.task(task)?.ok_or_else(|| ag_storage::meta::MetaError::NotFound(format!("task '{task}'")))
                    }
                })?;
                ctx.out(&row, || {
                    format!(
                        "Task {}: {} attempt(s) started; limit {} (0 = unset)",
                        row.id, row.attempts_started, row.max_attempts
                    )
                })?;
            }
        }
        TaskCmd::Edit { task, changes, remote } => {
            if let Some(remote) = remote {
                let updated = super::remote::update_task(ctx, remote, task, changes)?;
                ctx.done(&updated, || format!("Updated task {}: {}", updated.id, updated.title))?;
            } else {
                let current = meta.read(|tx| tx.task(task))?.ok_or_else(|| anyhow!("task '{task}' not found"))?;
                let expected = tasks::TaskContent::from(&current);
                let content = changes.apply(&expected);
                let updated =
                    meta.write(|tx| tasks::update_in(tx, task, &content, &expected, ctx.me.as_str(), false))?;
                ctx.done(&updated, || format!("Updated task {}: {}", updated.id, updated.title))?;
            }
        }
        TaskCmd::List { state } => {
            let rows = meta.read(|tx| tx.tasks(state.as_deref()))?;
            if rows.is_empty() {
                ctx.note("no tasks match; `ag task create TITLE` adds one");
            }
            ctx.out(&rows, || task_rows(&rows))?;
        }
        TaskCmd::Depend { task, on } => {
            tasks::add_dependency(meta, task, on, ctx.me.as_str())?;
            ctx.done(&json!({"task": task, "depends_on": on}), || format!("{task} now waits for {on}"))?;
        }
        TaskCmd::Ready => {
            let rows = tasks::ready_tasks(meta)?;
            ctx.out(&rows, || task_rows(&rows))?;
        }
        TaskCmd::Done { task } => {
            tasks::finish_task(meta, task, "done", ctx.me.as_str())?;
            ctx.done(&json!({"task": task, "state": "done"}), || format!("{task} done"))?;
        }
        TaskCmd::Cancel { task } => {
            tasks::finish_task(meta, task, "cancelled", ctx.me.as_str())?;
            ctx.done(&json!({"task": task, "state": "cancelled"}), || format!("{task} cancelled"))?;
        }
    }
    Ok(exit::OK)
}

// ---- leases ----------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum LeaseCmd {
    /// Claim a scope (path glob). Soft by default; `--exclusive` is enforced at integration.
    Acquire {
        /// Path glob, e.g. `src/auth/**`.
        scope: String,
        /// Enforce with a fencing token at integration time.
        #[arg(long)]
        exclusive: bool,
        /// How long the lease lasts.
        #[arg(long, default_value = "1h")]
        ttl: String,
        /// Task the lease belongs to.
        #[arg(long)]
        task: Option<String>,
    },
    /// Release a lease early.
    Release {
        /// Lease id.
        id: String,
    },
    /// Extend a lease; the fencing token stays the same.
    Extend {
        /// Lease id.
        id: String,
        /// New duration from now.
        #[arg(long, default_value = "1h")]
        ttl: String,
    },
    /// List active leases.
    List {
        /// Include released and expired leases (the newest ones, see -n).
        #[arg(long)]
        all: bool,
        /// With --all: show at most this many (0 = all).
        #[arg(short = 'n', long, default_value_t = 200, requires = "all")]
        limit: usize,
    },
}

pub fn lease(ctx: &Ctx, cmd: &LeaseCmd) -> Result<u8> {
    let meta = ctx.repo.meta();
    match cmd {
        LeaseCmd::Acquire { scope, exclusive, ttl, task } => {
            let got =
                leases::acquire(meta, scope, ctx.me.as_str(), *exclusive, parse_duration_ms(ttl)?, task.as_deref())?;
            if ctx.quiet && !ctx.json {
                for o in &got.overlaps {
                    ui::eprint_line(&format!("warning: overlaps {} on '{}' held by {}", o.id, o.scope, o.holder));
                }
            }
            // With --quiet: just the fencing token, so `TOKEN=$(ag -q lease acquire 'src/**' --exclusive)` works.
            ctx.created(&got.lease.token.to_string(), &got, || {
                let mut s = format!(
                    "{} lease {} on '{}' until {}\nfencing token: {}   (pass `--fencing-token {}` to `ag integrate`)",
                    if got.lease.exclusive { "Exclusive" } else { "Soft" },
                    got.lease.id,
                    got.lease.scope,
                    got.lease.expires_at,
                    got.lease.token,
                    got.lease.token
                );
                for o in &got.overlaps {
                    let _ = write!(s, "\nwarning: overlaps {} on '{}' held by {}", o.id, o.scope, o.holder);
                }
                s
            })?;
        }
        LeaseCmd::Release { id } => {
            leases::release(meta, id, ctx.me.as_str())?;
            ctx.done(&json!({"released": id}), || format!("Released {id}"))?;
        }
        LeaseCmd::Extend { id, ttl } => {
            let l = leases::extend(meta, id, ctx.me.as_str(), parse_duration_ms(ttl)?)?;
            ctx.done(&l, || format!("{} now expires {} (token {} unchanged)", l.id, l.expires_at, l.token))?;
        }
        LeaseCmd::List { all, limit } => {
            let rows = if *all {
                meta.read(|tx| tx.leases_all(Ctx::cap(*limit)))?
            } else {
                meta.read(|tx| tx.leases_active())?
            };
            if *all {
                ctx.note_cap(rows.len(), *limit, "leases");
            }
            let now = ctx.repo.clock().now();
            if rows.is_empty() {
                ctx.note("no active leases; `ag lease acquire 'GLOB'` claims a scope");
            }
            ctx.out(&rows, || {
                let mut s = String::new();
                for l in &rows {
                    let status = if l.released_at.is_some() {
                        "released"
                    } else if l.is_active(now) {
                        "active"
                    } else {
                        "expired"
                    };
                    let _ = writeln!(
                        s,
                        "{}  {:<8} {:<9} token {:<4} {}  {}  until {}",
                        l.id,
                        status,
                        if l.exclusive { "exclusive" } else { "soft" },
                        l.token,
                        l.scope,
                        l.holder,
                        l.expires_at
                    );
                }
                s
            })?;
        }
    }
    Ok(exit::OK)
}

// ---- provenance --------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum ProvCmd {
    /// Sign and attach a provenance record to a changeset.
    Attach {
        /// Changeset (default: the workspace head).
        rev: Option<String>,
        /// Agent name (its key signs the record).
        #[arg(long)]
        agent: String,
        /// Run id.
        #[arg(long)]
        run: Option<String>,
        /// Task id.
        #[arg(long)]
        task: Option<String>,
        /// Model identifier.
        #[arg(long)]
        model: Option<String>,
        /// File holding the prompt; only its SHA-256 is stored.
        #[arg(long)]
        prompt_file: Option<PathBuf>,
        /// Why the change was made.
        #[arg(long)]
        rationale: Option<String>,
        /// A check the agent ran itself (repeatable).
        #[arg(long = "check")]
        checks: Vec<String>,
        /// A source consulted (repeatable). Stored as untrusted text.
        #[arg(long = "input")]
        inputs: Vec<String>,
    },
    /// Show and verify the provenance of a changeset.
    Show {
        /// Changeset (default: the workspace head).
        rev: Option<String>,
    },
}

pub fn provenance(ctx: &Ctx, cmd: &ProvCmd) -> Result<u8> {
    match cmd {
        ProvCmd::Attach { rev, agent, run, task, model, prompt_file, rationale, checks, inputs } => {
            let id = super::repo::rev_or_default(ctx, rev)?;
            let identity = match run {
                Some(r) => Identity::new(format!("agent:{agent}#{r}")),
                None => Identity::new(format!("agent:{agent}")),
            }
            .map_err(|e| anyhow!("{e}"))?;
            let mut rec = ProvenanceRecord::new(id, identity, ctx.repo.clock().now());
            rec.run_id = run.clone();
            rec.task_id = task.clone();
            rec.model = model.clone();
            rec.rationale = rationale.clone();
            rec.checks_run = checks.clone();
            rec.inputs = inputs.clone();
            if let Some(f) = prompt_file {
                let bytes = std::fs::read(f).with_context(|| format!("reading {}", f.display()))?;
                rec.prompt_digest = Some(hex::encode(Sha256::digest(&bytes)));
            }
            let keys = KeyStore::open(ctx.repo.layout().keys_dir())?;
            let n = provenance::attach(ctx.repo.meta(), &keys, agent, &rec)?;
            ctx.done(&json!({"record": n, "changeset": id}), || {
                format!("Signed provenance record #{n} for {} by {}", id.short(), rec.agent)
            })?;
        }
        ProvCmd::Show { rev } => {
            let id = super::repo::rev_or_default(ctx, rev)?;
            let rows = ctx.repo.meta().read(|tx| tx.provenance_for(&id))?;
            let mut shown = Vec::new();
            for row in &rows {
                let verified = ctx.repo.meta().read(|tx| Ok(provenance::verify(tx, row)))?;
                shown.push(json!({"id": row.id, "agent": row.agent, "verified": verified.is_ok(), "error": verified.as_ref().err().map(|e| e.to_string()), "record": row.record}));
            }
            ctx.out(&shown, || {
                let mut s = String::new();
                for r in &shown {
                    let _ = writeln!(
                        s,
                        "#{} by {}  signature {}",
                        r["id"],
                        r["agent"].as_str().unwrap_or(""),
                        if r["verified"] == true {
                            "VALID".to_string()
                        } else {
                            format!("INVALID ({})", r["error"].as_str().unwrap_or("?"))
                        }
                    );
                    let _ = writeln!(s, "{}", serde_json::to_string_pretty(&r["record"]).unwrap_or_default());
                }
                if s.is_empty() { "No provenance records.".into() } else { s }
            })?;
        }
    }
    Ok(exit::OK)
}

// ---- prediction ----------------------------------------------------------------

#[derive(Args, Debug)]
pub struct PredictArgs {
    #[command(flatten)]
    pub stream: StreamSel,
}

pub fn predict(ctx: &Ctx, a: &PredictArgs) -> Result<u8> {
    let s = a.stream.name(ctx);
    let p = predictor::predict(ctx.repo.meta(), &s)?;
    ctx.out(&p, || {
        if p.overlaps.is_empty() {
            return format!("No overlapping work detected on '{s}'.");
        }
        let mut t = format!(
            "{} overlap(s) on '{s}' (path-level signal; `ag preview` decides real conflicts):\n",
            p.overlaps.len()
        );
        for o in &p.overlaps {
            let _ = writeln!(t, "  {} <-> {}  [{}]  {}", o.a, o.b, o.kind, o.paths.join(", "));
        }
        t
    })?;
    Ok(if p.overlaps.is_empty() { exit::OK } else { exit::BLOCKED })
}
