use std::collections::{BTreeMap, BTreeSet};

use ag_tree::{Change, Entry, Tree};
use ag_types::{BlobId, FileType, Limits, RepoPath};

use crate::rename::detect_renames_in;
use crate::text::{TextMerge, merge_text};

/// Largest blob (per side) that is text-merged; bigger files conflict.
pub const MAX_TEXT_MERGE_SIZE: usize = 16 * 1024 * 1024;

/// Error type of [`BlobIo`] implementations.
pub type IoError = Box<dyn std::error::Error + Send + Sync>;

/// Blob access needed to merge file contents.
pub trait BlobIo {
    /// Reads a blob's bytes.
    fn read_blob(&self, id: &BlobId) -> Result<Vec<u8>, IoError>;
    /// Stores bytes as a blob and returns its identifier.
    fn write_blob(&self, data: Vec<u8>) -> Result<BlobId, IoError>;
}

/// Why a path could not be merged automatically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    /// Both sides edited overlapping or touching lines differently.
    Content {
        /// Number of conflicting regions.
        regions: usize,
    },
    /// Both sides added the path with different content.
    BothAdded,
    /// One side deleted the path while the other modified it.
    DeleteModify {
        /// True if *our* side deleted it.
        ours_deleted: bool,
    },
    /// Both sides changed a binary file differently.
    Binary,
    /// File type changed (e.g. file ↔ symlink) or symlink targets differ.
    TypeChange,
    /// A file is too large for text merge.
    TooLarge,
    /// The merged result would hold a file where a directory is needed.
    FileDirectory {
        /// A path that lies beneath this file in the merged result.
        inside: RepoPath,
    },
    /// Both sides renamed the same file, to different paths. Reported at the original path.
    RenameRename {
        /// Where our side moved it.
        ours: RepoPath,
        /// Where their side moved it.
        theirs: RepoPath,
    },
    /// One side renamed a file the other side deleted. Reported at the new path.
    RenameDelete {
        /// True if *our* side renamed it (and theirs deleted it).
        ours_renamed: bool,
    },
}

impl std::fmt::Display for ConflictKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConflictKind::Content { regions } => write!(f, "content conflict ({regions} region(s))"),
            ConflictKind::BothAdded => f.write_str("both sides added different content"),
            ConflictKind::DeleteModify { ours_deleted: true } => f.write_str("deleted on our side, modified on theirs"),
            ConflictKind::DeleteModify { ours_deleted: false } => {
                f.write_str("modified on our side, deleted on theirs")
            }
            ConflictKind::Binary => f.write_str("binary file changed on both sides"),
            ConflictKind::TypeChange => f.write_str("file type or symlink target changed on both sides"),
            ConflictKind::TooLarge => f.write_str("file too large to merge"),
            ConflictKind::FileDirectory { inside } => write!(f, "file/directory conflict with '{inside}'"),
            ConflictKind::RenameRename { ours, theirs } => {
                write!(f, "renamed to '{ours}' on our side and to '{theirs}' on theirs")
            }
            ConflictKind::RenameDelete { ours_renamed: true } => f.write_str("renamed on our side, deleted on theirs"),
            ConflictKind::RenameDelete { ours_renamed: false } => f.write_str("deleted on our side, renamed on theirs"),
        }
    }
}

/// Where the three versions of a conflicting file live when a rename is involved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictOrigin {
    /// Path in the merge base.
    pub base: RepoPath,
    /// Path in our tree.
    pub ours: RepoPath,
    /// Path in their tree.
    pub theirs: RepoPath,
}

/// A conflict at one path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The conflicting path (in the merged result).
    pub path: RepoPath,
    /// What kind of conflict.
    pub kind: ConflictKind,
    /// Set when the three versions are not all stored at `path` because a side renamed the file.
    pub origin: Option<ConflictOrigin>,
}

/// Outcome of [`merge_trees`].
#[derive(Clone, Debug)]
pub struct TreeMerge {
    /// The merged tree. Only meaningful when `conflicts` is empty; otherwise it
    /// is a best-effort tree that takes *our* version at conflicting paths.
    pub tree: Tree,
    /// All conflicts, ordered by path.
    pub conflicts: Vec<Conflict>,
}

impl TreeMerge {
    /// True if the merge produced an unambiguous result.
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }
}

/// Three-way merges `ours` and `theirs` relative to `base`.
///
/// `ours` is the stream head the result will extend; `theirs` is the proposal.
pub fn merge_trees(
    base: &Tree,
    ours: &Tree,
    theirs: &Tree,
    io: &dyn BlobIo,
    limits: &Limits,
) -> Result<TreeMerge, IoError> {
    // Two linear walks find everything that differs; the merge then looks only at those paths.
    let ours_changes = base.changes(ours);
    let theirs_changes = base.changes(theirs);
    let plan = plan_renames(base, ours, theirs, &ours_changes, &theirs_changes, io)?;
    if let Some(merged) = merge_incremental(ours, &theirs_changes, &plan, io, limits)? {
        return Ok(merged);
    }
    merge_full(base, ours, theirs, &plan, io, limits)
}

