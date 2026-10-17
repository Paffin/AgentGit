//! Line-level blame: which changeset introduced each line of a file.
//!
//! Blame is *derived* from history and recomputed on demand; nothing is stored. The algorithm is the usual
//! one: the lines of the file at the starting revision are the "suspects" of that changeset; a changeset
//! keeps the lines it introduced and hands every line that existed before to a parent.
//!
//! * A changeset whose operations do not mention the path left the file as its first parent had it
//!   (operations are relative to the first parent), so all its lines pass through *without reading any file*.
//! * Otherwise the file is diffed against each parent in order (Myers, [`ag_diff`]); lines that are unchanged
//!   go to the first parent that has them, the rest are blamed on the changeset. A merge changeset therefore
//!   takes the blame for lines it had to write itself (a conflict resolution), not for lines either side
//!   already had.
//! * **Renames are followed** when a changeset creates the path while deleting a similar one (the same
//!   detection the merge uses, [`ag_merge::detect_renames`]); for merge parents beyond the first only the same
//!   path is considered.
//! * The walk visits changesets newest generation first, so a changeset is processed only after all of its
//!   children, and suspects arriving from several children are combined.
//!
//! Limits (all reported, never silent): files over [`MAX_BLAME_FILE_SIZE`], binary files and files with more than
//! [`MAX_BLAME_LINES`] lines are refused; after `max_commits` visited changesets the remaining lines are
//! attributed to the last changeset reached and flagged `boundary` (the result says it is `truncated`).

use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};

use ag_diff::{diff_hunks, is_binary, split_lines};
use ag_merge::detect_renames;
use ag_tree::Tree;
use ag_types::{ChangesetId, FileType, RepoPath, Timestamp};
use serde::Serialize;

use crate::repo::RepoBlobs;
use crate::{Error, Repository, Result};

/// Largest file blame will process.
pub const MAX_BLAME_FILE_SIZE: usize = 4 * 1024 * 1024;
/// Most lines blame will process.
pub const MAX_BLAME_LINES: usize = 100_000;
/// Default bound on visited changesets.
pub const DEFAULT_BLAME_COMMITS: usize = 100_000;

/// Options of [`Repository::blame`].
#[derive(Clone, Debug)]
pub struct BlameOptions {
    /// Visit at most this many changesets.
    pub max_commits: usize,
    /// Only report lines `first..=last` (1-based). `None` = the whole file.
    pub range: Option<(usize, usize)>,
}

impl Default for BlameOptions {
    fn default() -> Self {
        Self { max_commits: DEFAULT_BLAME_COMMITS, range: None }
    }
}

/// The origin of one line.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BlameLine {
    /// Line number in the file at the requested revision (1-based).
    pub line: usize,
    /// The line's text, without its terminator (lossy UTF-8).
    pub text: String,
    /// The changeset that introduced the line.
    pub changeset: ChangesetId,
    /// The path the file had in that changeset (differs from the requested path after a rename).
    pub path: String,
    /// The line's number in that changeset's version of the file (1-based).
    pub original_line: usize,
    /// Author of that changeset.
    pub author: String,
    /// Author time of that changeset.
    pub timestamp: Timestamp,
    /// First line of that changeset's message.
    pub summary: String,
    /// True if the walk stopped before finding the real origin; `changeset` is then the last one reached.
    pub boundary: bool,
}

/// The result of [`Repository::blame`].
#[derive(Clone, Debug, Serialize)]
pub struct Blame {
    /// The revision blamed.
    pub rev: ChangesetId,
    /// The requested path.
    pub path: String,
    /// One entry per line, in file order.
    pub lines: Vec<BlameLine>,
    /// Changesets visited.
    pub visited: usize,
    /// True if `max_commits` was reached before every line found its origin.
    pub truncated: bool,
}

/// Lines (of the file in one changeset) still looking for their origin: `(line index in that file, final index)`.
type Pending = Vec<(usize, usize)>;

