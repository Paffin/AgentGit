//! Benchmarks of the engine's hot paths by repository size.
//!
//! Everything here calls the real engine (`ag-integrate`, `ag-merge`, `ag-tree`) in-process, over the engine's own stores.
//! `--backend memory` uses the engine's in-memory metadata and object stores (CPU cost only); `--backend disk`
//! uses what `ag init` creates (SQLite metadata and one file per object, with fsync), still without a network.
//!
//! A repository of `n` files has paths `pkgNNN/modNN/fileN.rs` and two-line contents. Operations that change
//! state (integrations) are sampled one after another on the same repository, each on top of the previous
//! result, so the repository grows by a handful of changesets per row; that is noted in the docs, not hidden.

use std::path::Path;
use std::time::{Duration, Instant};

use ag_changeset::Op;
use ag_integrate::{InitOptions, IntegrateRequest, Outcome, Repository, Strategy};
use ag_merge::{BlobIo, IoError, detect_renames, merge_trees};
use ag_storage::SystemClock;
use ag_tree::Tree;
use ag_types::{BlobId, ChangesetId, FileType, Identity, Namespace, RepoPath, StreamName};
use anyhow::{Result, ensure};
use serde::Serialize;

use crate::stats::Summary;

/// Where the repository under test keeps its data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// In-memory metadata and objects.
    Memory,
    /// SQLite and the filesystem, as `ag init` creates them.
    Disk,
}

/// What to run.
pub struct Options {
    /// Repository sizes (files).
    pub sizes: Vec<usize>,
    /// Samples per repeated operation.
    pub samples: usize,
    /// Stop sampling an operation after this long (but never below 3 samples).
    pub budget: Duration,
    /// Storage under test.
    pub backend: Backend,
    /// Only rows whose name contains this text (all if empty). The setup still runs.
    pub filter: String,
}

/// One measured operation at one size.
#[derive(Clone, Debug, Serialize)]
pub struct Row {
    /// Operation name.
    pub op: String,
    /// Repository size in files.
    pub files: usize,
    /// How the numbers were taken (what one sample is).
    pub note: String,
    /// Timing distribution of the samples.
    pub summary: Summary,
}

fn path(i: usize) -> String {
    format!("pkg{:03}/mod{:02}/file{i}.rs", i % 200, i % 40)
}

fn actor(name: &str) -> Identity {
    Identity::new(name).expect("valid identity")
}

fn put(repo: &Repository, p: &str, body: String) -> Result<Op> {
    Ok(Op::Put {
        path: RepoPath::new(p)?,
        content: repo.put_blob(body.into_bytes())?,
        file_type: FileType::Regular,
        executable: false,
    })
}

/// The repository's blobs as the merge's I/O.
struct RepoIo<'a>(&'a Repository);

impl BlobIo for RepoIo<'_> {
    fn read_blob(&self, id: &BlobId) -> std::result::Result<Vec<u8>, IoError> {
        self.0.blob(id).map_err(|e| Box::new(e) as IoError)
    }
    fn write_blob(&self, data: Vec<u8>) -> std::result::Result<BlobId, IoError> {
        self.0.put_blob(data).map_err(|e| Box::new(e) as IoError)
    }
}

struct Bench<'a> {
    opts: &'a Options,
    files: usize,
    rows: Vec<Row>,
}

impl Bench<'_> {
    fn wanted(&self, op: &str) -> bool {
        self.opts.filter.is_empty() || op.contains(&self.opts.filter)
    }

    fn record(&mut self, op: &str, note: &str, samples: &[Duration]) {
        let summary = Summary::of(samples);
        eprintln!(
            "  {:>7} files  {op:<58} p50 {:>10} ms  (n={})",
            self.files,
            crate::stats::fmt_ms(summary.p50_ms),
            summary.n
        );
        self.rows.push(Row { op: op.into(), files: self.files, note: note.into(), summary });
    }

    /// Times `f` once.
    fn once<T>(&mut self, op: &str, note: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let t = Instant::now();
        let v = f()?;
        let d = t.elapsed();
        self.record(op, note, &[d]);
        Ok(v)
    }

    /// Times `f` for up to `samples` rounds (within the budget, at least 3).
    fn repeat(&mut self, op: &str, note: &str, mut f: impl FnMut(usize) -> Result<Duration>) -> Result<()> {
        let started = Instant::now();
        let mut out = Vec::new();
        for i in 0..self.opts.samples.max(1) {
            out.push(f(i)?);
            if out.len() >= 3 && started.elapsed() > self.opts.budget {
                break;
            }
        }
        self.record(op, note, &out);
        Ok(())
    }
}