/// Resolves the rename units and planned conflicts into `desired` (final entries of the paths they touch)
/// and `conflicts`. Shared by both merge strategies so they cannot drift apart.
fn resolve_plan(
    plan: &RenamePlan,
    io: &dyn BlobIo,
    desired: &mut BTreeMap<RepoPath, Option<Entry>>,
    conflicts: &mut Vec<Conflict>,
) -> Result<(), IoError> {
    // A consumed path is absent from the result unless a unit or a planned conflict puts something there.
    for p in &plan.consumed {
        desired.insert(p.clone(), None);
    }
    for unit in &plan.units {
        match resolve_both_changed(Some(unit.base), Some(unit.ours), Some(unit.theirs), io)? {
            Resolution::Take(Some(e)) => {
                desired.insert(unit.to.clone(), Some(e));
            }
            Resolution::Take(None) => {}
            Resolution::Conflict(kind) => {
                // Best effort: our content at the merged location.
                desired.insert(unit.to.clone(), Some(unit.ours));
                conflicts.push(Conflict { path: unit.to.clone(), kind, origin: Some(unit.origin()) });
            }
        }
    }
    for c in &plan.conflicts {
        for (path, entry) in &c.keep {
            desired.insert(path.clone(), Some(*entry));
        }
        conflicts.push(c.conflict.clone());
    }
    Ok(())
}

/// The ordinary per-path rule.
fn resolve_path(
    base: Option<Entry>,
    ours: Option<Entry>,
    theirs: Option<Entry>,
    io: &dyn BlobIo,
) -> Result<Resolution, IoError> {
    Ok(if ours == theirs || theirs == base {
        Resolution::Take(ours)
    } else if ours == base {
        Resolution::Take(theirs)
    } else {
        resolve_both_changed(base, ours, theirs, io)?
    })
}

/// Merge by editing `ours`: only paths their side changed (or a rename touched) are examined, and the
/// result is `ours` with those paths replaced. Linear walks and no per-path work for unchanged files.
///
/// Returns `None` when the edited tree would break the file/directory rule; the caller then runs
/// [`merge_full`], which reports such cases as conflicts.
pub(crate) fn merge_incremental(
    ours: &Tree,
    theirs_changes: &[Change],
    plan: &RenamePlan,
    io: &dyn BlobIo,
    limits: &Limits,
) -> Result<Option<TreeMerge>, IoError> {
    let mut desired: BTreeMap<RepoPath, Option<Entry>> = BTreeMap::new();
    let mut conflicts: Vec<Conflict> = Vec::new();
    resolve_plan(plan, io, &mut desired, &mut conflicts)?;
    for ch in theirs_changes {
        if plan.consumed.contains(&ch.path) {
            continue;
        }
        let o = ours.get(&ch.path).copied();
        match resolve_path(ch.before, o, ch.after, io)? {
            Resolution::Take(e) => {
                desired.insert(ch.path.clone(), e);
            }
            Resolution::Conflict(kind) => {
                desired.insert(ch.path.clone(), o);
                conflicts.push(Conflict { path: ch.path.clone(), kind, origin: None });
            }
        }
    }
    let ops: Vec<ag_changeset::Op> = desired
        .into_iter()
        .filter_map(|(path, want)| match (want, ours.get(&path).copied()) {
            (Some(w), Some(h)) if w == h => None,
            (Some(w), _) => Some(ag_changeset::Op::Put {
                path,
                content: w.content,
                file_type: w.file_type,
                executable: w.executable,
            }),
            (None, Some(_)) => Some(ag_changeset::Op::Delete { path }),
            (None, None) => None,
        })
        .collect();
    let Ok(tree) = ours.apply(&ops, limits) else { return Ok(None) };
    conflicts.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Some(TreeMerge { tree, conflicts }))
}

