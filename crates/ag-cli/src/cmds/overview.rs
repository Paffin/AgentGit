//! The one-screen summary behind `ag status`: workspace, proposals, leases, task and remote sync state.
//!
//! Everything here is read from the local repository (no network) except the optional server head that
//! `ag status --remote` asks for. All of it is advisory: `ag preview` and `ag integrate` decide.

use std::fmt::Write as _;

use ag_storage::meta::{ProposalRow, WorkspaceRow};
use ag_types::ChangesetId;
use anyhow::Result;
use serde::Serialize;

use crate::cmds::remote::{self, Remote};
use crate::ctx::{Ctx, IdentitySource};
use crate::ui::{self, Tone};

/// How the workspace head relates to its stream.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    /// Neither has any changesets.
    Empty,
    /// Same head.
    InSync,
    /// The workspace has snapshots the stream lacks.
    Ahead,
    /// The stream has changesets the workspace lacks (`ag sync`).
    Behind,
    /// Both moved.
    Diverged,
}

/// A proposal, briefly.
#[derive(Clone, Debug, Serialize)]
pub struct ProposalBrief {
    /// Proposal id.
    pub id: String,
    /// Title.
    pub title: String,
    /// Target stream.
    pub stream: String,
    /// Author.
    pub author: String,
    /// Is this the proposal for the workspace head?
    pub is_workspace_head: bool,
}

/// Proposals on the repository.
#[derive(Clone, Debug, Serialize)]
pub struct ProposalsSummary {
    /// Open proposals by anyone.
    pub open: usize,
    /// Open proposals by this identity's principal.
    pub mine: Vec<ProposalBrief>,
}

/// A lease, briefly.
#[derive(Clone, Debug, Serialize)]
pub struct LeaseBrief {
    /// Lease id.
    pub id: String,
    /// Path scope.
    pub scope: String,
    /// Exclusive (enforced) or soft (advisory).
    pub exclusive: bool,
    /// Fencing token.
    pub token: u64,
    /// Expiry.
    pub expires_at: ag_types::Timestamp,
}

/// Active leases.
#[derive(Clone, Debug, Serialize)]
pub struct LeasesSummary {
    /// Active leases held by anyone.
    pub active: usize,
    /// Held by this identity's principal.
    pub mine: Vec<LeaseBrief>,
}

/// The task this identity is working on.
#[derive(Clone, Debug, Serialize)]
pub struct TaskBrief {
    /// Task id.
    pub id: String,
    /// Title.
    pub title: String,
    /// `open`, `in_progress`, ...
    pub state: String,
}

/// One remote and how the local mirror relates to what was last fetched.
#[derive(Clone, Debug, Serialize)]
pub struct RemoteState {
    /// Remote name.
    pub name: String,
    /// Server URL.
    pub url: String,
    /// Repository on the server.
    pub repo: String,
    /// The stream compared.
    pub stream: String,
    /// The head recorded by the last `ag fetch`/`pull`/`clone`.
    pub fetched_head: Option<String>,
    /// `never_fetched`, `up_to_date`, `fetched_not_pulled`, `local_ahead` or `diverged`.
    pub state: &'static str,
    /// Changesets fetched but not yet in the local stream (`fetched_not_pulled`).
    pub behind: usize,
    /// Local stream changesets the server has not been told about (`local_ahead`).
    pub ahead: usize,
    /// The server's head right now (only with `--remote`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_head: Option<String>,
    /// Why the server could not be asked (only with `--remote`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_error: Option<String>,
}