impl Repository {
    /// Blames the file at `path` as of `rev`.
    pub fn blame(&self, rev: &ChangesetId, path: &str, opts: &BlameOptions) -> Result<Blame> {
        let path = RepoPath::new(path).map_err(|e| Error::Invalid(format!("bad path '{path}': {e}")))?;
        let tree = self.tree_of(rev)?;
        let entry =
            tree.get(&path).ok_or_else(|| Error::NotFound(format!("'{path}' does not exist at {}", rev.short())))?;
        if entry.file_type != FileType::Regular {
            return Err(Error::Invalid(format!("'{path}' is a symlink, not a file")));
        }
        let data = self.blob(&entry.content)?;
        if data.len() > MAX_BLAME_FILE_SIZE {
            return Err(Error::Invalid(format!(
                "'{path}' is {} bytes; blame handles files up to {MAX_BLAME_FILE_SIZE}",
                data.len()
            )));
        }
        if is_binary(&data) {
            return Err(Error::Invalid(format!("'{path}' is a binary file")));
        }
        let final_lines: Vec<&[u8]> = split_lines(&data);
        if final_lines.len() > MAX_BLAME_LINES {
            return Err(Error::Invalid(format!(
                "'{path}' has {} lines; blame handles up to {MAX_BLAME_LINES}",
                final_lines.len()
            )));
        }
        let wanted: Vec<usize> = match opts.range {
            None => (0..final_lines.len()).collect(),
            Some((first, last)) => {
                if first == 0 || last < first || first > final_lines.len() {
                    return Err(Error::Invalid(format!(
                        "line range {first}..{last} is outside the file ({} lines)",
                        final_lines.len()
                    )));
                }
                (first - 1..last.min(final_lines.len())).collect()
            }
        };

        let g = self.graph();
        // Suspects keyed by (changeset, path); processed newest generation first.
        let mut suspects: HashMap<(ChangesetId, RepoPath), Pending> = HashMap::new();
        let mut heap: BinaryHeap<(u64, ChangesetId, RepoPath)> = BinaryHeap::new();
        let mut queued: HashSet<(ChangesetId, RepoPath)> = HashSet::new();
        let mut enqueue = |suspects: &mut HashMap<(ChangesetId, RepoPath), Pending>,
                           heap: &mut BinaryHeap<(u64, ChangesetId, RepoPath)>,
                           id: ChangesetId,
                           p: RepoPath,
                           lines: Pending|
         -> Result<()> {
            suspects.entry((id, p.clone())).or_default().extend(lines);
            if queued.insert((id, p.clone())) {
                use ag_dag::Graph as _;
                heap.push((g.generation(&id)?, id, p));
            }
            Ok(())
        };
        enqueue(&mut suspects, &mut heap, *rev, path.clone(), wanted.iter().map(|i| (*i, *i)).collect())?;

        let mut origin: BTreeMap<usize, (ChangesetId, RepoPath, usize, bool)> = BTreeMap::new();
        let mut visited = 0usize;
        let mut truncated = false;
        let blobs = RepoBlobs(self);
        while let Some((_, id, cur_path)) = heap.pop() {
            let Some(mut lines) = suspects.remove(&(id, cur_path.clone())) else { continue };
            lines.sort_unstable();
            visited += 1;
            if visited > opts.max_commits {
                truncated = true;
                for (idx, fin) in lines {
                    origin.insert(fin, (id, cur_path.clone(), idx, true));
                }
                // Everything still queued is blamed on the changeset it is waiting at.
                for ((cid, cpath), pending) in suspects.drain() {
                    for (idx, fin) in pending {
                        origin.insert(fin, (cid, cpath.clone(), idx, true));
                    }
                }
                break;
            }
            let cs = self.changeset(&id)?;
            if cs.parents.is_empty() {
                for (idx, fin) in lines {
                    origin.insert(fin, (id, cur_path.clone(), idx, false));
                }
                continue;
            }

            let op = cs.ops.iter().find(|o| o.path() == &cur_path);
            let mut unresolved: Pending = lines;
            // File as it was in this changeset (needed only if it is diffed).
            let mut this_file: Option<Vec<u8>> = None;
            let mut c_tree: Option<Tree> = None;

            for (i, parent) in cs.parents.iter().enumerate() {
                if unresolved.is_empty() {
                    break;
                }
                // First parent, path untouched: the file is exactly the parent's.
                if i == 0 && op.is_none() {
                    let moved = std::mem::take(&mut unresolved);
                    enqueue(&mut suspects, &mut heap, *parent, cur_path.clone(), moved)?;
                    break;
                }
                // Load this changeset's file once.
                if this_file.is_none() {
                    let tree = match c_tree.take() {
                        Some(t) => t,
                        None => self.tree_of(&id)?,
                    };
                    let e = tree
                        .get(&cur_path)
                        .ok_or_else(|| Error::Invalid(format!("'{cur_path}' vanished at {}", id.short())))?;
                    this_file = Some(self.blob(&e.content)?);
                    c_tree = Some(tree);
                }
                let c_data = this_file.as_deref().expect("loaded");
                let c_lines = split_lines(c_data);

                let parent_tree = self.tree_of(parent)?;
                // Where was this file in the parent?
                let mut parent_path = cur_path.clone();
                let mut parent_entry = parent_tree.get(&cur_path).copied();
                if parent_entry.is_none() && i == 0 {
                    // Created here: a rename if a similar file was deleted in the same changeset.
                    let deleted: Vec<_> = cs
                        .ops
                        .iter()
                        .filter_map(|o| match o {
                            ag_changeset::Op::Delete { path } => parent_tree.get(path).map(|e| (path.clone(), *e)),
                            _ => None,
                        })
                        .collect();
                    if !deleted.is_empty() {
                        let old = Tree::from_entries(deleted, &self.limits)?;
                        let new_entry = c_tree.as_ref().and_then(|t| t.get(&cur_path)).copied().expect("present");
                        let new = Tree::from_entries([(cur_path.clone(), new_entry)], &self.limits)?;
                        let found = detect_renames(&old, &new, &blobs).map_err(|e| Error::Invalid(e.to_string()))?;
                        if let Some(r) = found.into_iter().next() {
                            parent_entry = old.get(&r.from).copied();
                            parent_path = r.from;
                        }
                    }
                }
                let Some(pe) = parent_entry.filter(|e| e.file_type == FileType::Regular) else { continue };
                let p_data = self.blob(&pe.content)?;
                if is_binary(&p_data) {
                    continue;
                }
                let p_lines = split_lines(&p_data);
                let unchanged = unchanged_map(&p_lines, &c_lines);
                let mut to_parent: Pending = Vec::new();
                let mut remaining: Pending = Vec::new();
                for (idx, fin) in unresolved {
                    match unchanged.get(idx).copied().flatten() {
                        Some(pidx) => to_parent.push((pidx, fin)),
                        None => remaining.push((idx, fin)),
                    }
                }
                unresolved = remaining;
                if !to_parent.is_empty() {
                    enqueue(&mut suspects, &mut heap, *parent, parent_path, to_parent)?;
                }
            }
            // Whatever no parent had is this changeset's own work.
            for (idx, fin) in unresolved {
                origin.insert(fin, (id, cur_path.clone(), idx, false));
            }
        }

        let mut meta_cache: HashMap<ChangesetId, (String, Timestamp, String)> = HashMap::new();
        let mut out = Vec::with_capacity(wanted.len());
        for fin in wanted {
            let (cid, p, orig, boundary) = origin
                .get(&fin)
                .cloned()
                .ok_or_else(|| Error::Invalid(format!("internal: line {} was never attributed", fin + 1)))?;
            if let std::collections::hash_map::Entry::Vacant(slot) = meta_cache.entry(cid) {
                let cs = self.changeset(&cid)?;
                slot.insert((cs.author.to_string(), cs.timestamp, cs.message.lines().next().unwrap_or("").to_string()));
            }
            let (author, timestamp, summary) = meta_cache[&cid].clone();
            let raw = final_lines[fin];
            let text = String::from_utf8_lossy(raw.strip_suffix(b"\n").unwrap_or(raw)).into_owned();
            out.push(BlameLine {
                line: fin + 1,
                text,
                changeset: cid,
                path: p.to_string(),
                original_line: orig + 1,
                author,
                timestamp,
                summary,
                boundary,
            });
        }
        Ok(Blame { rev: *rev, path: path.to_string(), lines: out, visited, truncated })
    }
}

