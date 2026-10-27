//! Repository, workspace and history commands.

use std::fmt::Write as _;
use std::path::PathBuf;

use ag_changeset::Op;
use ag_integrate::workdir::{IgnoreRules, checkout_tree, scan_against};
use ag_integrate::{InitOptions, Repository, StorageConfig, SyncOutcome};
use ag_tree::{DirEntry, Entry, Tree};
use ag_types::{ChangesetId, FileType, Namespace, RepoPath, StreamName};
use anyhow::{Context, Result, anyhow, bail};
use clap::Args;
use serde_json::json;

use crate::cmds::overview;
use crate::ctx::{Ctx, display_path};
use crate::{Cli, exit};

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Repository namespace, e.g. `acme/webapp` (default: `local/<directory name>`).
    #[arg(long)]
    pub namespace: Option<String>,
    /// Name of the first stream.
    #[arg(long, default_value = "main")]
    pub stream: String,
    /// JSON file with the `storage` section (PostgreSQL metadata and/or S3 objects); see
    /// docs/operations/postgres-and-s3.md. Default: a SQLite file and local objects under `.ag/`.
    #[arg(long, value_name = "FILE")]
    pub storage: Option<PathBuf>,
    /// Store a ready-made policy for the first stream: solo, team, agents or strict (`ag policy preset list`).
    #[arg(long, value_name = "NAME")]
    pub preset: Option<String>,
    #[command(flatten)]
    pub preset_flags: crate::cmds::policy::PresetFlags,
}

pub fn init(cli: &Cli, args: &InitArgs) -> Result<u8> {
    let root = match &cli.repo {
        Some(p) => p.clone(),
        None => std::env::current_dir()?,
    };
    std::fs::create_dir_all(&root)?;
    let ns = match &args.namespace {
        Some(n) => n.clone(),
        None => {
            let name = root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
            let clean: String = name
                .to_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' })
                .collect();
            format!("local/{}", clean.trim_start_matches(['-', '.', '_']))
        }
    };
    if args.preset.is_none()
        && (!args.preset_flags.admins.is_empty()
            || args.preset_flags.issuer.is_some()
            || args.preset_flags.ci_key.is_some())
    {
        bail!("--admin, --issuer and --ci-key go with --preset NAME (`ag policy preset list` shows the presets)");
    }
    let me = crate::ctx::identity(cli)?;
    // Check the preset before anything is created: a typo must not leave a half-made repository behind.
    if let Some(name) = &args.preset {
        ag_policy::presets::Preset::parse(name)?;
        if args.preset_flags.ci_key.is_none()
            && args.preset_flags.issuer.is_none()
            && ag_policy::presets::Preset::parse(name)?.requires_ci()
        {
            crate::cmds::policy::build_preset(name, &args.preset_flags, &me, None, false)?;
        }
    }
    let storage = match &args.storage {
        Some(file) => Some(
            serde_json::from_slice::<StorageConfig>(
                &std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?,
            )
            .with_context(|| format!("{} is not a valid storage section", file.display()))?,
        ),
        None => None,
    };
    let opts = InitOptions {
        namespace: Namespace::new(ns.clone()).map_err(|e| anyhow!("{e}"))?,
        default_stream: StreamName::new(args.stream.clone()).map_err(|e| anyhow!("{e}"))?,
        actor: me.clone(),
        storage,
    };
    let repo = Repository::init(&root, &opts)?;
    let applied = match &args.preset {
        Some(name) => {
            let policy = crate::cmds::policy::build_preset(name, &args.preset_flags, &me, Some(&repo), true)?;
            Some((name.clone(), repo.set_policy(&args.stream, &policy.to_json(), &me)?.version))
        }
        None => None,
    };
    if !(cli.quiet && !cli.json) {
        crate::ctx::emit(
            cli.json,
            &json!({"root": root, "namespace": ns, "stream": args.stream, "durability": repo.objects().durability().name,
                    "preset": applied.as_ref().map(|(n, _)| n), "policy_version": applied.as_ref().map(|(_, v)| v)}),
            || {
                let preset = match &applied {
                    Some((n, v)) => {
                        format!("\n  policy:    preset '{n}' (version {v}); `ag policy explain` says what it means")
                    }
                    None => String::new(),
                };
                format!(
                    "Initialized AgentGit repository in {}\n  namespace: {ns}\n  stream:    {}\n  storage:   {} (see `docs/operations/durability.md`){preset}\nNext: create files, then `ag snapshot -m \"first\"`, `ag propose -t \"first\"`, `ag integrate <proposal>`.",
                    root.join(".ag").display(),
                    args.stream,
                    repo.objects().durability().name
                )
            },
        )?;
    }
    Ok(exit::OK)
}

