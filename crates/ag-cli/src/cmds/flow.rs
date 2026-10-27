//! Streams, proposals, integration, operations and the queue.

use std::fmt::Write as _;

use ag_integrate::explain::{Where, explain_blocked, explain_conflict};
use ag_integrate::{IntegrateRequest, IntegrationResult, Outcome, Strategy};
use ag_storage::meta::IntentState;
use ag_types::StreamName;
use anyhow::{Result, anyhow};
use clap::{Args, Subcommand};
use serde_json::json;

use super::repo::rev_or_default;
use crate::cmds::StreamSel;
use crate::ctx::{Ctx, parse_duration_ms};
use crate::exit;

// ---- streams ---------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum StreamCmd {
    /// List streams and their heads.
    List,
    /// Create a stream, optionally at an existing published revision.
    Create {
        /// Stream name.
        name: String,
        /// Start the stream at this published revision.
        #[arg(long)]
        from: Option<String>,
    },
    /// Show one stream.
    Show {
        /// Stream name.
        name: String,
    },
    /// Show the append-only history of head movements.
    Reflog {
        /// Stream name.
        name: String,
        /// Maximum entries.
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
}

pub fn stream(ctx: &Ctx, cmd: &StreamCmd) -> Result<u8> {
    match cmd {
        StreamCmd::List => {
            let streams = ctx.repo.meta().read(|tx| tx.list_streams())?;
            ctx.out(&streams, || {
                let mut s = String::new();
                for st in &streams {
                    let _ = writeln!(
                        s,
                        "{:<20} {}  (version {})",
                        st.name,
                        st.state.head.map_or("(empty)".into(), |h| h.short()),
                        st.state.version
                    );
                }
                s
            })?;
        }
        StreamCmd::Create { name, from } => {
            let n = StreamName::new(name.clone()).map_err(|e| anyhow!("{e}"))?;
            let from = from.as_ref().map(|r| ctx.repo.resolve_rev(r)).transpose()?;
            if let Some(f) = &from
                && !ctx.repo.meta().read(|tx| tx.is_ready(f))?
            {
                return Err(anyhow!("{} is not published; propose and integrate it first", f.short()));
            }
            let row = ctx.repo.create_stream(&n, from.as_ref(), &ctx.me)?;
            ctx.done(&row, || format!("Created stream '{}'", row.name))?;
        }
        StreamCmd::Show { name } => {
            let row = ctx.repo.meta().read(|tx| tx.stream(name))?.ok_or_else(|| anyhow!("no stream '{name}'"))?;
            let (_, pv) = ctx.repo.policy(name)?;
            ctx.out(&json!({"stream": row, "policy_version": pv}), || {
                format!(
                    "{}\n  head:    {}\n  version: {}\n  policy:  v{pv}\n  created: {} by {}",
                    row.name,
                    row.state.head.map_or("(empty)".into(), |h| h.to_string()),
                    row.state.version,
                    row.created_at,
                    row.created_by
                )
            })?;
        }
        StreamCmd::Reflog { name, limit } => {
            let log = ctx.repo.meta().read(|tx| tx.reflog(name, *limit))?;
            ctx.out(&log, || {
                let mut s = String::new();
                for e in &log {
                    let _ = writeln!(
                        s,
                        "#{:<4} v{:<4} {} -> {}  {}  {}  [{}]",
                        e.seq,
                        e.version,
                        e.old_head.map_or("(none)".into(), |h| h.short()),
                        e.new_head.map_or("(none)".into(), |h| h.short()),
                        e.timestamp,
                        e.actor,
                        e.reason
                    );
                }
                s
            })?;
        }
    }
    Ok(exit::OK)
}

// ---- proposals -------------------------------------------------------------

#[derive(Args, Debug)]
pub struct ProposeArgs {
    /// Revision to propose (default: the workspace head).
    pub rev: Option<String>,
    /// Proposal title.
    #[arg(short, long)]
    pub title: String,
    /// Longer description.
    #[arg(short, long, default_value = "")]
    pub description: String,
    /// Target stream (default: the repository's default stream).
    #[arg(long)]
    pub stream: Option<String>,
    /// Task this proposal belongs to.
    #[arg(long)]
    pub task: Option<String>,
}

pub fn propose(ctx: &Ctx, a: &ProposeArgs) -> Result<u8> {
    let head = rev_or_default(ctx, &a.rev)?;
    let stream = a.stream.clone().unwrap_or_else(|| ctx.default_stream());
    let p = ctx.repo.propose(&stream, &head, &a.title, &a.description, &ctx.me, a.task.as_deref())?;
    ctx.created(&p.id, &p, || {
        format!("Opened proposal {} -> '{stream}' ({}).\nNext: `ag integrate {}`", p.id, head.short(), p.id)
    })?;
    Ok(exit::OK)
}

