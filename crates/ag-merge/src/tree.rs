use std::collections::{BTreeMap, BTreeSet};

use ag_tree::{Change, Entry, Tree, TreeError};
use ag_types::{BlobId, FileType, Limits, RepoPath};

use crate::dirrename::{Analysis, Decision};
use crate::rename::detect_renames_in;
use crate::text::{TextMerge, merge_text};
use crate::{Algorithm, MAX_NOTE_PATHS};

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
    /// One side added a file beneath a directory the other side vacated, but the vacated directory's files did not
    /// all go to one place (`ag-merge/3`). Reported at the added file's path; nothing is guessed.
    DirectoryRenameAmbiguous {
        /// True if *our* side vacated the directory (and theirs added the file).
        ours_renamed: bool,
        /// The directory of the merge base.
        directory: RepoPath,
        /// The directories some of its files moved to, in path order (at most [`crate::MAX_AMBIGUOUS_CANDIDATES`]).
        candidates: Vec<RepoPath>,
    },
    /// A file added beneath a renamed directory would move to a path that is already taken (`ag-merge/3`).
    /// Reported at the added file's path.
    DirectoryRenameCollision {
        /// True if *our* side renamed the directory (and theirs added the file).
        ours_renamed: bool,
        /// The directory of the merge base.
        directory: RepoPath,
        /// Where the file would have gone.
        target: RepoPath,
    },
}

impl ConflictKind {
    /// Every code [`ConflictKind::code`] can return (a test keeps the list in step with the variants).
    pub const CODES: [&'static str; 11] = [
        "content",
        "both_added",
        "delete_modify",
        "binary",
        "type_change",
        "too_large",
        "file_directory",
        "rename_rename",
        "rename_delete",
        "directory_rename_ambiguous",
        "directory_rename_collision",
    ];

    /// The stable, machine-readable name of the kind, as it appears on the wire (`Conflict.kind` of the
    /// protocol) and in the Web UI: `content`, `both_added`, `delete_modify`, `binary`, `type_change`,
    /// `too_large`, `file_directory`, `rename_rename`, `rename_delete`, `directory_rename_ambiguous` or
    /// `directory_rename_collision`. Unlike the [`Display`](std::fmt::Display)
    /// text, which is for people and carries details (region counts, paths), these never change.
    pub fn code(&self) -> &'static str {
        match self {
            ConflictKind::Content { .. } => "content",
            ConflictKind::BothAdded => "both_added",
            ConflictKind::DeleteModify { .. } => "delete_modify",
            ConflictKind::Binary => "binary",
            ConflictKind::TypeChange => "type_change",
            ConflictKind::TooLarge => "too_large",
            ConflictKind::FileDirectory { .. } => "file_directory",
            ConflictKind::RenameRename { .. } => "rename_rename",
            ConflictKind::RenameDelete { .. } => "rename_delete",
            ConflictKind::DirectoryRenameAmbiguous { .. } => "directory_rename_ambiguous",
            ConflictKind::DirectoryRenameCollision { .. } => "directory_rename_collision",
        }
    }
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
            ConflictKind::DirectoryRenameAmbiguous { ours_renamed, directory, candidates } => {
                let (who, other) = if *ours_renamed { ("our", "theirs") } else { ("their", "ours") };
                let list = candidates.iter().map(|c| format!("'{c}'")).collect::<Vec<_>>().join(", ");
                write!(
                    f,
                    "added on {other} beneath '{directory}', which {who} side emptied by moving files to several places ({list}) \
                     or only some of them"
                )
            }
            ConflictKind::DirectoryRenameCollision { ours_renamed, directory, target } => {
                let (who, other) = if *ours_renamed { ("our", "theirs") } else { ("their", "ours") };
                write!(
                    f,
                    "added on {other} beneath '{directory}', which {who} side renamed, but '{target}' is already taken"
                )
            }
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

/// Which side of a merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    /// The stream head (or the workspace).
    Ours,
    /// The proposal (or the incoming side).
    Theirs,
}

impl Side {
    /// `ours` or `theirs`.
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Ours => "ours",
            Side::Theirs => "theirs",
        }
    }
}