fn workspace_dir(ctx: &Ctx, workspace: &str, dir: &Option<PathBuf>) -> Result<PathBuf> {
    match (dir, workspace) {
        (Some(d), _) => Ok(d.clone()),
        (None, "default") => Ok(ctx.root.clone()),
        (None, w) => ctx
            .repo
            .workspace_dir(w)?
            .ok_or_else(|| anyhow!("workspace '{w}' has no recorded directory; pass --dir <directory>")),
    }
}

#[derive(clap::Subcommand, Debug)]
pub enum WorkspaceCmd {
    /// Create a workspace starting at the stream head and check it out into DIR.
    Create {
        /// Workspace name.
        name: String,
        /// Directory to check out into (created if missing; must be empty).
        #[arg(long)]
        dir: PathBuf,
        /// Stream to track (default: the default stream).
        #[arg(long)]
        stream: Option<String>,
    },
    /// List workspaces.
    List,
    /// Forget a workspace (its directory is left alone).
    Remove {
        /// Workspace name.
        name: String,
    },
}

pub fn workspace(ctx: &Ctx, cmd: &WorkspaceCmd) -> Result<u8> {
    match cmd {
        WorkspaceCmd::Create { name, dir, stream } => {
            let stream =
                StreamName::new(stream.clone().unwrap_or_else(|| ctx.default_stream())).map_err(|e| anyhow!("{e}"))?;
            std::fs::create_dir_all(dir)?;
            if std::fs::read_dir(dir)?.next().is_some() {
                bail!("{} is not empty", display_path(dir));
            }
            let ws = ctx.repo.workspace_create(name, &stream, &ctx.me)?;
            ctx.repo.workspace_checkout(name, dir)?;
            ctx.repo.workspace_set_dir(name, dir)?;
            if let Ok(w) = ctx.repo.workspace_row(name)
                && let Ok(tree) = ctx.repo.tree_or_empty(w.head.as_ref())
            {
                crate::ctx::note_adaptations(&tree);
            }
            ctx.done(&ws, || format!("Created workspace '{name}' on '{stream}' in {}", display_path(dir)))?;
        }
        WorkspaceCmd::List => {
            let rows = ctx.repo.workspaces()?;
            ctx.out(&rows, || {
                let mut s = String::new();
                for w in &rows {
                    let dir = if w.name == "default" {
                        Some(ctx.root.clone())
                    } else {
                        ctx.repo.workspace_dir(&w.name).ok().flatten()
                    };
                    let _ = writeln!(
                        s,
                        "{:<16} stream {:<12} head {}  {}",
                        w.name,
                        w.stream,
                        w.head.map_or("-".into(), |h| h.short()),
                        dir.map(|d| d.display().to_string()).unwrap_or_default()
                    );
                }
                s
            })?;
        }
        WorkspaceCmd::Remove { name } => {
            ctx.repo.workspace_remove(name, &ctx.me)?;
            ctx.done(&json!({"removed": name}), || format!("Removed workspace '{name}'"))?;
        }
    }
    Ok(exit::OK)
}

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Workspace name.
    #[arg(short, long, default_value = "default")]
    pub workspace: String,
    /// Working directory of the workspace (required for non-default workspaces).
    #[arg(long)]
    pub dir: Option<PathBuf>,
    /// Also ask each remote's server for its current head (needs the network and a credential).
    #[arg(long)]
    pub remote: bool,
}

