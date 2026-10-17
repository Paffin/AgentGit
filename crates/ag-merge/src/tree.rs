use std::collections::{BTreeMap, BTreeSet};

use ag_tree::{Entry, Tree};
use ag_types::{BlobId, FileType, Limits, RepoPath};

use crate::rename::detect_renames;
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
    let mut merged: BTreeMap<RepoPath, Entry> = BTreeMap::new();
    let mut conflicts: Vec<Conflict> = Vec::new();

    // Renames first: a file moved on one side and edited on the other is one unit at its new path.
    let plan = plan_renames(base, ours, theirs, io)?;
    for unit in &plan.units {
        match resolve_both_changed(Some(unit.base), Some(unit.ours), Some(unit.theirs), io)? {
            Resolution::Take(Some(e)) => {
                merged.insert(unit.to.clone(), e);
            }
            Resolution::Take(None) => {}
            Resolution::Conflict(kind) => {
                // Best effort: our content at the merged location.
                merged.insert(unit.to.clone(), unit.ours);
                conflicts.push(Conflict { path: unit.to.clone(), kind, origin: Some(unit.origin()) });
            }
        }
    }
    for c in plan.conflicts {
        for (path, entry) in c.keep {
            merged.insert(path, entry);
        }
        conflicts.push(c.conflict);
    }

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
        let resolved = if o == t || t == b {
            Resolution::Take(o)
        } else if o == b {
            Resolution::Take(t)
        } else {
            resolve_both_changed(b, o, t, io)?
        };
        match resolved {
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
struct RenamePlan {
    units: Vec<Unit>,
    conflicts: Vec<PlannedConflict>,
    /// Paths (in any of the three trees) the ordinary per-path rule must skip.
    consumed: BTreeSet<RepoPath>,
}

/// Decides, per renamed file, how the two sides combine. Fully determined by the three trees.
fn plan_renames(base: &Tree, ours: &Tree, theirs: &Tree, io: &dyn BlobIo) -> Result<RenamePlan, IoError> {
    let mut plan = RenamePlan::default();
    let ours_ren: BTreeMap<RepoPath, RepoPath> =
        detect_renames(base, ours, io)?.into_iter().map(|r| (r.from, r.to)).collect();
    let theirs_ren: BTreeMap<RepoPath, RepoPath> =
        detect_renames(base, theirs, io)?.into_iter().map(|r| (r.from, r.to)).collect();
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
