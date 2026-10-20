//! Trees: the *derived* resulting state of a changeset.
//!
//! The canonical history is the changeset DAG. A [`Tree`] is a verifiable
//! projection of it: `tree(c) = apply(tree(first_parent(c)), c.ops)`, with
//! `tree(root)` starting from the empty tree. Because a tree can always be
//! recomputed from history and blobs, any cached copy is disposable.
//!
//! A tree is a flat, sorted manifest of `path -> (blob, type, executable)`.
//! Its [`TreeId`] is the domain-separated hash of the canonical manifest
//! encoding, so equal states have equal ids no matter how they were reached.
//! Validation results and attestations are bound to a `TreeId`.
//!
//! Structural invariants enforced by [`Tree::apply`]:
//!
//! * a delete must target an existing entry;
//! * a put must change something (no-op puts are rejected so that the
//!   operation list of a changeset is the *minimal* delta);
//! * no path may be both a file and a directory.

use std::collections::BTreeMap;

use ag_changeset::{Changeset, ChangesetSource, Op, SourceError};
use ag_object::canon::{CanonError, Reader, Writer};
use ag_object::{ObjectKind, object_id};
use ag_types::{BlobId, ChangesetId, FileType, Limits, ObjectId, RepoPath, TreeId};

/// Current manifest format version.
pub const TREE_FORMAT_VERSION: u16 = 1;

/// One entry of a tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Entry {
    /// Content blob (file bytes, or the link target for symlinks).
    pub content: BlobId,
    /// Entry type.
    pub file_type: FileType,
    /// Executable bit.
    pub executable: bool,
}

/// Errors from applying operations or decoding a manifest.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TreeError {
    /// A delete targeted a path that does not exist.
    #[error("cannot delete '{0}': no such entry")]
    DeleteMissing(RepoPath),
    /// A put did not change the entry (non-minimal changeset).
    #[error("put of '{0}' does not change anything")]
    NoOpPut(RepoPath),
    /// A path is used both as a file and as a directory.
    #[error("'{file}' is a file but '{inside}' lies beneath it")]
    FileDirectoryConflict {
        /// The path that holds a file.
        file: RepoPath,
        /// A path that would live inside it.
        inside: RepoPath,
    },
    /// The tree would exceed the entry limit.
    #[error("tree has too many entries ({0})")]
    TooManyEntries(usize),
    /// Manifest decoding failed.
    #[error("malformed tree manifest: {0}")]
    Malformed(String),
    /// A changeset in the chain could not be loaded.
    #[error(transparent)]
    Source(#[from] SourceError),
    /// The first-parent chain is deeper than the configured limit.
    #[error("history too deep (more than {0} changesets)")]
    TooDeep(u64),
}

impl From<CanonError> for TreeError {
    fn from(e: CanonError) -> Self {
        TreeError::Malformed(e.to_string())
    }
}

/// An item of a directory listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirEntry {
    /// A file or symlink directly inside the directory.
    File {
        /// Final path component.
        name: String,
        /// The entry.
        entry: Entry,
    },
    /// A sub-directory (derived: directories are not stored).
    Dir {
        /// Final path component.
        name: String,
    },
}

impl DirEntry {
    /// The final path component.
    pub fn name(&self) -> &str {
        match self {
            DirEntry::File { name, .. } | DirEntry::Dir { name } => name,
        }
    }
}

/// One path that differs between two trees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The path.
    pub path: RepoPath,
    /// Its entry in the first tree, if any.
    pub before: Option<Entry>,
    /// Its entry in the second tree, if any.
    pub after: Option<Entry>,
}

/// A repository state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    entries: BTreeMap<RepoPath, Entry>,
}

impl Tree {
    /// The empty tree (state of a root changeset's parent).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Number of entries (files and symlinks).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// An estimate of the memory this tree holds, in bytes: per entry the path's text and handle, the entry itself
    /// and the map's per-entry share (three machine words of node bookkeeping). Close enough to size a cache by; it
    /// is not an allocator measurement.
    pub fn approx_bytes(&self) -> usize {
        let per_entry =
            std::mem::size_of::<RepoPath>() + std::mem::size_of::<Entry>() + 3 * std::mem::size_of::<usize>();
        std::mem::size_of::<Self>() + self.entries.keys().map(|p| per_entry + p.as_str().len()).sum::<usize>()
    }

    /// True if there are no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Looks up an entry.
    pub fn get(&self, path: &RepoPath) -> Option<&Entry> {
        self.entries.get(path)
    }

    /// Iterates entries in canonical (bytewise path) order.
    pub fn iter(&self) -> impl Iterator<Item = (&RepoPath, &Entry)> {
        self.entries.iter()
    }

    /// Builds a tree directly from entries, validating the file/directory rule.
    pub fn from_entries(
        entries: impl IntoIterator<Item = (RepoPath, Entry)>,
        limits: &Limits,
    ) -> Result<Self, TreeError> {
        let tree = Self { entries: entries.into_iter().collect() };
        if tree.entries.len() > limits.max_tree_entries {
            return Err(TreeError::TooManyEntries(tree.entries.len()));
        }
        tree.check_structure()?;
        Ok(tree)
    }