pub fn status(cli: &Cli, ctx: &Ctx, a: &StatusArgs) -> Result<u8> {
    let dir = workspace_dir(ctx, &a.workspace, &a.dir)?;
    let ws = ctx.repo.workspace_row(&a.workspace)?;
    let changes = ctx.repo.workspace_status(&a.workspace, &dir)?;
    let merging = ctx.repo.workspace_is_merging(&a.workspace)?;
    let (_, source) = crate::ctx::identity_with_source(cli)?;
    let summary = overview::summarize(ctx, &ws, source, changes.len(), merging, a.remote)?;
    ctx.out(
        &json!({
            "workspace": ws.name, "stream": ws.stream, "base": ws.base, "head": ws.head, "merging": merging, "changes": changes,
            "summary": summary,
        }),
        || {
            let mut s = format!("workspace '{}' on stream '{}'\n", ws.name, ws.stream);
            let _ = writeln!(s, "  base: {}", ws.base.map_or("(none)".into(), |b| b.short()));
            let _ = writeln!(s, "  head: {}", ws.head.map_or("(no snapshots yet)".into(), |b| b.short()));
            if merging {
                s.push_str("  a conflicted sync is waiting: resolve the marked files, then `ag snapshot`\n");
            }
            if changes.is_empty() {
                s.push_str("nothing to snapshot, working directory clean\n");
            } else {
                for c in &changes {
                    let _ = writeln!(s, "  {:<9} {}", c.kind, c.path);
                }
            }
            s.push('\n');
            s.push_str(&overview::render(&summary, &ws.stream));
            s
        },
    )?;
    Ok(exit::OK)
}

#[derive(Args, Debug)]
pub struct SnapshotArgs {
    /// Snapshot message.
    #[arg(short, long)]
    pub message: String,
    /// Workspace name.
    #[arg(short, long, default_value = "default")]
    pub workspace: String,
    /// Working directory of the workspace.
    #[arg(long)]
    pub dir: Option<PathBuf>,
    /// Snapshot even if conflict markers remain.
    #[arg(long)]
    pub force: bool,
}

pub fn snapshot(ctx: &Ctx, a: &SnapshotArgs) -> Result<u8> {
    let dir = workspace_dir(ctx, &a.workspace, &a.dir)?;
    match ctx.repo.snapshot(&a.workspace, &dir, &a.message, &ctx.me, a.force)? {
        Some(id) => ctx.created(&id.to_string(), &json!({"changeset": id}), || {
            format!("Snapshot {} in workspace '{}'", id.short(), a.workspace)
        })?,
        None => ctx.done(&json!({"changeset": null}), || "Nothing to snapshot.".to_string())?,
    }
    Ok(exit::OK)
}

#[derive(Args, Debug)]
pub struct SyncArgs {
    /// Workspace name.
    #[arg(short, long, default_value = "default")]
    pub workspace: String,
    /// Working directory of the workspace.
    #[arg(long)]
    pub dir: Option<PathBuf>,
}