#[derive(Subcommand, Debug)]
pub enum ProposalCmd {
    /// List proposals, newest first.
    List {
        /// Filter by state: open, integrated, closed.
        #[arg(long)]
        state: Option<String>,
        /// Show at most this many (0 = all).
        #[arg(short = 'n', long, default_value_t = 200)]
        limit: usize,
    },
    /// Show a proposal with its reviews.
    Show {
        /// Proposal id.
        id: String,
    },
    /// Review the proposal's current head.
    Review {
        /// Proposal id.
        id: String,
        /// Approve.
        #[arg(long, group = "decision")]
        approve: bool,
        /// Request changes.
        #[arg(long, group = "decision")]
        request_changes: bool,
        /// Comment only.
        #[arg(long, group = "decision")]
        comment: bool,
        /// Review text.
        #[arg(short, long, default_value = "")]
        message: String,
    },
    /// Point the proposal at a new head (earlier approvals stop counting).
    Update {
        /// Proposal id.
        id: String,
        /// New head (default: the workspace head).
        rev: Option<String>,
    },
    /// Close without integrating.
    Close {
        /// Proposal id.
        id: String,
    },
}

pub fn proposal(ctx: &Ctx, cmd: &ProposalCmd) -> Result<u8> {
    match cmd {
        ProposalCmd::List { state, limit } => {
            let list = ctx.repo.meta().read(|tx| tx.proposals_list(state.as_deref(), Ctx::cap(*limit)))?;
            ctx.note_cap(list.len(), *limit, "proposals");
            if list.is_empty() {
                ctx.note("no proposals match; `ag propose -t TITLE` opens one");
            }
            ctx.out(&list, || {
                let mut s = String::new();
                for p in &list {
                    let _ = writeln!(
                        s,
                        "{}  {:<10} {} -> {}  {}  by {}",
                        p.id,
                        p.state,
                        p.head.short(),
                        p.stream,
                        p.title,
                        p.author
                    );
                }
                s
            })?;
        }
        ProposalCmd::Show { id } => {
            let p = ctx.repo.find_proposal(id)?;
            let reviews = ctx.repo.meta().read(|tx| tx.reviews(&p.id))?;
            ctx.out(&json!({"proposal": p, "reviews": reviews}), || {
                let mut s = format!(
                    "{}  [{}]\n  {}\n  head {} -> stream '{}'\n  author {}\n",
                    p.id, p.state, p.title, p.head, p.stream, p.author
                );
                if !p.description.is_empty() {
                    let _ = writeln!(s, "\n{}\n", p.description);
                }
                for r in &reviews {
                    let current = if r.head == p.head { "" } else { "  (stale: older head)" };
                    let _ = writeln!(
                        s,
                        "  review: {:<16} {} on {}{current}  {}",
                        r.decision,
                        r.reviewer,
                        r.head.short(),
                        r.body
                    );
                }
                s
            })?;
        }
        ProposalCmd::Review { id, approve, request_changes, comment, message } => {
            let decision = match (approve, request_changes, comment) {
                (true, _, _) => "approve",
                (_, true, _) => "request_changes",
                (_, _, true) => "comment",
                _ => return Err(anyhow!("choose one of --approve, --request-changes, --comment")),
            };
            let r = ctx.repo.review(id, &ctx.me, decision, message)?;
            ctx.done(&r, || format!("Recorded '{decision}' by {} on {}", ctx.me, r.head.short()))?;
        }
        ProposalCmd::Update { id, rev } => {
            let head = rev_or_default(ctx, rev)?;
            let p = ctx.repo.update_proposal(id, &head, &ctx.me)?;
            ctx.done(&p, || {
                format!("Proposal {} now points at {} (earlier approvals no longer count)", p.id, p.head.short())
            })?;
        }
        ProposalCmd::Close { id } => {
            ctx.repo.close_proposal(id, &ctx.me)?;
            ctx.done(&json!({"closed": id}), || format!("Closed {id}"))?;
        }
    }
    Ok(exit::OK)
}

// ---- integrate -------------------------------------------------------------

#[derive(Args, Debug)]
pub struct IntegrateArgs {
    /// Proposal id.
    pub proposal: String,
    /// Idempotency key (default: derived from the proposal; repeating resumes/replays). Not the issuer key of
    /// `ag validate run --key`; `--idempotency-key` is the unambiguous spelling.
    #[arg(long, visible_alias = "idempotency-key")]
    pub key: Option<String>,
    /// Fencing token(s) of exclusive leases the author holds.
    #[arg(long = "fencing-token")]
    pub fencing_tokens: Vec<u64>,
}

