//! Deterministic rename detection between two trees.
//!
//! A *rename* is a pairing of a path that exists only in the old tree with a path that exists only in the
//! new tree, whose contents are the same file (identical, or similar enough). The result is a pure function
//! of the two trees and the blob contents: no clocks, no hash-map iteration order, no randomness.
//!
//! # Rules
//!
//! * Only regular files take part (a symlink is its target string; it is never "renamed").
//! * **Exact** renames (identical content) are found first and always win over similar ones.
//! * **Similar** renames need a similarity of at least [`MIN_SIMILARITY`] percent, measured as
//!   `2 × bytes of common lines / (bytes of old + bytes of new)` over the multiset of lines. Binary files and
//!   files above [`MAX_SIMILARITY_FILE_SIZE`] only match exactly.
//! * Candidate pairs are ordered by (similarity ↓, same file name first, old path ↑, new path ↑) and chosen
//!   greedily; a path is used at most once, so `a → b` and `a → c` can never both be reported.
//! * Work is bounded: with more than [`MAX_SIMILARITY_CANDIDATES`] deleted or added files on either side, or
//!   when the comparison budget [`MAX_SIMILARITY_WORK`] would be exceeded, only exact renames are reported.
//!   The budget depends on the inputs alone, so the result stays deterministic.
//!
//! Directory renames are *not* detected as such; they appear as one rename per file.

use std::collections::{BTreeMap, HashMap};

use ag_diff::{is_binary, split_lines};
use ag_tree::{Entry, Tree};
use ag_types::{BlobId, FileType, RepoPath};

use crate::tree::{BlobIo, IoError};

/// Minimum similarity, in percent, for a non-identical file to count as a rename.
pub const MIN_SIMILARITY: u32 = 60;
/// Largest file (bytes) considered for similarity matching.
pub const MAX_SIMILARITY_FILE_SIZE: usize = 256 * 1024;
/// Most deleted (and most added) files for which similarity matching is attempted.
pub const MAX_SIMILARITY_CANDIDATES: usize = 200;
/// Most line comparisons (sum over pairs of both files' line counts) spent on similarity matching.
pub const MAX_SIMILARITY_WORK: usize = 40_000_000;

/// One detected rename.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rename {
    /// Path in the old tree.
    pub from: RepoPath,
    /// Path in the new tree.
    pub to: RepoPath,
    /// 100 for identical content, otherwise the similarity in percent.
    pub similarity: u32,
}