pub fn sync(ctx: &Ctx, a: &SyncArgs) -> Result<u8> {
    let dir = workspace_dir(ctx, &a.workspace, &a.dir)?;
    let out = ctx.repo.sync_workspace(&a.workspace, &dir, &ctx.me)?;
    if !matches!(out, SyncOutcome::UpToDate)
        && let Ok(w) = ctx.repo.workspace_row(&a.workspace)
        && let Ok(tree) = ctx.repo.tree_or_empty(w.head.as_ref())
    {
        crate::ctx::note_adaptations(&tree);
    }
    let text = || match &out {
        SyncOutcome::UpToDate => "Already up to date.".into(),
        SyncOutcome::FastForwarded { to } => format!("Fast-forwarded to {}.", to.short()),
        SyncOutcome::Merged { id } => format!("Merged the stream into the workspace: {}.", id.short()),
        SyncOutcome::Conflicted { conflicts } => {
            let mut s = format!(
                "{} conflict(s). Resolve the files below, then run `ag snapshot -m \"resolve\"`:\n",
                conflicts.len()
            );
            for c in conflicts {
                let _ = writeln!(s, "  {}  ({})", c.path, c.describe());
            }
            s
        }
    };
    if matches!(out, SyncOutcome::Conflicted { .. }) {
        ctx.out(&out, text)?;
        Ok(exit::REFUSED)
    } else {
        ctx.done(&out, text)?;
        Ok(exit::OK)
    }
}

/// Resolves an optional revision, defaulting to the workspace head, then the stream.
pub fn rev_or_default(ctx: &Ctx, rev: &Option<String>) -> Result<ChangesetId> {
    match rev {
        Some(r) => Ok(ctx.repo.resolve_rev(r)?),
        // Only "there is nothing there" means "no history yet". A metadata database that is locked, corrupt or
        // unreadable is that problem, and says so.
        None => {
            use ag_integrate::Error as E;
            let no_history = || anyhow!("the repository has no history yet; create files and run `ag snapshot -m ...`");
            match ctx.repo.resolve_rev("@") {
                Ok(id) => Ok(id),
                Err(E::NotFound(_)) => match ctx.repo.resolve_rev(ctx.repo.default_stream().as_str()) {
                    Ok(id) => Ok(id),
                    Err(E::NotFound(_)) => Err(no_history()),
                    Err(e) => Err(e.into()),
                },
                Err(e) => Err(e.into()),
            }
        }
    }
}

#[derive(Args, Debug)]
pub struct LogArgs {
    /// Revision to start from (default: the workspace head).
    pub rev: Option<String>,
    /// Maximum number of entries.
    #[arg(short = 'n', long, default_value_t = 20)]
    pub limit: usize,
    /// Only changesets that touched this path.
    #[arg(long)]
    pub path: Option<String>,
}

pub fn log(ctx: &Ctx, a: &LogArgs) -> Result<u8> {
    let head = rev_or_default(ctx, &a.rev)?;
    let entries = ctx.repo.log(&head, a.limit, a.path.as_deref())?;
    ctx.out(&entries, || {
        let mut s = String::new();
        for e in &entries {
            let first = e.message.lines().next().unwrap_or("");
            let merge = if e.parents.len() > 1 {
                format!("  (merge of {})", e.parents.iter().map(|p| p.short()).collect::<Vec<_>>().join(" + "))
            } else {
                String::new()
            };
            let _ = writeln!(s, "{}  {}  {}{merge}", e.id.short(), e.timestamp, e.author);
            let _ = writeln!(s, "    {first}");
        }
        s
    })?;
    Ok(exit::OK)
}

fn op_marker(parent: &Tree, op: &Op) -> &'static str {
    match op {
        Op::Delete { .. } => "D",
        Op::Put { path, .. } => {
            if parent.get(path).is_some() {
                "M"
            } else {
                "A"
            }
        }
    }
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// Revision (default: the workspace head).
    pub rev: Option<String>,
    /// Also report files created as copies of another file (`C` lines): identical content, or at least 50% of the
    /// bytes in common with another file (co-modified and neighbouring files are tried first).
    #[arg(long)]
    pub copies: bool,
}

