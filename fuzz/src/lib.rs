//! Shared helpers for the fuzz targets: an in-memory blob store and generators for small, collision-prone
//! trees. The generators draw paths and contents from tiny alphabets on purpose: a fuzzer that has to guess
//! a 32-byte hash never finds a rename, a file/directory clash or two sides touching the same line.

#[cfg(feature = "heavy")]
pub mod agwp_types;

use std::cell::RefCell;
use std::collections::HashMap;

use arbitrary::{Result as ArbResult, Unstructured};

use ag_merge::{BlobIo, IoError};
use ag_object::hash_blob;
use ag_tree::{Entry, Tree};
use ag_types::{BlobId, FileType, Limits, RepoPath};

/// Blobs in memory. Content addressed, so writing the same bytes twice is harmless.
#[derive(Default)]
pub struct MemIo {
    blobs: RefCell<HashMap<BlobId, Vec<u8>>>,
}

impl MemIo {
    /// Stores `data` and returns its id.
    pub fn put(&self, data: &[u8]) -> BlobId {
        let id = hash_blob(data);
        self.blobs.borrow_mut().entry(id).or_insert_with(|| data.to_vec());
        id
    }

    /// Bytes of a blob, if present.
    pub fn get(&self, id: &BlobId) -> Option<Vec<u8>> {
        self.blobs.borrow().get(id).cloned()
    }
}

impl BlobIo for MemIo {
    fn read_blob(&self, id: &BlobId) -> Result<Vec<u8>, IoError> {
        self.get(id).ok_or_else(|| format!("blob {id} not in the fuzz store").into())
    }

    fn write_blob(&self, data: Vec<u8>) -> Result<BlobId, IoError> {
        Ok(self.put(&data))
    }
}

/// Path components: a few names that collide often, one that is a prefix of another, and a deep chain.
const NAMES: &[&str] = &["a", "b", "c", "dir", "dir2", "x.rs", "y.txt", "lib", "README"];

/// A valid repository path of one to three components.
pub fn arb_path(u: &mut Unstructured<'_>) -> ArbResult<RepoPath> {
    let depth = u.int_in_range(1..=3)?;
    let mut parts = Vec::with_capacity(depth);
    for _ in 0..depth {
        parts.push(*u.choose(NAMES)?);
    }
    Ok(RepoPath::new(parts.join("/")).expect("generated paths are valid"))
}

/// File content made of a handful of short lines, so that two sides often edit the same line, and
/// sometimes binary or without a trailing newline.
pub fn arb_content(u: &mut Unstructured<'_>) -> ArbResult<Vec<u8>> {
    const LINES: &[&str] = &["one", "two", "three", "four", "five", "fn main() {}", "}", ""];
    let n = u.int_in_range(0..=12)?;
    let mut out = Vec::new();
    for _ in 0..n {
        out.extend_from_slice(u.choose(LINES)?.as_bytes());
        out.push(b'\n');
    }
    match u.int_in_range(0..=9)? {
        0 => {
            out.pop();
        }
        1 => out.push(0),
        _ => {}
    }
    Ok(out)
}

/// A tree of up to `max` entries whose blobs live in `io`. Entries that would break the file/directory
/// rule are dropped, so the result is always a valid tree.
pub fn arb_tree(u: &mut Unstructured<'_>, io: &MemIo, max: usize) -> ArbResult<Tree> {
    let n = u.int_in_range(0..=max)?;
    let mut tree = Tree::empty();
    let limits = Limits::default();
    for _ in 0..n {
        let path = arb_path(u)?;
        let (file_type, data) = if u.int_in_range(0..=7)? == 0 {
            (FileType::Symlink, u.choose(&["a", "dir/b", "../x", "/etc/passwd", "x.rs"])?.as_bytes().to_vec())
        } else {
            (FileType::Regular, arb_content(u)?)
        };
        let executable = file_type == FileType::Regular && u.int_in_range(0..=5)? == 0;
        let entry = Entry { content: io.put(&data), file_type, executable };
        let ops = [ag_changeset::Op::Put { path, content: entry.content, file_type, executable }];
        if let Ok(next) = tree.apply(&ops, &limits) {
            tree = next;
        }
    }
    Ok(tree)
}

/// A variation of `base`: drops, edits, renames and adds a few entries, the way a real change would.
pub fn arb_edit(u: &mut Unstructured<'_>, io: &MemIo, base: &Tree) -> ArbResult<Tree> {
    let limits = Limits::default();
    let mut tree = base.clone();
    let rounds = u.int_in_range(0..=6)?;
    for _ in 0..rounds {
        let existing: Vec<(RepoPath, Entry)> = tree.iter().map(|(p, e)| (p.clone(), *e)).collect();
        let op = match u.int_in_range(0..=3)? {
            // delete
            0 if !existing.is_empty() => {
                let (p, _) = u.choose(&existing)?;
                vec![ag_changeset::Op::Delete { path: p.clone() }]
            }
            // edit the content of an existing regular file
            1 if !existing.is_empty() => {
                let (p, e) = u.choose(&existing)?.clone();
                if e.file_type != FileType::Regular {
                    continue;
                }
                let mut data = io.get(&e.content).unwrap_or_default();
                if u.arbitrary()? {
                    data.extend_from_slice(b"edit\n");
                } else if let Some(i) = data.iter().position(|b| *b == b'\n') {
                    data.splice(..=i, b"changed\n".iter().copied());
                }
                vec![ag_changeset::Op::Put {
                    path: p,
                    content: io.put(&data),
                    file_type: FileType::Regular,
                    executable: e.executable,
                }]
            }
            // rename (possibly with a small edit)
            2 if !existing.is_empty() => {
                let (from, e) = u.choose(&existing)?.clone();
                let to = arb_path(u)?;
                if to == from {
                    continue;
                }
                let mut ops = vec![
                    ag_changeset::Op::Delete { path: from.clone() },
                    ag_changeset::Op::Put {
                        path: to.clone(),
                        content: e.content,
                        file_type: e.file_type,
                        executable: e.executable,
                    },
                ];
                ops.sort_by(|a, b| a.path().cmp(b.path()));
                ops
            }
            // add
            _ => {
                let p = arb_path(u)?;
                let data = arb_content(u)?;
                vec![ag_changeset::Op::Put {
                    path: p,
                    content: io.put(&data),
                    file_type: FileType::Regular,
                    executable: false,
                }]
            }
        };
        if let Ok(next) = tree.apply(&op, &limits) {
            tree = next;
        }
    }
    Ok(tree)
}