/// Everything `ag status` knows beyond the changed files.
#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    /// The acting identity.
    pub identity: String,
    /// Where it came from.
    pub identity_source: IdentitySource,
    /// The stream's head.
    pub stream_head: Option<ChangesetId>,
    /// The stream's version.
    pub stream_version: u64,
    /// Workspace head against stream head.
    pub relation: Relation,
    /// Snapshots the workspace has that the stream lacks.
    pub ahead: usize,
    /// Of those, snapshots that have never been published (proposed).
    pub unpublished: usize,
    /// Changesets the stream has that the workspace lacks.
    pub behind: usize,
    /// Proposals.
    pub proposals: ProposalsSummary,
    /// Leases.
    pub leases: LeasesSummary,
    /// The task of this identity's run, or the one it owns.
    pub task: Option<TaskBrief>,
    /// Remotes and sync state.
    pub remotes: Vec<RemoteState>,
    /// What to do next, if there is an obvious step.
    pub next: Option<String>,
}

fn principal(identity: &str) -> &str {
    identity.split_once('#').map_or(identity, |(p, _)| p)
}

fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// Collects the summary for `ws` (a workspace row) with `changes` uncommitted changes.
pub fn summarize(
    ctx: &Ctx,
    ws: &WorkspaceRow,
    source: IdentitySource,
    changes: usize,
    merging: bool,
    ask_remote: bool,
) -> Result<Summary> {
    let repo = &ctx.repo;
    let me = ctx.me.as_str();
    let mine = principal(me);
    let stream_row = repo.meta().read(|tx| tx.stream(&ws.stream))?;
    let stream_head = stream_row.as_ref().and_then(|s| s.state.head);
    let stream_version = stream_row.as_ref().map_or(0, |s| s.state.version);

    let ahead_ids = match (ws.head, stream_head) {
        (Some(h), Some(s)) => repo.ancestors_between(&[h], &[s])?,
        (Some(h), None) => repo.ancestors_between(&[h], &[])?,
        _ => Vec::new(),
    };
    let behind = match (ws.head, stream_head) {
        (Some(h), Some(s)) => repo.ancestors_between(&[s], &[h])?.len(),
        (None, Some(s)) => repo.ancestors_between(&[s], &[])?.len(),
        _ => 0,
    };
    let mut unpublished = 0;
    for id in &ahead_ids {
        if !repo.meta().read(|tx| tx.is_ready(id))? {
            unpublished += 1;
        }
    }
    let relation = match (ahead_ids.len(), behind, ws.head.is_none() && stream_head.is_none()) {
        (_, _, true) => Relation::Empty,
        (0, 0, _) => Relation::InSync,
        (_, 0, _) => Relation::Ahead,
        (0, _, _) => Relation::Behind,
        _ => Relation::Diverged,
    };

    let open: Vec<ProposalRow> = repo.meta().read(|tx| tx.proposals_list(Some("open"), 500))?;
    let my_props: Vec<ProposalBrief> = open
        .iter()
        .filter(|p| principal(&p.author) == mine)
        .map(|p| ProposalBrief {
            id: p.id.clone(),
            title: p.title.clone(),
            stream: p.stream.clone(),
            author: p.author.clone(),
            is_workspace_head: Some(p.head) == ws.head,
        })
        .collect();

    let now = repo.clock().now();
    let active: Vec<_> = repo.meta().read(|tx| tx.leases_active())?.into_iter().filter(|l| l.is_active(now)).collect();
    let my_leases: Vec<LeaseBrief> = active
        .iter()
        .filter(|l| principal(&l.holder) == mine)
        .map(|l| LeaseBrief {
            id: l.id.clone(),
            scope: l.scope.clone(),
            exclusive: l.exclusive,
            token: l.token,
            expires_at: l.expires_at,
        })
        .collect();

    // The task: the one the current run was started for, else one this principal owns and is working on.
    let task = if let Some((_, run)) = me.split_once("#") {
        let runs = repo.meta().read(|tx| tx.runs(None, 200))?;
        let task_id = runs.iter().find(|r| r.run_id == run).and_then(|r| r.task_id.clone());
        match task_id {
            Some(id) => repo.meta().read(|tx| tx.tasks(None))?.into_iter().find(|t| t.id == id),
            None => None,
        }
    } else {
        repo.meta()
            .read(|tx| tx.tasks(Some("in_progress")))?
            .into_iter()
            .find(|t| t.owner.as_deref().is_some_and(|o| principal(o) == mine))
    }
    .map(|t| TaskBrief { id: t.id, title: t.title, state: t.state });

    let remotes = remote_states(ctx, &ws.stream, stream_head, ask_remote)?;

    let next = if merging {
        Some("resolve the marked files, then `ag snapshot -m \"resolve\"`".to_string())
    } else if changes > 0 {
        Some("record your changes: `ag snapshot -m \"...\"`".to_string())
    } else if let Some(p) = my_props.iter().find(|p| p.is_workspace_head) {
        Some(format!("`ag preview {id}` to check it, `ag integrate {id}` to land it", id = p.id))
    } else if unpublished > 0 || (relation == Relation::Ahead && !my_props.iter().any(|p| p.is_workspace_head)) {
        Some("publish your snapshots: `ag propose -t \"...\"`".to_string())
    } else if matches!(relation, Relation::Behind | Relation::Diverged) {
        Some("bring in the stream's new head: `ag sync`".to_string())
    } else if remotes.iter().any(|r| r.state == "fetched_not_pulled") {
        Some("`ag pull` applies what was fetched".to_string())
    } else {
        None
    };

    Ok(Summary {
        identity: me.to_string(),
        identity_source: source,
        stream_head,
        stream_version,
        relation,
        ahead: ahead_ids.len(),
        unpublished,
        behind,
        proposals: ProposalsSummary { open: open.len(), mine: my_props },
        leases: LeasesSummary { active: active.len(), mine: my_leases },
        task,
        remotes,
        next,
    })
}