/// An explanation of something the merge did on its own that a person may want to check. Notes never make a merge
/// unclean; they say *why* the result looks the way it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeNote {
    /// A directory was renamed on one side, and the other side's changes beneath the old directory followed it
    /// (`ag-merge/3`; the rule is in `docs/architecture/merge-semantics.md`).
    DirectoryRename {
        /// The side that renamed the directory.
        renamed_by: Side,
        /// The directory in the merge base.
        from: RepoPath,
        /// Where it went.
        to: RepoPath,
        /// How many of its files moved with unchanged relative paths ...
        moved_files: usize,
        /// ... out of this many in the base (more than half is needed).
        total_files: usize,
        /// Files the other side added beneath the old directory that were moved along, as `(added at, now at)`, in
        /// path order, at most [`crate::MAX_NOTE_PATHS`] of them.
        relocated: Vec<(RepoPath, RepoPath)>,
        /// How many files were moved along in all (the length of `relocated` unless it was cut).
        relocated_count: usize,
        /// How many files the other side *modified* that the directory move carried to a new path although their
        /// content no longer matched by similarity.
        followed_edits: usize,
    },
}

impl MergeNote {
    /// The stable, machine-readable name of the kind of note: `directory_rename`.
    pub fn code(&self) -> &'static str {
        match self {
            MergeNote::DirectoryRename { .. } => "directory_rename",
        }
    }
}

impl MergeNote {
    /// The note as one `Key: value` line for the message of a merge changeset, so history says why the merge looks
    /// the way it does (`Directory-Rename: lib -> src (ours renamed it; 3 of 3 files; 1 added and 0 edited followed)`).
    pub fn trailer(&self) -> String {
        match self {
            MergeNote::DirectoryRename {
                renamed_by,
                from,
                to,
                moved_files,
                total_files,
                relocated_count,
                followed_edits,
                ..
            } => format!(
                "Directory-Rename: {from} -> {to} ({} renamed it; {moved_files} of {total_files} files; \
                 {relocated_count} added and {followed_edits} edited followed)",
                renamed_by.as_str()
            ),
        }
    }
}

impl std::fmt::Display for MergeNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeNote::DirectoryRename {
                renamed_by,
                from,
                to,
                moved_files,
                total_files,
                relocated_count,
                followed_edits,
                ..
            } => {
                let other = if *renamed_by == Side::Ours { "theirs" } else { "ours" };
                write!(
                    f,
                    "directory '{from}' was renamed to '{to}' on {} ({moved_files} of {total_files} files); \
                     {relocated_count} file(s) added and {followed_edits} edited file(s) on {other} followed it",
                    renamed_by.as_str()
                )
            }
        }
    }
}

/// Outcome of [`merge_trees`].
#[derive(Clone, Debug)]
pub struct TreeMerge {
    /// The merged tree. Only meaningful when `conflicts` is empty; otherwise it
    /// is a best-effort tree that takes *our* version at conflicting paths.
    pub tree: Tree,
    /// All conflicts, ordered by path.
    pub conflicts: Vec<Conflict>,
    /// Explanations of what the merge decided on its own, ordered by (side, directory). Empty for
    /// `ag-merge/2`.
    pub notes: Vec<MergeNote>,
    // A file/directory clash cannot inhabit Tree. Retain its exact merged entries for an
    // explicit resolution instead of later reconstructing them from the fallback tree.
    unresolved_entries: Option<BTreeMap<RepoPath, Entry>>,
}

impl TreeMerge {
    /// True if the merge produced an unambiguous result.
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }

    /// Describes every candidate path beneath each file/directory conflict, subject to fixed bounds.
    /// A scope that is incomplete must not be used for automatic whole-subtree replacement.
    pub fn structural_resolution_scopes(&self) -> BTreeMap<&str, crate::ResolutionScope> {
        crate::scopes::collect_scopes(&self.conflicts, self.unresolved_entries.as_ref())
    }

    /// Applies explicit resolved path states before validating the resulting tree.
    ///
    /// Unlike the best-effort [`Self::tree`], this preserves the clean parts of a merge
    /// containing a file/directory clash. A caller must supply all desired structural
    /// changes: keeping a file never implicitly deletes entries beneath it.
    pub fn resolve_paths(
        &self,
        replacements: &BTreeMap<RepoPath, Option<Entry>>,
        limits: &Limits,
    ) -> Result<Tree, TreeError> {
        let mut entries = match &self.unresolved_entries {
            Some(entries) => entries.clone(),
            None => self.tree.iter().map(|(path, entry)| (path.clone(), *entry)).collect(),
        };
        for (path, entry) in replacements {
            match entry {
                Some(entry) => {
                    entries.insert(path.clone(), *entry);
                }
                None => {
                    entries.remove(path);
                }
            }
        }
        Tree::from_entries(entries, limits)
    }
}

