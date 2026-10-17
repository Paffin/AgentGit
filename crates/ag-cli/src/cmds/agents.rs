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

use crate::ctx::{Ctx, parse_duration_ms};
use crate::exit;

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
    List,
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
    /// List runs.
    Runs {
        /// Only this agent.
        name: Option<String>,
    },
    /// Abandon runs with no recent heartbeat and release their leases.
    Reap {
        /// Heartbeat timeout.
        #[arg(long, default_value = "10m")]
        timeout: String,
    },
    /// Stop an agent from starting new runs.
    Disable {
        /// Agent name.
        name: String,
    },
    /// Allow a disabled agent to run again.
    Enable {
        /// Agent name.
        name: String,
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
        AgentCmd::List => {
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
            ctx.out(&json!({"run_id": run.run_id, "identity": run.identity}), || {
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
        AgentCmd::Runs { name } => {
            let rows = meta.read(|tx| tx.runs(name.as_deref(), 50))?;
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
        AgentCmd::Reap { timeout } => {
            let reaped = registry::reap_stale_runs(meta, parse_duration_ms(timeout)?, ctx.me.as_str())?;
            ctx.out(&reaped, || {
                format!(
                    "Abandoned {} run(s){}",
                    reaped.len(),
                    reaped.iter().map(|r| format!("\n  {r}")).collect::<String>()
                )
            })?;
        }
        AgentCmd::Disable { name } => {
            meta.write(|tx| {
                tx.agent_set_disabled(name, true)?;
                tx.audit_append(ctx.me.as_str(), "agent.disable", name, &json!({}))?;
                Ok(())
            })?;
            ctx.done(&json!({"disabled": name}), || format!("Disabled {name}"))?;
        }
        AgentCmd::Enable { name } => {
            meta.write(|tx| {
                tx.agent_set_disabled(name, false)?;
                tx.audit_append(ctx.me.as_str(), "agent.enable", name, &json!({}))?;
                Ok(())
            })?;
            ctx.done(&json!({"enabled": name}), || format!("Enabled {name}"))?;
        }
    }
    Ok(exit::OK)
}

// ---- tasks -----------------------------------------------------------------

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

pub fn task(ctx: &Ctx, cmd: &TaskCmd) -> Result<u8> {
    let meta = ctx.repo.meta();
    let print = |rows: &Vec<ag_storage::meta::TaskRow>| {
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
        }
        s
    };
    match cmd {
        TaskCmd::Create { title, description, scope } => {
            let t = tasks::create_task(meta, title, description, scope, ctx.me.as_str())?;
            ctx.created(&t.id, &t, || format!("Created task {}", t.id))?;
        }
        TaskCmd::List { state } => {
            let rows = meta.read(|tx| tx.tasks(state.as_deref()))?;
            if rows.is_empty() {
                ctx.note("no tasks match; `ag task create TITLE` adds one");
            }
            ctx.out(&rows, || print(&rows))?;
        }
        TaskCmd::Depend { task, on } => {
            tasks::add_dependency(meta, task, on, ctx.me.as_str())?;
            ctx.done(&json!({"task": task, "depends_on": on}), || format!("{task} now waits for {on}"))?;
        }
        TaskCmd::Ready => {
            let rows = tasks::ready_tasks(meta)?;
            ctx.out(&rows, || print(&rows))?;
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
        /// Include released and expired leases.
        #[arg(long)]
        all: bool,
    },
}

pub fn lease(ctx: &Ctx, cmd: &LeaseCmd) -> Result<u8> {
    let meta = ctx.repo.meta();
    match cmd {
        LeaseCmd::Acquire { scope, exclusive, ttl, task } => {
            let got =
                leases::acquire(meta, scope, ctx.me.as_str(), *exclusive, parse_duration_ms(ttl)?, task.as_deref())?;
            ctx.out(&got, || {
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
        LeaseCmd::List { all } => {
            let rows = if *all { meta.read(|tx| tx.leases_all(200))? } else { meta.read(|tx| tx.leases_active())? };
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
    /// Stream (default: the default stream).
    pub stream: Option<String>,
}

pub fn predict(ctx: &Ctx, a: &PredictArgs) -> Result<u8> {
    let s = a.stream.clone().unwrap_or_else(|| ctx.default_stream());
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