fn time<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let t = Instant::now();
    let v = f();
    (v, t.elapsed())
}

fn open_repo(opts: &Options, dir: &Path) -> Result<Repository> {
    let ns = Namespace::new("bench/app")?;
    let stream = StreamName::new("main")?;
    Ok(match opts.backend {
        Backend::Memory => Repository::ephemeral(ns, stream, std::sync::Arc::new(SystemClock))?,
        Backend::Disk => Repository::init(dir, &InitOptions::new(ns, stream, actor("user:alice")))?,
    })
}

fn head(repo: &Repository) -> Result<ChangesetId> {
    repo.meta()
        .read(|tx| tx.stream("main"))?
        .and_then(|s| s.state.head)
        .ok_or_else(|| anyhow::anyhow!("the stream has no head"))
}

fn integrate(repo: &Repository, proposal: &str, who: &Identity) -> Result<ag_integrate::IntegrationResult> {
    Ok(repo.integrate(&IntegrateRequest {
        proposal_id: proposal.into(),
        actor: who.clone(),
        idempotency_key: None,
        fencing_tokens: vec![],
    })?)
}

/// Runs the whole suite and returns the rows.
pub fn run(opts: &Options) -> Result<Vec<Row>> {
    let mut all = Vec::new();
    for &n in &opts.sizes {
        ensure!(n >= 1_000, "sizes below 1000 files are not meaningful for these benchmarks");
        eprintln!("\n== {n} files ({:?})", opts.backend);
        let mut b = Bench { opts, files: n, rows: Vec::new() };
        run_size(&mut b, n)?;
        all.append(&mut b.rows);
    }
    Ok(all)
}