/// Three-way merges `ours` and `theirs` relative to `base`.
///
/// `ours` is the stream head the result will extend; `theirs` is the proposal.
///
/// This is the current algorithm ([`Algorithm::CURRENT`], `ag-merge/3`); [`merge_trees_with`] runs an earlier one,
/// which is how a recorded merge is reproduced.
pub fn merge_trees(
    base: &Tree,
    ours: &Tree,
    theirs: &Tree,
    io: &dyn BlobIo,
    limits: &Limits,
) -> Result<TreeMerge, IoError> {
    merge_trees_with(Algorithm::CURRENT, base, ours, theirs, io, limits)
}

/// [`merge_trees`] under a specific algorithm version. `ag-merge/2` results stay reproducible: they are what this
/// returns for [`Algorithm::V2`].
pub fn merge_trees_with(
    algorithm: Algorithm,
    base: &Tree,
    ours: &Tree,
    theirs: &Tree,
    io: &dyn BlobIo,
    limits: &Limits,
) -> Result<TreeMerge, IoError> {
    // Two linear walks find everything that differs; the merge then looks only at those paths.
    let ours_changes = base.changes(ours);
    let theirs_changes = base.changes(theirs);
    let plan = plan_renames(algorithm, base, ours, theirs, &ours_changes, &theirs_changes, io)?;
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
    for r in &plan.relocations {
        desired.insert(r.from.clone(), None);
        desired.insert(r.to.clone(), Some(r.entry));
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
    // Only a file/directory collision is meant to hand over to the reference algorithm, which reports it as a
    // conflict. Every other failure (the merged tree is over the entry limit, or an engine bug made the ops
    // inconsistent) is an error here: it must never turn into a "clean" merge.
    let tree = match ours.apply(&ops, limits) {
        Ok(tree) => tree,
        Err(TreeError::FileDirectoryConflict { .. }) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    conflicts.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Some(TreeMerge { tree, conflicts, notes: plan.notes.clone(), unresolved_entries: None }))
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
    let unresolved_entries = (!file_dir.is_empty()).then(|| merged.clone());
    for (file, inside) in file_dir {
        conflicts.push(Conflict { path: file, kind: ConflictKind::FileDirectory { inside }, origin: None });
    }
    conflicts.sort_by(|a, b| a.path.cmp(&b.path));

    let tree = match Tree::from_entries(merged, limits) {
        Ok(t) => t,
        // A file with something beneath it: the FileDirectory conflicts above report it, and the best-effort
        // tree is ours.
        Err(TreeError::FileDirectoryConflict { .. }) => {
            debug_assert!(conflicts.iter().any(|c| matches!(c.kind, ConflictKind::FileDirectory { .. })));
            ours.clone()
        }
        // Anything else (the merged tree is over the entry limit) has no conflict that describes it, and "ours" would
        // silently drop everything "theirs" added: it is an error, never a clean result.
        Err(e) => return Err(e.into()),
    };
    Ok(TreeMerge { tree, conflicts, notes: plan.notes.clone(), unresolved_entries })
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

/// A file added beneath a renamed directory that moves to the renamed directory.
struct Relocation {
    from: RepoPath,
    to: RepoPath,
    entry: Entry,
}

#[derive(Default)]
pub(crate) struct RenamePlan {
    units: Vec<Unit>,
    relocations: Vec<Relocation>,
    notes: Vec<MergeNote>,
    conflicts: Vec<PlannedConflict>,
    /// Paths (in any of the three trees) the ordinary per-path rule must skip.
    consumed: BTreeSet<RepoPath>,
}

/// Decides, per renamed file, how the two sides combine. Fully determined by the three trees.
pub(crate) fn plan_renames(
    algorithm: Algorithm,
    base: &Tree,
    ours: &Tree,
    theirs: &Tree,
    ours_changes: &[Change],
    theirs_changes: &[Change],
    io: &dyn BlobIo,
) -> Result<RenamePlan, IoError> {
    let mut plan = RenamePlan::default();
    let mut ours_ren: BTreeMap<RepoPath, RepoPath> =
        detect_renames_in(ours_changes, io)?.into_iter().map(|r| (r.from, r.to)).collect();
    let mut theirs_ren: BTreeMap<RepoPath, RepoPath> =
        detect_renames_in(theirs_changes, io)?.into_iter().map(|r| (r.from, r.to)).collect();
    let directories = algorithm.follows_directory_renames();
    let mut ours_dirs = Analysis::default();
    let mut theirs_dirs = Analysis::default();
    let mut dir_pairs: Vec<(Side, String, RepoPath)> = Vec::new();
    if directories {
        let added = |changes: &'_ [Change]| -> Vec<RepoPath> {
            changes.iter().filter(|c| c.before.is_none() && c.after.is_some()).map(|c| c.path.clone()).collect()
        };
        let (ours_added, theirs_added) = (added(ours_changes), added(theirs_changes));
        let (theirs_probe, ours_probe): (Vec<&RepoPath>, Vec<&RepoPath>) =
            (theirs_added.iter().collect(), ours_added.iter().collect());
        ours_dirs = Analysis::of(base, ours_changes, &ours_ren, &theirs_probe);
        theirs_dirs = Analysis::of(base, theirs_changes, &theirs_ren, &ours_probe);
        for pair in ours_dirs.pairs(base, ours, theirs, ours_changes, &ours_ren) {
            dir_pairs.push((Side::Ours, pair.dir, pair.from.clone()));
            ours_ren.insert(pair.from, pair.to);
        }
        for pair in theirs_dirs.pairs(base, theirs, ours, theirs_changes, &theirs_ren) {
            dir_pairs.push((Side::Theirs, pair.dir, pair.from.clone()));
            theirs_ren.insert(pair.from, pair.to);
        }
    }
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
    if directories {
        relocate(
            &mut plan,
            &Followers {
                ours_dirs: &ours_dirs,
                theirs_dirs: &theirs_dirs,
                ours,
                theirs,
                ours_changes,
                theirs_changes,
                ours_targets: ours_ren.values().collect(),
                theirs_targets: theirs_ren.values().collect(),
                dir_pairs: &dir_pairs,
            },
        );
    }
    Ok(plan)
}

/// Everything [`relocate`] looks at.
struct Followers<'a> {
    ours_dirs: &'a Analysis,
    theirs_dirs: &'a Analysis,
    ours: &'a Tree,
    theirs: &'a Tree,
    ours_changes: &'a [Change],
    theirs_changes: &'a [Change],
    /// New paths of the file renames of each side (they are handled as renames, never relocated).
    ours_targets: BTreeSet<&'a RepoPath>,
    theirs_targets: BTreeSet<&'a RepoPath>,
    /// Files paired by their directory's move: (renaming side, directory, old path).
    dir_pairs: &'a [(Side, String, RepoPath)],
}