/// For each line of `new`, the index of the identical line in `old` it corresponds to, or `None` if the line
/// was added or changed (according to the deterministic diff).
fn unchanged_map(old: &[&[u8]], new: &[&[u8]]) -> Vec<Option<usize>> {
    let hunks = diff_hunks(old, new);
    let mut map = vec![None; new.len()];
    let (mut a, mut b) = (0usize, 0usize);
    for h in hunks {
        while b < h.b_start {
            map[b] = Some(a);
            a += 1;
            b += 1;
        }
        a = h.a_end();
        b = h.b_end();
    }
    while b < new.len() {
        map[b] = Some(a);
        a += 1;
        b += 1;
    }
    map
}

impl Repository {
    /// The renames within one changeset: deleted paths paired with similar paths it creates, relative to its
    /// first parent. Used for display; the merge applies the same detection to whole trees.
    pub fn changeset_renames(&self, cs: &ag_changeset::Changeset) -> Result<Vec<ag_merge::Rename>> {
        let Some(parent) = cs.first_parent() else { return Ok(Vec::new()) };
        let deleted: Vec<_> =
            cs.ops.iter().filter(|o| matches!(o, ag_changeset::Op::Delete { .. })).map(|o| o.path().clone()).collect();
        let created: Vec<_> = cs
            .ops
            .iter()
            .filter_map(|o| match o {
                ag_changeset::Op::Put { path, content, file_type, executable } => {
                    Some((path.clone(), *content, *file_type, *executable))
                }
                _ => None,
            })
            .collect();
        if deleted.is_empty() || created.is_empty() {
            return Ok(Vec::new());
        }
        let parent_tree = self.tree_of(&parent)?;
        let old = Tree::from_entries(
            deleted.iter().filter_map(|p| parent_tree.get(p).map(|e| (p.clone(), *e))),
            &self.limits,
        )?;
        let new = Tree::from_entries(
            created
                .into_iter()
                .filter(|(p, ..)| parent_tree.get(p).is_none())
                .map(|(p, content, file_type, executable)| (p, ag_tree::Entry { content, file_type, executable })),
            &self.limits,
        )?;
        detect_renames(&old, &new, &RepoBlobs(self)).map_err(|e| Error::Invalid(e.to_string()))
    }
}