fn remote_states(ctx: &Ctx, stream: &str, local_head: Option<ChangesetId>, ask: bool) -> Result<Vec<RemoteState>> {
    let all = match remote::load_remotes_if_any(&ctx.root) {
        Ok(r) => r,
        Err(e) => {
            ui::verbose(format!("remotes.json is unreadable: {e}"));
            return Ok(Vec::new());
        }
    };
    let mut out = Vec::new();
    for (name, r) in all {
        let key = remote::tracking_key(&name, stream);
        let fetched: Option<ChangesetId> = ctx.repo.meta().read(|tx| tx.meta_get(&key))?.and_then(|s| s.parse().ok());
        let (state, behind, ahead) = match (fetched, local_head) {
            (None, _) => ("never_fetched", 0, 0),
            (Some(f), Some(l)) if f == l => ("up_to_date", 0, 0),
            (Some(f), l) => {
                let have: Vec<ChangesetId> = l.into_iter().collect();
                let new_remote = ctx.repo.ancestors_between(&[f], &have)?.len();
                let new_local = match l {
                    Some(l) => ctx.repo.ancestors_between(&[l], &[f])?.len(),
                    None => 0,
                };
                match (new_remote, new_local) {
                    (0, 0) => ("up_to_date", 0, 0),
                    (n, 0) => ("fetched_not_pulled", n, 0),
                    (0, n) => ("local_ahead", 0, n),
                    (n, m) => ("diverged", n, m),
                }
            }
        };
        let mut rs = RemoteState {
            name: name.clone(),
            url: r.url.clone(),
            repo: r.repo.clone(),
            stream: stream.to_string(),
            fetched_head: fetched.map(|f| f.to_string()),
            state,
            behind,
            ahead,
            server_head: None,
            server_error: None,
        };
        if ask {
            match ask_server_head(ctx, &name, &r, stream) {
                Ok(h) => rs.server_head = Some(h),
                Err(e) => rs.server_error = Some(format!("{e:#}")),
            }
        }
        out.push(rs);
    }
    Ok(out)
}

fn ask_server_head(ctx: &Ctx, name: &str, r: &Remote, stream: &str) -> Result<String> {
    remote::server_head(&ctx.root, name, r, stream)
}

