use std::collections::{BTreeMap, BTreeSet};

use ag_tree::{Entry, Tree};
use ag_types::{BlobId, FileType, Limits, RepoPath};

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
}

impl std::fmt::Display for ConflictKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConflictKind::Content { regions } => write!(f, "content conflict ({regions} region(s))"),
            ConflictKind::BothAdded => f.write_str("both sides added different content"),
            ConflictKind::DeleteModify { ours_deleted: true } => f.write_str("deleted on our side, modified on theirs"),
            ConflictKind::DeleteModify { ours_deleted: false } => f.write_str("modified on our side, deleted on theirs"),
            ConflictKind::Binary => f.write_str("binary file changed on both sides"),
            ConflictKind::TypeChange => f.write_str("file type or symlink target changed on both sides"),
            ConflictKind::TooLarge => f.write_str("file too large to merge"),
            ConflictKind::FileDirectory { inside } => write!(f, "file/directory conflict with '{inside}'"),
        }
    }
}

/// A conflict at one path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The conflicting path.
    pub path: RepoPath,
    /// What kind of conflict.
    pub kind: ConflictKind,
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
    let mut paths: BTreeSet<&RepoPath> = BTreeSet::new();
    paths.extend(base.iter().map(|(p, _)| p));
    paths.extend(ours.iter().map(|(p, _)| p));
    paths.extend(theirs.iter().map(|(p, _)| p));

    let mut merged: BTreeMap<RepoPath, Entry> = BTreeMap::new();
    let mut conflicts: Vec<Conflict> = Vec::new();

    for path in paths {
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
                conflicts.push(Conflict { path: path.clone(), kind });
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
        conflicts.push(Conflict { path: file, kind: ConflictKind::FileDirectory { inside } });
    }
    conflicts.sort_by(|a, b| a.path.cmp(&b.path));

    let tree = match Tree::from_entries(merged.clone(), limits) {
        Ok(t) => t,
        // Structurally invalid best-effort result: fall back to our tree.
        Err(_) => ours.clone(),
    };
    Ok(TreeMerge { tree, conflicts })
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