pub fn show(ctx: &Ctx, a: &ShowArgs) -> Result<u8> {
    let id = rev_or_default(ctx, &a.rev)?;
    let cs = ctx.repo.changeset(&id)?;
    let parent = ctx.repo.tree_or_empty(cs.first_parent().as_ref())?;
    let published = ctx.repo.meta().read(|tx| tx.is_ready(&id))?;
    let prov = ctx.repo.meta().read(|tx| tx.provenance_for(&id))?;
    let renames = ctx.repo.changeset_renames(&cs)?;
    let renamed_to: std::collections::BTreeMap<_, _> = renames.iter().map(|r| (r.to.clone(), r)).collect();
    let renamed_from: std::collections::BTreeSet<_> = renames.iter().map(|r| r.from.clone()).collect();
    let copies = if a.copies { ctx.repo.changeset_copies(&cs)? } else { Vec::new() };
    let copied_to: std::collections::BTreeMap<_, _> = copies.iter().map(|r| (r.to.clone(), r)).collect();
    let changes: Vec<_> = cs
        .ops
        .iter()
        .filter(|op| !renamed_from.contains(op.path()))
        .map(|op| match (renamed_to.get(op.path()), copied_to.get(op.path())) {
            (Some(r), _) => json!({"op": "R", "path": op.path(), "from": r.from, "similarity": r.similarity}),
            (None, Some(r)) => json!({"op": "C", "path": op.path(), "from": r.from, "similarity": r.similarity}),
            (None, None) => json!({"op": op_marker(&parent, op), "path": op.path()}),
        })
        .collect();
    ctx.out(
        &json!({"id": id, "parents": cs.parents, "author": cs.author, "timestamp": cs.timestamp, "message": cs.message, "published": published, "changes": changes, "provenance_records": prov.len()}),
        || {
            let mut s = format!("changeset {id}\n");
            for p in &cs.parents {
                let _ = writeln!(s, "parent    {p}");
            }
            let _ = writeln!(s, "author    {}", cs.author);
            let _ = writeln!(s, "date      {}", cs.timestamp);
            let _ = writeln!(s, "published {}", if published { "yes" } else { "no (snapshot only)" });
            if !prov.is_empty() {
                let _ = writeln!(s, "provenance {} signed record(s); see `ag provenance show`", prov.len());
            }
            s.push('\n');
            for l in cs.message.lines() {
                let _ = writeln!(s, "    {l}");
            }
            s.push('\n');
            for op in cs.ops.iter().filter(|op| !renamed_from.contains(op.path())) {
                match (renamed_to.get(op.path()), copied_to.get(op.path())) {
                    (Some(r), _) => {
                        let _ = writeln!(s, "  R {} -> {}  ({}% similar)", r.from, r.to, r.similarity);
                    }
                    (None, Some(r)) => {
                        let _ = writeln!(s, "  C {} -> {}  ({}% similar)", r.from, r.to, r.similarity);
                    }
                    (None, None) => {
                        let _ = writeln!(s, "  {} {}", op_marker(&parent, op), op.path());
                    }
                }
            }
            s
        },
    )?;
    Ok(exit::OK)
}

#[derive(Args, Debug)]
pub struct DiffArgs {
    /// With two revisions: diff A to B. With one: diff it against its first parent.
    /// With none: diff the workspace head against the working directory.
    pub revs: Vec<String>,
    /// Lines of context.
    #[arg(short = 'U', long, default_value_t = 3)]
    pub context: usize,
    /// Workspace (for the no-argument form).
    #[arg(short, long, default_value = "default")]
    pub workspace: String,
    /// Workspace directory (for the no-argument form).
    #[arg(long)]
    pub dir: Option<PathBuf>,
}

fn entry_bytes(repo: &Repository, e: &Entry) -> Result<Vec<u8>> {
    Ok(repo.blob(&e.content)?)
}