/// The directory-rename step of `ag-merge/3`: files one side added beneath a directory the other side renamed move
/// to the renamed directory; ambiguous and colliding cases become conflicts.
fn relocate(plan: &mut RenamePlan, w: &Followers<'_>) {
    struct Candidate {
        renamer: Side,
        dir: String,
        from: RepoPath,
        entry: Entry,
        outcome: Result<RepoPath, ConflictKind>,
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    // Files `adder` added, looked up in what `renamer` did with the directory.
    let passes = [
        (Side::Ours, w.ours_dirs, w.theirs_changes, &w.theirs_targets),
        (Side::Theirs, w.theirs_dirs, w.ours_changes, &w.ours_targets),
    ];
    for (renamer, dirs, adder_changes, adder_targets) in passes {
        let ours_renamed = renamer == Side::Ours;
        for ch in adder_changes {
            let (None, Some(entry)) = (&ch.before, &ch.after) else { continue };
            if plan.consumed.contains(&ch.path) || adder_targets.contains(&ch.path) {
                continue;
            }
            let Some((dir, decision)) = dirs.decide(&ch.path) else { continue };
            let directory = RepoPath::new(dir).expect("an ancestor of a valid path is valid");
            let outcome = match decision {
                Decision::Plain => continue,
                Decision::Ambiguous { candidates } => Err(ConflictKind::DirectoryRenameAmbiguous {
                    ours_renamed,
                    directory,
                    candidates: candidates.iter().filter_map(|c| RepoPath::new(c.as_str()).ok()).collect(),
                }),
                Decision::Moved { to, .. } => match RepoPath::new(format!("{to}{}", &ch.path.as_str()[dir.len()..])) {
                    Ok(target) => Ok(target),
                    Err(_) => continue,
                },
            };
            candidates.push(Candidate { renamer, dir: dir.to_string(), from: ch.path.clone(), entry: *entry, outcome });
        }
    }
    // Two files heading for the same place collide, and so does a file heading for an occupied one.
    let mut heading: BTreeMap<RepoPath, usize> = BTreeMap::new();
    for c in &candidates {
        if let Ok(t) = &c.outcome {
            *heading.entry(t.clone()).or_default() += 1;
        }
    }
    #[derive(Default)]
    struct Acc {
        relocated: Vec<(RepoPath, RepoPath)>,
        count: usize,
    }
    let mut accs: BTreeMap<(Side, String), Acc> = BTreeMap::new();
    for c in candidates {
        let ours_renamed = c.renamer == Side::Ours;
        match c.outcome {
            Err(kind) => {
                plan.consume(&c.from, std::iter::empty());
                plan.conflicts.push(PlannedConflict {
                    conflict: Conflict { path: c.from.clone(), kind, origin: None },
                    keep: vec![(c.from, c.entry)],
                });
            }
            Ok(target) => {
                let taken = w.ours.get(&target).is_some()
                    || w.theirs.get(&target).is_some()
                    || plan.consumed.contains(&target)
                    || heading.get(&target).copied().unwrap_or(0) > 1;
                if taken {
                    let directory = RepoPath::new(c.dir.as_str()).expect("valid directory");
                    plan.consume(&c.from, std::iter::empty());
                    plan.conflicts.push(PlannedConflict {
                        conflict: Conflict {
                            path: c.from.clone(),
                            kind: ConflictKind::DirectoryRenameCollision { ours_renamed, directory, target },
                            origin: None,
                        },
                        keep: vec![(c.from, c.entry)],
                    });
                    continue;
                }
                plan.consume(&c.from, [&target]);
                let acc = accs.entry((c.renamer, c.dir)).or_default();
                acc.count += 1;
                if acc.relocated.len() < MAX_NOTE_PATHS {
                    acc.relocated.push((c.from.clone(), target.clone()));
                }
                plan.relocations.push(Relocation { from: c.from, to: target, entry: c.entry });
            }
        }
    }
    // Edits the directory move carried along (only those the merge actually used).
    let mut followed: BTreeMap<(Side, String), usize> = BTreeMap::new();
    for (side, dir, from) in w.dir_pairs {
        if plan.consumed.contains(from) {
            *followed.entry((*side, dir.clone())).or_default() += 1;
        }
    }
    let keys: BTreeSet<(Side, String)> = accs.keys().chain(followed.keys()).cloned().collect();
    for (side, dir) in keys {
        let dirs = if side == Side::Ours { w.ours_dirs } else { w.theirs_dirs };
        let Some((to, votes, total)) = dirs.moved(&dir) else { continue };
        let (Ok(from_dir), Ok(to_dir)) = (RepoPath::new(dir.as_str()), RepoPath::new(to)) else { continue };
        let acc = accs.remove(&(side, dir.clone())).unwrap_or_default();
        plan.notes.push(MergeNote::DirectoryRename {
            renamed_by: side,
            from: from_dir,
            to: to_dir,
            moved_files: votes,
            total_files: total,
            relocated: acc.relocated,
            relocated_count: acc.count,
            followed_edits: followed.get(&(side, dir)).copied().unwrap_or(0),
        });
    }
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