/// The reference algorithm: visit every path of the three trees and build the merged map from scratch.
/// Slower, but it also reports file/directory conflicts; [`merge_incremental`] must agree with it
/// wherever it answers (property-tested).
pub(crate) fn merge_full(
    base: &Tree,
    ours: &Tree,
    theirs: &Tree,
    plan: &RenamePlan,
    io: &dyn BlobIo,
    limits: &Limits,
) -> Result<TreeMerge, IoError> {
    let mut desired: BTreeMap<RepoPath, Option<Entry>> = BTreeMap::new();
    let mut conflicts: Vec<Conflict> = Vec::new();
    resolve_plan(plan, io, &mut desired, &mut conflicts)?;
    let mut merged: BTreeMap<RepoPath, Entry> = desired.into_iter().filter_map(|(p, e)| e.map(|e| (p, e))).collect();

    let mut paths: BTreeSet<&RepoPath> = BTreeSet::new();
    paths.extend(base.iter().map(|(p, _)| p));
    paths.extend(ours.iter().map(|(p, _)| p));
    paths.extend(theirs.iter().map(|(p, _)| p));

    for path in paths {
        if plan.consumed.contains(path) {
            continue;
        }
        let b = base.get(path).copied();
        let o = ours.get(path).copied();
        let t = theirs.get(path).copied();
        match resolve_path(b, o, t, io)? {
            Resolution::Take(e) => {
                if let Some(e) = e {
                    merged.insert(path.clone(), e);
                }
            }
            Resolution::Conflict(kind) => {
                if let Some(e) = o {
                    merged.insert(path.clone(), e);
                }
                conflicts.push(Conflict { path: path.clone(), kind, origin: None });
            }
        }
    }

    // Structural validity: no file may have anything beneath it.
    let mut file_dir: BTreeMap<RepoPath, RepoPath> = BTreeMap::new();
    for path in merged.keys() {
        for anc in path.ancestors() {
            let anc = RepoPath::new(anc).expect("ancestor of valid path is valid");
            if merged.contains_key(&anc) {
                file_dir.entry(anc).or_insert_with(|| path.clone());
            }
        }
    }
    for (file, inside) in file_dir {
        conflicts.push(Conflict { path: file, kind: ConflictKind::FileDirectory { inside }, origin: None });
    }
    conflicts.sort_by(|a, b| a.path.cmp(&b.path));

    let tree = match Tree::from_entries(merged.clone(), limits) {
        Ok(t) => t,
        // Structurally invalid best-effort result: fall back to our tree.
        Err(_) => ours.clone(),
    };
    Ok(TreeMerge { tree, conflicts })
}

/// A file that moved on at least one side and is merged at its new path.
struct Unit {
    from: RepoPath,
    to: RepoPath,
    ours_path: RepoPath,
    theirs_path: RepoPath,
    base: Entry,
    ours: Entry,
    theirs: Entry,
}

impl Unit {
    fn origin(&self) -> ConflictOrigin {
        ConflictOrigin { base: self.from.clone(), ours: self.ours_path.clone(), theirs: self.theirs_path.clone() }
    }
}

/// A conflict decided by the rename analysis, with the entries the best-effort tree keeps.
struct PlannedConflict {
    conflict: Conflict,
    keep: Vec<(RepoPath, Entry)>,
}

#[derive(Default)]
pub(crate) struct RenamePlan {
    units: Vec<Unit>,
    conflicts: Vec<PlannedConflict>,
    /// Paths (in any of the three trees) the ordinary per-path rule must skip.
    consumed: BTreeSet<RepoPath>,
}