    /// Verifies the file/directory rule for the whole tree in one pass: no path may lie beneath another
    /// entry. (If a file `a` had anything beneath it, checking that thing's ancestors finds `a`, so looking
    /// at ancestors alone is complete.) Linear in the number of entries, with no allocation per path.
    fn check_structure(&self) -> Result<(), TreeError> {
        let present: std::collections::HashSet<&str> = self.entries.keys().map(RepoPath::as_str).collect();
        for path in self.entries.keys() {
            for anc in path.ancestors() {
                if present.contains(anc) {
                    let file = RepoPath::new(anc).expect("ancestor of valid path");
                    return Err(TreeError::FileDirectoryConflict { file, inside: path.clone() });
                }
            }
        }
        Ok(())
    }

    /// Verifies that `path` neither sits under a file nor shadows a directory.
    fn check_path(&self, path: &RepoPath) -> Result<(), TreeError> {
        for anc in path.ancestors() {
            if let Some((file, _)) = self.entries.get_key_value(&RepoPath::new(anc).expect("ancestor of valid path")) {
                return Err(TreeError::FileDirectoryConflict { file: file.clone(), inside: path.clone() });
            }
        }
        let prefix = path.as_str();
        for (other, _) in self.entries.range(path.clone()..).skip(1) {
            let o = other.as_str();
            if !o.starts_with(prefix) {
                break;
            }
            if o.as_bytes()[prefix.len()] == b'/' {
                return Err(TreeError::FileDirectoryConflict { file: path.clone(), inside: other.clone() });
            }
        }
        Ok(())
    }

    /// Applies a changeset's operations, returning the resulting tree.
    pub fn apply(&self, ops: &[Op], limits: &Limits) -> Result<Tree, TreeError> {
        let mut next = self.clone();
        for op in ops {
            match op {
                Op::Delete { path } => {
                    if next.entries.remove(path).is_none() {
                        return Err(TreeError::DeleteMissing(path.clone()));
                    }
                }
                Op::Put { path, content, file_type, executable } => {
                    let entry = Entry { content: *content, file_type: *file_type, executable: *executable };
                    if next.entries.get(path) == Some(&entry) {
                        return Err(TreeError::NoOpPut(path.clone()));
                    }
                    next.entries.insert(path.clone(), entry);
                }
            }
        }
        if next.entries.len() > limits.max_tree_entries {
            return Err(TreeError::TooManyEntries(next.entries.len()));
        }
        for op in ops {
            if let Op::Put { path, .. } = op {
                next.check_path(path)?;
            }
        }
        Ok(next)
    }

    /// Every path that differs between `self` and `target`, in path order. One linear walk over both trees;
    /// only the differing paths are cloned.
    pub fn changes(&self, target: &Tree) -> Vec<Change> {
        let mut out = Vec::new();
        let mut a = self.entries.iter().peekable();
        let mut b = target.entries.iter().peekable();
        loop {
            match (a.peek(), b.peek()) {
                (None, None) => break,
                (Some(_), None) => {
                    let (p, e) = a.next().expect("peeked");
                    out.push(Change { path: p.clone(), before: Some(*e), after: None });
                }
                (None, Some(_)) => {
                    let (p, e) = b.next().expect("peeked");
                    out.push(Change { path: p.clone(), before: None, after: Some(*e) });
                }
                (Some((pa, _)), Some((pb, _))) => match pa.cmp(pb) {
                    std::cmp::Ordering::Less => {
                        let (p, e) = a.next().expect("peeked");
                        out.push(Change { path: p.clone(), before: Some(*e), after: None });
                    }
                    std::cmp::Ordering::Greater => {
                        let (p, e) = b.next().expect("peeked");
                        out.push(Change { path: p.clone(), before: None, after: Some(*e) });
                    }
                    std::cmp::Ordering::Equal => {
                        let ((p, ea), (_, eb)) = (a.next().expect("peeked"), b.next().expect("peeked"));
                        if ea != eb {
                            out.push(Change { path: p.clone(), before: Some(*ea), after: Some(*eb) });
                        }
                    }
                },
            }
        }
        out
    }

    /// The minimal canonical operation list that turns `self` into `target`.
    pub fn diff(&self, target: &Tree) -> Vec<Op> {
        let mut ops = Vec::new();
        let mut a = self.entries.iter().peekable();
        let mut b = target.entries.iter().peekable();
        loop {
            match (a.peek(), b.peek()) {
                (None, None) => break,
                (Some((pa, _)), None) => {
                    ops.push(Op::Delete { path: (*pa).clone() });
                    a.next();
                }
                (None, Some(_)) => {
                    let (p, e) = b.next().expect("peeked");
                    ops.push(put_op(p, e));
                }
                (Some((pa, ea)), Some((pb, eb))) => match pa.cmp(pb) {
                    std::cmp::Ordering::Less => {
                        ops.push(Op::Delete { path: (*pa).clone() });
                        a.next();
                    }
                    std::cmp::Ordering::Greater => {
                        ops.push(put_op(pb, eb));
                        b.next();
                    }
                    std::cmp::Ordering::Equal => {
                        if ea != eb {
                            ops.push(put_op(pb, eb));
                        }
                        a.next();
                        b.next();
                    }
                },
            }
        }
        ops
    }