/// The text after the workspace lines of `ag status`.
pub fn render(s: &Summary, stream: &str) -> String {
    let mut t = String::new();
    let head = |h: &Option<ChangesetId>| h.map_or("(empty)".to_string(), |h| h.short());
    let _ = writeln!(
        t,
        "{} {}",
        ui::tone(Tone::Strong, "identity "),
        format_args!("{} ({})", s.identity, s.identity_source.describe())
    );
    let stream_line = match s.relation {
        Relation::Empty => format!("'{stream}' has no changesets yet"),
        Relation::InSync => format!("in sync with '{stream}' ({}, version {})", head(&s.stream_head), s.stream_version),
        Relation::Ahead => {
            format!("{} ahead of '{stream}' ({} not yet proposed)", count(s.ahead, "snapshot"), s.unpublished)
        }
        Relation::Behind => {
            format!("{} behind '{stream}': run `ag sync`", count(s.behind, "changeset"))
        }
        Relation::Diverged => format!(
            "diverged from '{stream}': {} ahead, {} behind; `ag sync` merges",
            count(s.ahead, "snapshot"),
            count(s.behind, "changeset")
        ),
    };
    let _ = writeln!(t, "{} {stream_line}", ui::tone(Tone::Strong, "stream   "));
    let mut p = format!("{} open", s.proposals.open);
    if !s.proposals.mine.is_empty() {
        p.push_str(&format!(", {} yours:", s.proposals.mine.len()));
        for m in s.proposals.mine.iter().take(3) {
            p.push_str(&format!("\n             {}  {}", m.id, m.title));
        }
        if s.proposals.mine.len() > 3 {
            p.push_str(&format!("\n             ... and {} more (`ag proposal list`)", s.proposals.mine.len() - 3));
        }
    }
    let _ = writeln!(t, "{} {p}", ui::tone(Tone::Strong, "proposals"));
    let mut l = format!("{} active", s.leases.active);
    if !s.leases.mine.is_empty() {
        l.push_str(&format!(", {} held by you:", s.leases.mine.len()));
        for m in s.leases.mine.iter().take(3) {
            l.push_str(&format!(
                "\n             {}  {} {}  token {}",
                m.id,
                m.scope,
                if m.exclusive { "exclusive" } else { "soft" },
                m.token
            ));
        }
    }
    let _ = writeln!(t, "{} {l}", ui::tone(Tone::Strong, "leases   "));
    if let Some(task) = &s.task {
        let _ = writeln!(t, "{} {}  {} [{}]", ui::tone(Tone::Strong, "task     "), task.id, task.title, task.state);
    }
    for r in &s.remotes {
        let what = match r.state {
            "never_fetched" => "never fetched: run `ag fetch`".to_string(),
            "up_to_date" => "up to date as of the last fetch".to_string(),
            "fetched_not_pulled" => {
                format!("{} fetched, not yet in the local stream: run `ag pull`", count(r.behind, "changeset"))
            }
            "local_ahead" => format!(
                "the local stream has {} the server has not seen (a clone mirrors the server: propose with `ag push`)",
                count(r.ahead, "changeset")
            ),
            _ => format!("diverged: {} to pull, {} local only", r.behind, r.ahead),
        };
        let _ = write!(t, "{} {} -> {}/{}: {what}", ui::tone(Tone::Strong, "remote   "), r.name, r.url, r.repo);
        match (&r.server_head, &r.server_error) {
            (Some(h), _) => {
                let fresh = r.fetched_head.as_deref() == Some(h.as_str());
                let _ = write!(
                    t,
                    "\n             server is at {}{}",
                    &h[..h.len().min(12)],
                    if fresh { "" } else { " (differs from the last fetch: run `ag fetch`)" }
                );
            }
            (_, Some(e)) => {
                let _ = write!(t, "\n             server not asked: {e}");
            }
            _ => {}
        }
        t.push('\n');
    }
    if let Some(n) = &s.next {
        let _ = writeln!(t, "{} {n}", ui::tone(Tone::Hint, "next:    "));
    }
    t
}