/// Finds the renames that turn `old` into `new`, ordered by `from`.
pub fn detect_renames(old: &Tree, new: &Tree, io: &dyn BlobIo) -> Result<Vec<Rename>, IoError> {
    let deleted: Vec<(&RepoPath, &Entry)> =
        old.iter().filter(|(p, e)| e.file_type == FileType::Regular && new.get(p).is_none()).collect();
    let added: Vec<(&RepoPath, &Entry)> =
        new.iter().filter(|(p, e)| e.file_type == FileType::Regular && old.get(p).is_none()).collect();
    if deleted.is_empty() || added.is_empty() {
        return Ok(Vec::new());
    }

    let mut used_from: BTreeMap<&RepoPath, ()> = BTreeMap::new();
    let mut used_to: BTreeMap<&RepoPath, ()> = BTreeMap::new();
    let mut found: Vec<Rename> = Vec::new();

    // 1. Exact: identical content (and, for copies, the pairing is by file name then path order).
    let mut exact: Vec<(&RepoPath, &RepoPath)> = Vec::new();
    let mut by_content: HashMap<BlobId, Vec<&RepoPath>> = HashMap::new();
    for (p, e) in &added {
        by_content.entry(e.content).or_default().push(p);
    }
    for (from, e) in &deleted {
        if let Some(tos) = by_content.get(&e.content) {
            for to in tos {
                exact.push((from, to));
            }
        }
    }
    exact.sort_by(|(af, at), (bf, bt)| pair_order(af, at, bf, bt));
    for (from, to) in exact {
        if used_from.contains_key(from) || used_to.contains_key(to) {
            continue;
        }
        used_from.insert(from, ());
        used_to.insert(to, ());
        found.push(Rename { from: from.clone(), to: to.clone(), similarity: 100 });
    }

    // 2. Similar content, within a bounded budget.
    let left: Vec<(&RepoPath, &Entry)> = deleted.iter().filter(|(p, _)| !used_from.contains_key(p)).copied().collect();
    let right: Vec<(&RepoPath, &Entry)> = added.iter().filter(|(p, _)| !used_to.contains_key(p)).copied().collect();
    if !left.is_empty()
        && !right.is_empty()
        && left.len() <= MAX_SIMILARITY_CANDIDATES
        && right.len() <= MAX_SIMILARITY_CANDIDATES
    {
        let a = signatures(&left, io)?;
        let b = signatures(&right, io)?;
        let work: usize = a
            .iter()
            .flatten()
            .map(|s| s.lines.len())
            .sum::<usize>()
            .saturating_mul(b.iter().flatten().count().max(1))
            .saturating_add(
                b.iter()
                    .flatten()
                    .map(|s| s.lines.len())
                    .sum::<usize>()
                    .saturating_mul(a.iter().flatten().count().max(1)),
            );
        if work <= MAX_SIMILARITY_WORK {
            let mut scored: Vec<(u32, &RepoPath, &RepoPath)> = Vec::new();
            for (i, (from, _)) in left.iter().enumerate() {
                let Some(sa) = &a[i] else { continue };
                for (j, (to, _)) in right.iter().enumerate() {
                    let Some(sb) = &b[j] else { continue };
                    // The best possible score is bounded by the size ratio; skip hopeless pairs cheaply.
                    let (lo, hi) = (sa.bytes.min(sb.bytes), sa.bytes.max(sb.bytes));
                    if hi == 0 || (200 * lo / (lo + hi)) < MIN_SIMILARITY as usize {
                        continue;
                    }
                    let score = similarity(sa, sb);
                    if score >= MIN_SIMILARITY {
                        scored.push((score, from, to));
                    }
                }
            }
            scored.sort_by(|(s1, f1, t1), (s2, f2, t2)| s2.cmp(s1).then_with(|| pair_order(f1, t1, f2, t2)));
            for (score, from, to) in scored {
                if used_from.contains_key(from) || used_to.contains_key(to) {
                    continue;
                }
                used_from.insert(from, ());
                used_to.insert(to, ());
                found.push(Rename { from: from.clone(), to: to.clone(), similarity: score });
            }
        }
    }
    found.sort_by(|a, b| a.from.cmp(&b.from));
    Ok(found)
}

/// Orders candidate pairs: same file name first, then by old path, then by new path.
fn pair_order(af: &RepoPath, at: &RepoPath, bf: &RepoPath, bt: &RepoPath) -> std::cmp::Ordering {
    let a_same = af.file_name() == at.file_name();
    let b_same = bf.file_name() == bt.file_name();
    b_same.cmp(&a_same).then_with(|| af.cmp(bf)).then_with(|| at.cmp(bt))
}

struct Signature {
    /// Sorted (line hash, line length) multiset.
    lines: Vec<(u64, u32)>,
    bytes: usize,
}

/// Line signatures of each file; `None` for files that may only match exactly.
fn signatures(files: &[(&RepoPath, &Entry)], io: &dyn BlobIo) -> Result<Vec<Option<Signature>>, IoError> {
    files
        .iter()
        .map(|(_, e)| {
            let data = io.read_blob(&e.content)?;
            if data.len() > MAX_SIMILARITY_FILE_SIZE || is_binary(&data) || data.is_empty() {
                return Ok(None);
            }
            let mut lines: Vec<(u64, u32)> =
                split_lines(&data).into_iter().map(|l| (fnv1a(l), l.len() as u32)).collect();
            lines.sort_unstable();
            Ok(Some(Signature { lines, bytes: data.len() }))
        })
        .collect()
}

/// FNV-1a: fixed, dependency-free and identical on every platform, as determinism requires.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// `2 × common bytes / (bytes a + bytes b)` in percent, over the multiset of lines.
fn similarity(a: &Signature, b: &Signature) -> u32 {
    let (mut i, mut j, mut common) = (0, 0, 0usize);
    while i < a.lines.len() && j < b.lines.len() {
        match a.lines[i].cmp(&b.lines[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                common += a.lines[i].1 as usize;
                i += 1;
                j += 1;
            }
        }
    }
    ((200 * common) / (a.bytes + b.bytes)) as u32
}