fn render_diff(
    repo: &Repository,
    old: &Tree,
    new: &Tree,
    ctx_lines: usize,
    read_new: &dyn Fn(&RepoPath, &Entry) -> Result<Vec<u8>>,
) -> Result<String> {
    let mut out = String::new();
    for op in old.diff(new) {
        match &op {
            Op::Delete { path } => {
                let e = old.get(path).expect("deleted path existed");
                let d =
                    ag_diff::unified_diff(&format!("a/{path}"), "/dev/null", &entry_bytes(repo, e)?, b"", ctx_lines);
                let _ = write!(out, "diff --ag {path}\ndeleted file\n{d}");
            }
            Op::Put { path, .. } => {
                let ne = new.get(path).expect("put path exists");
                let nb = read_new(path, ne)?;
                match old.get(path) {
                    None => {
                        let d = ag_diff::unified_diff("/dev/null", &format!("b/{path}"), b"", &nb, ctx_lines);
                        let _ = write!(
                            out,
                            "diff --ag {path}\nnew file{}\n{d}",
                            if ne.executable { " (executable)" } else { "" }
                        );
                    }
                    Some(oe) => {
                        let ob = entry_bytes(repo, oe)?;
                        let _ = writeln!(out, "diff --ag {path}");
                        if oe.executable != ne.executable {
                            let _ = writeln!(out, "mode change: executable {} -> {}", oe.executable, ne.executable);
                        }
                        if oe.file_type != ne.file_type {
                            let _ = writeln!(out, "type change: {} -> {}", oe.file_type, ne.file_type);
                        }
                        let d = ag_diff::unified_diff(&format!("a/{path}"), &format!("b/{path}"), &ob, &nb, ctx_lines);
                        out.push_str(&d);
                    }
                }
            }
        }
    }
    Ok(out)
}

pub fn diff(ctx: &Ctx, a: &DiffArgs) -> Result<u8> {
    let repo = &ctx.repo;
    let text = match a.revs.as_slice() {
        [] => {
            let dir = workspace_dir(ctx, &a.workspace, &a.dir)?;
            let ws = repo.workspace_row(&a.workspace)?;
            let old = repo.tree_or_empty(ws.head.as_ref())?;
            let (new, _) = scan_against(repo, &dir, &IgnoreRules::load(&dir)?, false, &old)?;
            render_diff(repo, &old, &new, a.context, &|p, e| {
                let abs = dir.join(p.as_str());
                // Where symlinks cannot be created they are plain files holding the target: read those as files.
                let is_link = std::fs::symlink_metadata(&abs).is_ok_and(|m| m.file_type().is_symlink());
                if e.file_type == FileType::Symlink && is_link {
                    Ok(std::fs::read_link(&abs)?.to_string_lossy().into_owned().into_bytes())
                } else {
                    Ok(std::fs::read(&abs).with_context(|| format!("reading {}", abs.display()))?)
                }
            })?
        }
        [one] => {
            let id = repo.resolve_rev(one)?;
            let cs = repo.changeset(&id)?;
            let old = repo.tree_or_empty(cs.first_parent().as_ref())?;
            render_diff(repo, &old, &repo.tree_of(&id)?, a.context, &|_, e| entry_bytes(repo, e))?
        }
        [x, y] => {
            let (ox, oy) = (repo.resolve_rev(x)?, repo.resolve_rev(y)?);
            render_diff(repo, &repo.tree_of(&ox)?, &repo.tree_of(&oy)?, a.context, &|_, e| entry_bytes(repo, e))?
        }
        _ => bail!("diff takes at most two revisions"),
    };
    ctx.out(&json!({"diff": text}), || text.clone())?;
    Ok(exit::OK)
}

#[derive(Args, Debug)]
pub struct CatArgs {
    /// `REV:PATH`, for example `main:src/lib.rs` or `@:README.md`.
    pub spec: String,
}

pub fn cat(ctx: &Ctx, a: &CatArgs) -> Result<u8> {
    let (rev, path) = a.spec.split_once(':').ok_or_else(|| anyhow!("expected REV:PATH, e.g. main:src/lib.rs"))?;
    let id = ctx.repo.resolve_rev(rev)?;
    let path = RepoPath::new(path).map_err(|e| anyhow!("{e}"))?;
    let tree = ctx.repo.tree_of(&id)?;
    let entry = tree.get(&path).ok_or_else(|| anyhow!("'{path}' does not exist at {}", id.short()))?;
    let bytes = ctx.repo.blob(&entry.content)?;
    crate::ui::print_bytes(&bytes);
    Ok(exit::OK)
}