/// Decides, per renamed file, how the two sides combine. Fully determined by the three trees.
pub(crate) fn plan_renames(
    base: &Tree,
    ours: &Tree,
    theirs: &Tree,
    ours_changes: &[Change],
    theirs_changes: &[Change],
    io: &dyn BlobIo,
) -> Result<RenamePlan, IoError> {
    let mut plan = RenamePlan::default();
    let ours_ren: BTreeMap<RepoPath, RepoPath> =
        detect_renames_in(ours_changes, io)?.into_iter().map(|r| (r.from, r.to)).collect();
    let theirs_ren: BTreeMap<RepoPath, RepoPath> =
        detect_renames_in(theirs_changes, io)?.into_iter().map(|r| (r.from, r.to)).collect();
    let froms: BTreeSet<&RepoPath> = ours_ren.keys().chain(theirs_ren.keys()).collect();

    for from in froms {
        let Some(b) = base.get(from).copied() else { continue };
        match (ours_ren.get(from), theirs_ren.get(from)) {
            (Some(o_to), Some(t_to)) if o_to == t_to => {
                let (Some(o), Some(t)) = (ours.get(o_to).copied(), theirs.get(t_to).copied()) else { continue };
                plan.consume(from, [o_to]);
                plan.units.push(Unit {
                    from: from.clone(),
                    to: o_to.clone(),
                    ours_path: o_to.clone(),
                    theirs_path: t_to.clone(),
                    base: b,
                    ours: o,
                    theirs: t,
                });
            }
            (Some(o_to), Some(t_to)) => {
                let (Some(o), Some(t)) = (ours.get(o_to).copied(), theirs.get(t_to).copied()) else { continue };
                plan.consume(from, [o_to, t_to]);
                plan.conflicts.push(PlannedConflict {
                    conflict: Conflict {
                        path: from.clone(),
                        kind: ConflictKind::RenameRename { ours: o_to.clone(), theirs: t_to.clone() },
                        origin: None,
                    },
                    keep: vec![(o_to.clone(), o), (t_to.clone(), t)],
                });
            }
            (Some(o_to), None) => {
                // Only we moved it. If they left it alone, the ordinary rule already does the right thing.
                let Some(o) = ours.get(o_to).copied() else { continue };
                if theirs.get(o_to).is_some() {
                    continue; // they also have something at the new path: ordinary both-added logic
                }
                match theirs.get(from).copied() {
                    Some(t) if t == b => {}
                    Some(t) => {
                        plan.consume(from, [o_to]);
                        plan.units.push(Unit {
                            from: from.clone(),
                            to: o_to.clone(),
                            ours_path: o_to.clone(),
                            theirs_path: from.clone(),
                            base: b,
                            ours: o,
                            theirs: t,
                        });
                    }
                    None => {
                        plan.consume(from, [o_to]);
                        plan.conflicts.push(PlannedConflict {
                            conflict: Conflict {
                                path: o_to.clone(),
                                kind: ConflictKind::RenameDelete { ours_renamed: true },
                                origin: None,
                            },
                            keep: vec![(o_to.clone(), o)],
                        });
                    }
                }
            }
            (None, Some(t_to)) => {
                let Some(t) = theirs.get(t_to).copied() else { continue };
                if ours.get(t_to).is_some() {
                    continue;
                }
                match ours.get(from).copied() {
                    Some(o) if o == b => {}
                    Some(o) => {
                        plan.consume(from, [t_to]);
                        plan.units.push(Unit {
                            from: from.clone(),
                            to: t_to.clone(),
                            ours_path: from.clone(),
                            theirs_path: t_to.clone(),
                            base: b,
                            ours: o,
                            theirs: t,
                        });
                    }
                    None => {
                        plan.consume(from, [t_to]);
                        // Our side deleted it, theirs renamed it: keep their file in the best-effort tree.
                        plan.conflicts.push(PlannedConflict {
                            conflict: Conflict {
                                path: t_to.clone(),
                                kind: ConflictKind::RenameDelete { ours_renamed: false },
                                origin: None,
                            },
                            keep: vec![(t_to.clone(), t)],
                        });
                    }
                }
            }
            (None, None) => {}
        }
    }
    Ok(plan)
}

impl RenamePlan {
    fn consume<'a>(&mut self, from: &RepoPath, tos: impl IntoIterator<Item = &'a RepoPath>) {
        self.consumed.insert(from.clone());
        self.consumed.extend(tos.into_iter().cloned());
    }
}

enum Resolution {
    Take(Option<Entry>),
    Conflict(ConflictKind),
}

fn resolve_both_changed(
    base: Option<Entry>,
    ours: Option<Entry>,
    theirs: Option<Entry>,
    io: &dyn BlobIo,
) -> Result<Resolution, IoError> {
    let (o, t) = match (ours, theirs) {
        (Some(o), Some(t)) => (o, t),
        // Exactly one side deleted; the other changed it (otherwise an earlier rule applied).
        (None, Some(_)) => return Ok(Resolution::Conflict(ConflictKind::DeleteModify { ours_deleted: true })),
        (Some(_), None) => return Ok(Resolution::Conflict(ConflictKind::DeleteModify { ours_deleted: false })),
        (None, None) => return Ok(Resolution::Take(None)),
    };
    let Some(b) = base else {
        return Ok(Resolution::Conflict(ConflictKind::BothAdded));
    };
    if !(b.file_type == FileType::Regular && o.file_type == FileType::Regular && t.file_type == FileType::Regular) {
        return Ok(Resolution::Conflict(ConflictKind::TypeChange));
    }
    let executable = if o.executable == t.executable {
        o.executable
    } else if o.executable == b.executable {
        t.executable
    } else {
        o.executable
    };
    let content = if o.content == t.content {
        o.content
    } else if o.content == b.content {
        t.content
    } else if t.content == b.content {
        o.content
    } else {
        let bb = io.read_blob(&b.content)?;
        let ob = io.read_blob(&o.content)?;
        let tb = io.read_blob(&t.content)?;
        if [bb.len(), ob.len(), tb.len()].iter().any(|n| *n > MAX_TEXT_MERGE_SIZE) {
            return Ok(Resolution::Conflict(ConflictKind::TooLarge));
        }
        if [&bb, &ob, &tb].iter().any(|d| ag_diff::is_binary(d)) {
            return Ok(Resolution::Conflict(ConflictKind::Binary));
        }
        match merge_text(&bb, &ob, &tb) {
            TextMerge::Clean(bytes) => io.write_blob(bytes)?,
            TextMerge::Conflicted { regions, .. } => {
                return Ok(Resolution::Conflict(ConflictKind::Content { regions: regions.len() }));
            }
        }
    };
    Ok(Resolution::Take(Some(Entry { content, file_type: FileType::Regular, executable })))
}