pub fn print_result(ctx: &Ctx, r: &IntegrationResult, proposal: Option<&str>, confirmation: bool) -> Result<u8> {
    let report = |ctx: &Ctx, text: &dyn Fn() -> String| {
        if confirmation && matches!(r.state, IntentState::Integrated) { ctx.done(r, text) } else { ctx.out(r, text) }
    };
    report(ctx, &|| {
        let mut s = String::new();
        match &r.outcome {
            Outcome::Integrated { new_head, previous_head, version, strategy, changed_files, .. } => {
                let how = match strategy {
                    Strategy::FastForward => "fast-forward",
                    Strategy::Merge => "merge",
                    Strategy::AlreadyIntegrated => "already integrated",
                };
                let _ = write!(s, "Integrated ({how}): stream is now at {} (version {version}", new_head.short());
                if let Some(p) = previous_head {
                    let _ = write!(s, ", was {}", p.short());
                }
                let _ = write!(s, "), {changed_files} file(s) changed.\noperation {}", r.operation_id);
            }
            Outcome::Blocked { candidate_tree, policy_version, blockers, .. } => {
                let _ = writeln!(s, "Blocked (policy v{policy_version}); the stream was not changed:");
                for b in blockers {
                    let _ = writeln!(s, "  - {}", b.message);
                }
                let _ = writeln!(s, "candidate tree: {candidate_tree}");
                if let Some(p) = proposal {
                    let ex: Vec<_> = blockers.iter().map(|b| explain_blocked(b, p, Where::Local)).collect();
                    let _ = writeln!(
                        s,
                        "Provide what is missing, then run `ag integrate {p}` again (it resumes this operation)."
                    );
                    let _ = writeln!(s, "{}", crate::cmds::explain::hints(&ex, p, Where::Local));
                }
                let _ = write!(s, "operation {}", r.operation_id);
            }
            Outcome::Conflict { conflicts, base } => {
                let _ =
                    writeln!(s, "Conflict: the proposal cannot be merged automatically; the stream was not changed.");
                for c in conflicts {
                    let _ = writeln!(s, "  {}  ({})", c.path, c.describe());
                }
                if let Some(b) = base {
                    let _ = writeln!(s, "merge base: {}", b.short());
                }
                match (proposal, conflicts.first()) {
                    (Some(p), Some(c)) => {
                        let e = explain_conflict(c, p, Where::Local);
                        let steps: Vec<String> = e.fixes.iter().map(|f| format!("`{}`", f.command)).collect();
                        let _ = write!(s, "Fix: {}.\n`ag explain {p}` says why, step by step.", steps.join(", "));
                    }
                    _ => {
                        let _ = write!(
                            s,
                            "Fix: `ag sync`, resolve the marked files, `ag snapshot`, `ag proposal update <id>`."
                        );
                    }
                }
            }
            Outcome::Denied { blockers } => {
                let _ = writeln!(s, "Denied by policy; the stream was not changed:");
                for b in blockers {
                    let _ = writeln!(s, "  - {}", b.message);
                }
                if let Some(p) = proposal {
                    let ex: Vec<_> = blockers.iter().map(|b| explain_blocked(b, p, Where::Local)).collect();
                    let _ = write!(s, "What to do:\n{}", crate::cmds::explain::hints(&ex, p, Where::Local));
                }
            }
            Outcome::Invalid { reason } => {
                let _ = write!(s, "Invalid: {reason}");
            }
        }
        s
    })?;
    Ok(match (&r.state, &r.outcome) {
        (IntentState::Integrated, _) => exit::OK,
        (IntentState::Blocked, _) => exit::BLOCKED,
        _ => exit::REFUSED,
    })
}

pub fn integrate(ctx: &Ctx, a: &IntegrateArgs) -> Result<u8> {
    let r = ctx.repo.integrate(&IntegrateRequest {
        proposal_id: a.proposal.clone(),
        actor: ctx.me.clone(),
        idempotency_key: a.key.clone(),
        fencing_tokens: a.fencing_tokens.clone(),
    })?;
    print_result(ctx, &r, Some(&a.proposal), true)
}

#[derive(Args, Debug)]
pub struct PreviewArgs {
    /// Proposal id.
    pub proposal: String,
}

pub fn preview(ctx: &Ctx, a: &PreviewArgs) -> Result<u8> {
    let pv = ctx.repo.merge_preview(&a.proposal)?;
    ctx.out(&pv, || {
        let mut s = format!("strategy: {:?}   (algorithm {})\n", pv.strategy, pv.algorithm);
        if let Some(p) = &pv.problem {
            let _ = writeln!(s, "problem: {p}");
        }
        if let Some(b) = pv.base {
            let _ = writeln!(s, "merge base: {}", b.short());
        }
        if let Some(t) = pv.candidate_tree {
            let _ = writeln!(s, "candidate tree: {t}");
        }
        for c in &pv.conflicts {
            let _ = writeln!(s, "conflict: {} ({})", c.path, c.describe());
        }
        for n in &pv.notes {
            let _ = writeln!(s, "note: {}", n.message);
            for [from, to] in &n.moves {
                let _ = writeln!(s, "  {from} -> {to}");
            }
        }
        if !pv.changed_files.is_empty() {
            let _ = writeln!(s, "would change {} file(s): {}", pv.changed_files.len(), pv.changed_files.join(", "));
        }
        s
    })?;
    Ok(if pv.conflicts.is_empty() && pv.problem.is_none() { exit::OK } else { exit::REFUSED })
}