#[derive(Args, Debug)]
pub struct BlameArgs {
    /// `[REV:]PATH`, for example `src/lib.rs`, `main:src/lib.rs` or `@:README.md`. Without a revision, the
    /// workspace head (or the default stream) is used.
    pub spec: String,
    /// Only these lines, `START,END` (1-based, inclusive).
    #[arg(short = 'L', long, value_name = "START,END")]
    pub lines: Option<String>,
    /// Visit at most this many changesets; older lines are marked as a boundary.
    #[arg(long, default_value_t = ag_integrate::DEFAULT_BLAME_COMMITS)]
    pub max_commits: usize,
    /// Follow files that were created as copies of another file: lines a copy took over are blamed on whoever
    /// wrote them in the source (identical content, or at least 50% of the bytes in common with a neighbouring or
    /// co-modified file).
    #[arg(long)]
    pub copies: bool,
}

pub fn blame(ctx: &Ctx, a: &BlameArgs) -> Result<u8> {
    let (rev, path) = match a.spec.split_once(':') {
        Some((r, p)) => (Some(r.to_string()), p.to_string()),
        None => (None, a.spec.clone()),
    };
    let id = rev_or_default(ctx, &rev)?;
    let range = match &a.lines {
        None => None,
        Some(l) => {
            let (s, e) = l.split_once(',').ok_or_else(|| anyhow!("--lines takes START,END, e.g. 10,20"))?;
            let (s, e): (usize, usize) = (
                s.trim().parse().map_err(|_| anyhow!("bad line number '{s}'"))?,
                e.trim().parse().map_err(|_| anyhow!("bad line number '{e}'"))?,
            );
            Some((s, e))
        }
    };
    let b = ctx.repo.blame(
        &id,
        &path,
        &ag_integrate::BlameOptions { max_commits: a.max_commits, range, copies: a.copies },
    )?;
    ctx.out(&b, || {
        let width = b.lines.last().map(|l| l.line.to_string().len()).unwrap_or(1);
        let author_w = b.lines.iter().map(|l| l.author.len()).max().unwrap_or(0).min(28);
        let names_differ = b.lines.iter().any(|l| l.path != b.path);
        let mut s = String::new();
        for l in &b.lines {
            let mark = if l.boundary { "^" } else { " " };
            let file = if names_differ { format!(" {}", l.path) } else { String::new() };
            let _ = writeln!(
                s,
                "{mark}{} {:<author_w$} {}{file} {:>width$}) {}",
                l.changeset.short(),
                l.author,
                l.timestamp,
                l.line,
                l.text
            );
        }
        if b.truncated {
            let _ = writeln!(s, "(stopped after {} changesets; lines marked ^ may be older)", b.visited);
        }
        s
    })?;
    Ok(exit::OK)
}

#[derive(Args, Debug)]
pub struct LsArgs {
    /// Revision (default: the workspace head).
    pub rev: Option<String>,
    /// Directory to list (default: the root).
    pub path: Option<String>,
}

pub fn ls(ctx: &Ctx, a: &LsArgs) -> Result<u8> {
    let id = rev_or_default(ctx, &a.rev)?;
    let tree = ctx.repo.tree_of(&id)?;
    let dir = a.path.clone().unwrap_or_default();
    let entries = tree.list_dir(dir.trim_matches('/'));
    let rows: Vec<_> = entries
        .iter()
        .map(|e| match e {
            DirEntry::Dir { name } => json!({"name": name, "type": "dir"}),
            DirEntry::File { name, entry } => json!({"name": name, "type": entry.file_type.to_string(), "executable": entry.executable, "blob": entry.content}),
        })
        .collect();
    ctx.out(&rows, || {
        let mut s = String::new();
        for e in &entries {
            match e {
                DirEntry::Dir { name } => {
                    let _ = writeln!(s, "dir      {name}/");
                }
                DirEntry::File { name, entry } => {
                    let _ = writeln!(
                        s,
                        "{:<8} {name}{}",
                        entry.file_type.to_string(),
                        if entry.executable { "*" } else { "" }
                    );
                }
            }
        }
        s
    })?;
    Ok(exit::OK)
}