fn run_size(b: &mut Bench<'_>, n: usize) -> Result<()> {
    let dir = tempfile::tempdir()?;
    let repo = open_repo(b.opts, dir.path())?;
    let limits = repo.limits().clone();
    let (alice, bob) = (actor("user:alice"), actor("user:bob"));

    // ---- ingest and the first publication ----------------------------------------------------------------
    let blame_path = "docs/blame_target.txt";
    let mut ops = b.once("ingest: put every blob", "one sample = all blobs of the import", || {
        let mut ops = Vec::with_capacity(n + 1);
        // The last of the n files is the history target below (one changeset may carry at most `max_ops` operations).
        for i in 0..n - 1 {
            ops.push(put(&repo, &path(i), format!("// file {i}\nfn f{i}() {{}}\n"))?);
        }
        Ok(ops)
    })?;
    ops.push(put(&repo, blame_path, (0..200).map(|l| format!("line {l}\n")).collect())?);
    let root = b.once("commit: build and store the import changeset", "one sample", || {
        Ok(repo.commit(vec![], &bob, "import", ops)?)
    })?;
    b.once("publish: the whole import (verify every blob)", "one sample = n blobs + 1 changeset", || {
        let r = repo.publish(&root, &bob, None)?;
        ensure!(r.new_blobs >= n, "unexpected publish result {r:?}");
        Ok(())
    })?;
    let prop = repo.propose("main", &root, "import", "", &bob, None)?;
    b.once("integrate: initial import into an empty stream", "one sample", || {
        let r = integrate(&repo, &prop.id, &alice)?;
        ensure!(matches!(r.outcome, Outcome::Integrated { .. }), "{:?}", r.outcome);
        Ok(())
    })?;

    // One-file edit on top of `base`, as a published-later changeset.
    let edit = |base: ChangesetId, k: usize, tag: &str| -> Result<ChangesetId> {
        let op = put(&repo, &path(k), format!("// edited by {tag}\n"))?;
        Ok(repo.commit(vec![base], &bob, tag, vec![op])?)
    };
    // Different files for every sample, spread over the repository and away from the rename targets below.
    let pick = |i: usize, salt: usize| (i * 7919 + salt * 104_729 + 13) % (n - 1);

    // ---- publish, propose, fast-forward ------------------------------------------------------------------
    let mut seq = 0usize;
    let mut publish_t = Vec::new();
    let mut propose_t = Vec::new();
    let mut ff_t = Vec::new();
    for i in 0..b.opts.samples.max(1) {
        let started = Instant::now();
        let h = head(&repo)?;
        let c = edit(h, pick(i, 1), &format!("ff-{i}"))?;
        let (r, d) = time(|| repo.publish(&c, &bob, None));
        ensure!(r?.new_changesets == 1);
        publish_t.push(d);
        let (p, d) = time(|| repo.propose("main", &c, "ff", "", &bob, None));
        propose_t.push(d);
        let p = p?;
        let (r, d) = time(|| integrate(&repo, &p.id, &alice));
        let r = r?;
        ensure!(matches!(r.outcome, Outcome::Integrated { strategy: Strategy::FastForward, .. }), "{:?}", r.outcome);
        ff_t.push(d);
        seq += 1;
        if i >= 2 && started.elapsed() > b.opts.budget {
            break;
        }
    }
    b.record(
        "publish: a one-file changeset on a published base",
        "one sample = publish of 1 changeset, 1 blob",
        &publish_t,
    );
    b.record("propose: open a proposal for a published head", "one sample", &propose_t);
    b.record(
        "integrate: fast-forward (1-file proposal)",
        "one sample = Repository::integrate, compare-and-swap included",
        &ff_t,
    );

    // ---- three-way merge ---------------------------------------------------------------------------------
    let mut merge_t = Vec::new();
    for i in 0..b.opts.samples.max(1) {
        let started = Instant::now();
        let h = head(&repo)?;
        let (ka, kb) = (pick(i, 2), pick(i, 3));
        let (a, bb) = (edit(h, ka, &format!("ma-{i}"))?, edit(h, kb, &format!("mb-{i}"))?);
        let pa = repo.propose("main", &a, "a", "", &bob, None)?;
        let pb = repo.propose("main", &bb, "b", "", &bob, None)?;
        let r = integrate(&repo, &pa.id, &alice)?;
        ensure!(matches!(r.outcome, Outcome::Integrated { .. }), "{:?}", r.outcome);
        let (r, d) = time(|| integrate(&repo, &pb.id, &alice));
        let r = r?;
        ensure!(matches!(r.outcome, Outcome::Integrated { strategy: Strategy::Merge, .. }), "{:?}", r.outcome);
        merge_t.push(d);
        seq += 1;
        if i >= 2 && started.elapsed() > b.opts.budget {
            break;
        }
    }
    b.record(
        "integrate: three-way merge (1 file vs 1 file)",
        "one sample = integrate of the second of two proposals from the same base",
        &merge_t,
    );

    // ---- three-way merge with a rename on one side -------------------------------------------------------
    let mut rename_t = Vec::new();
    for i in 0..b.opts.samples.max(1) {
        let started = Instant::now();
        let h = head(&repo)?;
        let k = pick(i, 4);
        let from = path(k);
        let to = format!("moved/{i}/file{k}.rs");
        let tree = repo.tree_of(&h)?;
        let entry = *tree.get(&RepoPath::new(&from)?).ok_or_else(|| anyhow::anyhow!("{from} missing"))?;
        let mv = vec![
            Op::Delete { path: RepoPath::new(&from)? },
            Op::Put {
                path: RepoPath::new(&to)?,
                content: entry.content,
                file_type: entry.file_type,
                executable: false,
            },
        ];
        let a = repo.commit(vec![h], &bob, "rename", mv)?;
        let bb = repo.commit(vec![h], &bob, "edit", vec![put(&repo, &from, format!("// edited in place {i}\n"))?])?;
        let pa = repo.propose("main", &a, "rename", "", &bob, None)?;
        let pb = repo.propose("main", &bb, "edit", "", &bob, None)?;
        integrate(&repo, &pa.id, &alice)?;
        let (r, d) = time(|| integrate(&repo, &pb.id, &alice));
        let r = r?;
        ensure!(matches!(r.outcome, Outcome::Integrated { strategy: Strategy::Merge, .. }), "{:?}", r.outcome);
        let merged = repo.tree_of(&head(&repo)?)?;
        ensure!(
            merged.get(&RepoPath::new(&to)?).is_some_and(|e| e.content != entry.content),
            "edit did not follow the rename"
        );
        rename_t.push(d);
        seq += 1;
        if i >= 2 && started.elapsed() > b.opts.budget {
            break;
        }
    }
    b.record(
        "integrate: three-way merge, rename vs edit of the same file",
        "one sample = integrate of the edit after the rename landed (the edit follows the rename)",
        &rename_t,
    );
    let _ = seq;

    // ---- merge_trees, tree diff, rename detection on prepared trees -------------------------------------
    let io = RepoIo(&repo);
    let base = repo.tree_of(&head(&repo)?)?;
    let edit_tree = |t: &Tree, k: usize, tag: &str| -> Result<Tree> {
        Ok(t.apply(&[put(&repo, &path(k), format!("// {tag}\n"))?], &limits)?)
    };
    let ours = edit_tree(&base, 7, "ours")?;
    let theirs = edit_tree(&base, n / 2 + 1, "theirs")?;
    if b.wanted("merge_trees") {
        b.repeat(
            "merge_trees: 1 file each side",
            "one sample = the pure merge, no storage writes except the result",
            |_| {
                let (m, d) = time(|| merge_trees(&base, &ours, &theirs, &io, &limits));
                ensure!(m.map_err(|e| anyhow::anyhow!("{e}"))?.is_clean());
                Ok(d)
            },
        )?;
    }
    // k files moved unchanged on ours; theirs edits one other file.
    let k = (n / 100).clamp(1, 1_000);
    let moved_ops = |edited: bool, count: usize| -> Result<Vec<Op>> {
        let mut ops = Vec::new();
        for j in 0..count {
            let from = path(100 + j * 3);
            let e = *base.get(&RepoPath::new(&from)?).ok_or_else(|| anyhow::anyhow!("{from} missing"))?;
            let content = if edited {
                let old = repo.blob(&e.content)?;
                let mut new = old.clone();
                new.extend_from_slice(b"// touched while moving\n");
                repo.put_blob(new)?
            } else {
                e.content
            };
            ops.push(Op::Delete { path: RepoPath::new(&from)? });
            ops.push(Op::Put {
                path: RepoPath::new(format!("moved/{j}.rs"))?,
                content,
                file_type: FileType::Regular,
                executable: false,
            });
        }
        Ok(ops)
    };
    // Similarity matching is attempted for at most MAX_SIMILARITY_CANDIDATES (200) deleted files; more are matched exactly only.
    let ks = k.min(100);
    let ours_moved = base.apply(&moved_ops(false, k)?, &limits)?;
    let ours_moved_edited = base.apply(&moved_ops(true, ks)?, &limits)?;
    if b.wanted("rename") {
        let note = format!("{k} files (1%, at most 1000) moved unchanged on one side; the other side edits one file");
        b.repeat("merge_trees: 1% of files renamed (exact) vs 1 edit", &note, |_| {
            let (m, d) = time(|| merge_trees(&base, &ours_moved, &theirs, &io, &limits));
            ensure!(m.map_err(|e| anyhow::anyhow!("{e}"))?.is_clean());
            Ok(d)
        })?;
        let note_sim =
            format!("{ks} files moved and edited on one side (matched by similarity); the other side edits one file");
        b.repeat("merge_trees: 100 renamed and edited files (similarity) vs 1 edit", &note_sim, |_| {
            let (m, d) = time(|| merge_trees(&base, &ours_moved_edited, &theirs, &io, &limits));
            ensure!(m.map_err(|e| anyhow::anyhow!("{e}"))?.is_clean());
            Ok(d)
        })?;
        b.repeat(
            "detect_renames: 1% of files renamed (exact)",
            "one sample = rename detection between two trees (1% = n/100, at most 1000 files)",
            |_| {
                let (r, d) = time(|| detect_renames(&base, &ours_moved, &io));
                ensure!(r.map_err(|e| anyhow::anyhow!("{e}"))?.len() == k);
                Ok(d)
            },
        )?;
        b.repeat(
            "detect_renames: 100 renamed and edited files (similarity)",
            "one sample = rename detection between two trees",
            |_| {
                let (r, d) = time(|| detect_renames(&base, &ours_moved_edited, &io));
                ensure!(r.map_err(|e| anyhow::anyhow!("{e}"))?.len() == ks);
                Ok(d)
            },
        )?;
    }
    if b.wanted("diff") {
        b.repeat("tree diff: 1 changed file", "one sample = Tree::diff of two trees of n files", |_| {
            let (ops, d) = time(|| base.diff(&ours));
            ensure!(ops.len() == 1);
            Ok(d)
        })?;
        b.repeat("tree diff: 1% of files changed", "one sample = Tree::diff", |_| {
            let many = k.max(n / 100);
            let mut ops = Vec::with_capacity(many);
            for j in 0..many {
                ops.push(put(&repo, &path(j * 97 % n), format!("// bulk {j}\n"))?);
            }
            let changed = base.apply(&ops, &limits)?;
            let (ops, d) = time(|| base.diff(&changed));
            ensure!(!ops.is_empty());
            Ok(d)
        })?;
    }

    // ---- the manifest ------------------------------------------------------------------------------------
    let bytes = base.encode();
    if b.wanted("manifest") {
        let note = format!("one sample = one pass over a {} MiB manifest", bytes.len() >> 20);
        b.repeat("manifest: Tree::decode (strict, as for a cached tree)", &note, |_| {
            let (t, d) = time(|| Tree::decode(&bytes, &limits));
            ensure!(t?.len() == base.len());
            Ok(d)
        })?;
        b.repeat("manifest: Tree::encode", &note, |_| Ok(time(|| base.encode()).1))?;
        b.repeat("manifest: Tree::id (encode + SHA-256)", &note, |_| Ok(time(|| base.id()).1))?;
        b.repeat("manifest: Tree::clone", "one sample = clone of the in-memory tree", |_| Ok(time(|| base.clone()).1))?;
        let head_now = head(&repo)?;
        b.repeat("tree_of: head of the stream (cache hit)", "one sample = Repository::tree_of", |_| {
            let (t, d) = time(|| repo.tree_of(&head_now));
            ensure!(t?.len() == base.len());
            Ok(d)
        })?;
    }

    // ---- history: blame and path log ---------------------------------------------------------------------
    if b.wanted("blame") || b.wanted("log") {
        const HISTORY: usize = 20;
        let mut parent = head(&repo)?;
        for h in 0..HISTORY {
            let body: String = (0..200)
                .map(|l| if l % 20 == h % 20 { format!("line {l} v{h}\n") } else { format!("line {l}\n") })
                .collect();
            parent = repo.commit(vec![parent], &bob, &format!("touch {h}"), vec![put(&repo, blame_path, body)?])?;
        }
        let p = repo.propose("main", &parent, "history", "", &bob, None)?;
        let r = integrate(&repo, &p.id, &alice)?;
        ensure!(matches!(r.outcome, Outcome::Integrated { .. }), "{:?}", r.outcome);
        let h = head(&repo)?;
        let note = format!("one sample = blame of a 200-line file changed by {HISTORY} changesets");
        b.repeat("blame: one file, 20 changesets of history", &note, |_| {
            let (r, d) = time(|| repo.blame(&h, blame_path, &Default::default()));
            ensure!(r?.lines.len() == 200);
            Ok(d)
        })?;
        b.repeat("log --path: one file, 20 changesets", "one sample = Repository::log(limit 25, path)", |_| {
            let (r, d) = time(|| repo.log(&h, 25, Some(blame_path)));
            ensure!(r?.len() >= HISTORY);
            Ok(d)
        })?;
    }
    Ok(())
}