// ---- operations --------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum OpCmd {
    /// Show an operation's recorded state and result. Use this instead of
    /// blindly retrying when a response was lost.
    Show {
        /// Operation id.
        id: String,
    },
    /// Resume an operation (for example one that was blocked).
    Resume {
        /// Operation id.
        id: String,
        /// Fencing token(s) to present.
        #[arg(long = "fencing-token")]
        fencing_tokens: Vec<u64>,
    },
    /// List recent operations.
    List {
        /// Maximum entries.
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
}

pub fn op(ctx: &Ctx, cmd: &OpCmd) -> Result<u8> {
    match cmd {
        OpCmd::Show { id } => {
            let r = ctx.repo.operation(id)?;
            print_result(ctx, &r, None, false)?;
            Ok(exit::OK)
        }
        OpCmd::Resume { id, fencing_tokens } => {
            let tokens = if fencing_tokens.is_empty() { None } else { Some(fencing_tokens.as_slice()) };
            let r = ctx.repo.resume_integrate(id, &ctx.me, tokens)?;
            print_result(ctx, &r, None, true)
        }
        OpCmd::List { limit } => {
            let list = ctx.repo.meta().read(|tx| tx.intents_recent(*limit))?;
            if list.is_empty() {
                ctx.note("no operations yet; publishing and integrating create them");
            }
            ctx.out(&list, || {
                let mut s = String::new();
                for i in &list {
                    let _ =
                        writeln!(s, "{}  {:<9} {:<10} {}  {}", i.operation_id, i.kind, i.state, i.updated_at, i.actor);
                }
                s
            })?;
            Ok(exit::OK)
        }
    }
}

#[derive(Args, Debug)]
pub struct RecoverArgs {
    /// Operations untouched for longer than this are aborted.
    #[arg(long, default_value = "7d")]
    pub max_age: String,
}

pub fn recover(ctx: &Ctx, a: &RecoverArgs) -> Result<u8> {
    let rep = ctx.repo.recover(parse_duration_ms(&a.max_age)?)?;
    ctx.out(&rep, || {
        format!(
            "resumed {}, aborted {}, still waiting {}, errors {}",
            rep.resumed.len(),
            rep.aborted.len(),
            rep.waiting.len(),
            rep.errors.len()
        )
    })?;
    Ok(if rep.errors.is_empty() { exit::OK } else { exit::ERROR })
}

// ---- queue -----------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum QueueCmd {
    /// Add a proposal to its stream's queue.
    Add {
        /// Proposal id.
        proposal: String,
    },
    /// Show the queue.
    List {
        #[command(flatten)]
        stream: StreamSel,
    },
    /// Integrate queued proposals in order.
    Run {
        #[command(flatten)]
        stream: StreamSel,
    },
}

pub fn queue(ctx: &Ctx, cmd: &QueueCmd) -> Result<u8> {
    match cmd {
        QueueCmd::Add { proposal } => {
            let row = ctx.repo.enqueue(proposal, &ctx.me)?;
            ctx.done(&row, || format!("Queued {proposal} as #{} on '{}'", row.id, row.stream))?;
        }
        QueueCmd::List { stream } => {
            let s = stream.name(ctx);
            let rows = ctx.repo.queue(&s)?;
            if rows.is_empty() {
                ctx.note(format!("the queue of '{s}' is empty; `ag queue add PROPOSAL` adds to it"));
            }
            ctx.out(&rows, || {
                let mut t = String::new();
                for r in &rows {
                    let _ = writeln!(
                        t,
                        "#{:<4} {:<10} {}  {}",
                        r.id,
                        r.state,
                        r.proposal,
                        r.outcome.clone().unwrap_or_default()
                    );
                }
                t
            })?;
        }
        QueueCmd::Run { stream } => {
            let s = stream.name(ctx);
            let rep = ctx.repo.run_queue(&s, &ctx.me)?;
            ctx.out(&rep, || {
                format!(
                    "integrated {}, blocked {}, rejected {}, failed {}",
                    rep.integrated.len(),
                    rep.blocked.len(),
                    rep.rejected.len(),
                    rep.failed.len()
                )
            })?;
            return Ok(if rep.blocked.is_empty() && rep.rejected.is_empty() && rep.failed.is_empty() {
                exit::OK
            } else {
                exit::BLOCKED
            });
        }
    }
    Ok(exit::OK)
}