#[derive(Args, Debug)]
pub struct CheckoutArgs {
    /// Revision to write out.
    pub rev: String,
    /// Destination directory (created if missing; must be empty).
    pub dir: PathBuf,
}

pub fn checkout(ctx: &Ctx, a: &CheckoutArgs) -> Result<u8> {
    let id = ctx.repo.resolve_rev(&a.rev)?;
    std::fs::create_dir_all(&a.dir)?;
    if std::fs::read_dir(&a.dir)?.next().is_some() {
        bail!("{} is not empty", display_path(&a.dir));
    }
    let tree = ctx.repo.tree_of(&id)?;
    checkout_tree(&ctx.repo, &tree, &a.dir)?;
    crate::ctx::note_adaptations(&tree);
    ctx.done(&json!({"changeset": id, "dir": a.dir, "files": tree.len()}), || {
        format!("Wrote {} file(s) of {} to {}", tree.len(), id.short(), display_path(&a.dir))
    })?;
    Ok(exit::OK)
}

#[derive(Args, Debug)]
pub struct PortabilityArgs {
    /// Revision to inspect (default: the workspace head).
    #[arg(default_value = "@")]
    pub rev: String,
    /// File system to check against: `windows`, `macos` or `linux` (default: this machine).
    #[arg(long, value_name = "PLATFORM")]
    pub target: Option<String>,
}

/// Lists what a revision contains that a platform's file system cannot hold: Windows device names and forbidden
/// characters, names that differ only in case on Windows and macOS. Exit status 4 if there is anything.
pub fn portability(ctx: &Ctx, a: &PortabilityArgs) -> Result<u8> {
    use ag_integrate::workdir::{Platform, adaptations, inspect_tree};
    let platform = match &a.target {
        Some(t) => Platform::named(t).ok_or_else(|| anyhow!("unknown platform '{t}' (use windows, macos or linux)"))?,
        None => Platform::host(),
    };
    let id = ctx.repo.resolve_rev(&a.rev)?;
    let tree = ctx.repo.tree_of(&id)?;
    let findings = inspect_tree(&tree, &platform);
    let adapted = adaptations(&tree, &platform);
    let problems = findings.describe(usize::MAX);
    let ok = findings.is_clean();
    ctx.out(
        &json!({
            "changeset": id,
            "platform": platform.name(),
            "paths": tree.len(),
            "ok": ok,
            "problems": problems,
            "hazards": findings.hazards.iter().map(|h| json!({"path": h.path, "component": h.component, "message": h.to_string()})).collect::<Vec<_>>(),
            "collisions": findings.collisions.iter().map(|c| &c.spellings).collect::<Vec<_>>(),
            "symlinks_as_files": adapted.symlinks_as_files,
            "executables_without_bit": adapted.executables_without_bit,
        }),
        || {
            let mut s = if ok {
                format!("All {} path(s) of {} can be checked out on {}.\n", tree.len(), id.short(), platform.name())
            } else {
                format!(
                    "{} cannot be checked out on {}: {} problem(s)\n",
                    id.short(),
                    platform.name(),
                    problems.len()
                )
            };
            for p in &problems {
                let _ = writeln!(s, "  {p}");
            }
            for line in adapted.describe() {
                let _ = writeln!(s, "note: {line}");
            }
            s
        },
    )?;
    Ok(if ok { exit::OK } else { exit::REFUSED })
}