    /// Lists the immediate children of `dir` (`""` is the root).
    pub fn list_dir(&self, dir: &str) -> Vec<DirEntry> {
        let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
        let mut out: Vec<DirEntry> = Vec::new();
        for (path, entry) in &self.entries {
            let Some(rest) = path.as_str().strip_prefix(prefix.as_str()) else { continue };
            match rest.split_once('/') {
                None => out.push(DirEntry::File { name: rest.to_string(), entry: *entry }),
                Some((sub, _)) => {
                    if !matches!(out.last(), Some(DirEntry::Dir { name }) if name == sub) {
                        out.push(DirEntry::Dir { name: sub.to_string() });
                    }
                }
            }
        }
        out
    }

    /// Canonical manifest bytes.
    ///
    /// ```text
    /// varint n, n * ( path:str file_type:u8 executable:u8 content:fixed32 )
    /// ```
    /// sorted strictly ascending by path.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.varint(self.entries.len() as u64);
        for (path, e) in &self.entries {
            w.str(path.as_str());
            w.u8(e.file_type.tag());
            w.u8(u8::from(e.executable));
            w.fixed32(e.content.as_bytes());
        }
        w.finish()
    }

    /// Strictly decodes a manifest.
    pub fn decode(bytes: &[u8], limits: &Limits) -> Result<Self, TreeError> {
        let mut r = Reader::new(bytes);
        let n = r.bounded_varint(limits.max_tree_entries as u64)? as usize;
        // An entry takes at least 36 bytes, so a hostile count cannot make this allocation large.
        let mut sorted: Vec<(RepoPath, Entry)> = Vec::with_capacity(n.min(bytes.len() / 36 + 1));
        for _ in 0..n {
            let path = RepoPath::new(r.str(ag_types::path_limits::MAX_PATH_LEN)?)
                .map_err(|e| TreeError::Malformed(e.to_string()))?;
            if sorted.last().is_some_and(|(prev, _)| prev >= &path) {
                return Err(TreeError::Malformed("entries not strictly ascending".into()));
            }
            let file_type =
                FileType::from_tag(r.u8()?).ok_or_else(|| TreeError::Malformed("unknown file type".into()))?;
            let executable = match r.u8()? {
                0 => false,
                1 => true,
                t => return Err(CanonError::InvalidTag(t).into()),
            };
            let content = BlobId::new(ObjectId::from_bytes(r.fixed32()?));
            sorted.push((path, Entry { content, file_type, executable }));
        }
        r.finish()?;
        // Already strictly ascending, so collecting builds the map in linear time.
        Self::from_entries(sorted, limits)
    }

    /// The identifier of this exact state.
    pub fn id(&self) -> TreeId {
        TreeId::new(object_id(ObjectKind::Tree, TREE_FORMAT_VERSION, &self.encode()))
    }
}

fn put_op(path: &RepoPath, e: &Entry) -> Op {
    Op::Put { path: path.clone(), content: e.content, file_type: e.file_type, executable: e.executable }
}

/// Optional cache of already derived trees, consulted by [`resolve_tree`].
pub trait TreeCache {
    /// Returns the cached tree of `id`, if any.
    fn cached_tree(&self, id: &ChangesetId) -> Option<Tree>;
}

/// A [`TreeCache`] that never hits.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoCache;

impl TreeCache for NoCache {
    fn cached_tree(&self, _id: &ChangesetId) -> Option<Tree> {
        None
    }
}

/// Derives the tree of `id` from history.
///
/// Walks the first-parent chain until a cached tree or a root is found, then
/// replays the collected operation lists forward. The result is validated at
/// every step, so a corrupt or malicious history cannot yield a malformed tree.
pub fn resolve_tree<S: ChangesetSource + ?Sized, C: TreeCache + ?Sized>(
    source: &S,
    cache: &C,
    id: &ChangesetId,
    limits: &Limits,
) -> Result<Tree, TreeError> {
    let mut chain: Vec<std::sync::Arc<Changeset>> = Vec::new();
    let mut cursor = Some(*id);
    let mut base = Tree::empty();
    while let Some(cur) = cursor {
        if let Some(t) = cache.cached_tree(&cur) {
            base = t;
            break;
        }
        if chain.len() as u64 >= limits.max_dag_depth {
            return Err(TreeError::TooDeep(limits.max_dag_depth));
        }
        let cs = source.changeset(&cur)?;
        cursor = cs.first_parent();
        chain.push(cs);
    }
    for cs in chain.iter().rev() {
        base = base.apply(&cs.ops, limits)?;
    }
    Ok(base)
}

#[cfg(test)]
mod tests;
