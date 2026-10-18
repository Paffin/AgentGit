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
use ag_tree::{Change, Entry, Tree};
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
    detect_renames_in(&old.changes(new), io)
}

/// Like [`detect_renames`], from the list of paths that differ (see [`Tree::changes`]). Deleted paths are
/// those with no `after`; added paths those with no `before`.
pub fn detect_renames_in(changes: &[Change], io: &dyn BlobIo) -> Result<Vec<Rename>, IoError> {
    let regular = |e: &Entry| e.file_type == FileType::Regular;
    let deleted: Vec<(&RepoPath, &Entry)> = changes
        .iter()
        .filter_map(|c| match (&c.before, &c.after) {
            (Some(e), None) if regular(e) => Some((&c.path, e)),
            _ => None,
        })
        .collect();
    let added: Vec<(&RepoPath, &Entry)> = changes
        .iter()
        .filter_map(|c| match (&c.before, &c.after) {
            (None, Some(e)) if regular(e) => Some((&c.path, e)),
            _ => None,
        })
        .collect();
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
                    // Skip hopeless pairs cheaply: their best possible score is below the threshold.
                    if similarity_ceiling(sa.bytes, sb.bytes) < MIN_SIMILARITY {
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
            Ok(Some(signature(&data)))
        })
        .collect()
}

/// The sorted multiset of a file's lines, each as (hash, length).
fn signature(data: &[u8]) -> Signature {
    let mut lines: Vec<(u64, u32)> = split_lines(data).into_iter().map(|l| (fnv1a(l), l.len() as u32)).collect();
    lines.sort_unstable();
    Signature { lines, bytes: data.len() }
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

/// The highest [`similarity`] two files of these sizes (both above zero: empty files never get a signature) can
/// reach, in percent: they cannot have more bytes in common than the smaller one has.
fn similarity_ceiling(a_bytes: usize, b_bytes: usize) -> u32 {
    let (lo, hi) = (a_bytes.min(b_bytes), a_bytes.max(b_bytes));
    (200 * lo / (lo + hi)) as u32
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The output of the FNV-1a (64 bit) algorithm for these inputs, computed from its definition (offset basis
    /// 0xcbf29ce484222325, prime 0x100000001b3, xor then multiply) independently of this code. The line hashes are
    /// part of what makes a rename search the same everywhere, so they are pinned.
    #[test]
    fn fnv1a_is_the_fnv_1a_64_bit_hash() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
        assert_eq!(fnv1a(b"hello\n"), 0xa9bc_80cc_a21f_28b3);
    }

    #[test]
    fn the_ceiling_is_the_share_of_the_larger_file_that_the_smaller_one_can_cover() {
        // 2 * smaller / (smaller + larger), in whole percent, whichever file is which
        assert_eq!(similarity_ceiling(5, 5), 100);
        assert_eq!(similarity_ceiling(3, 7), 60);
        assert_eq!(similarity_ceiling(7, 3), 60);
        assert_eq!(similarity_ceiling(2, 3), 80);
        assert_eq!(similarity_ceiling(1, 99), 2);
        assert_eq!(similarity_ceiling(100, 1), 1);
    }

    #[test]
    fn the_ceiling_is_never_below_what_two_files_actually_score() {
        // a pair is skipped when its ceiling is under the threshold, so the ceiling must not be an underestimate
        let texts = ["a\n", "a\nb\n", "a\nb\nc\n", "b\nc\nd\ne\n", "a\nb\nc\nd\ne\nf\ng\n", "zzzzzzzz\n"];
        for x in texts {
            for y in texts {
                let (a, b) = (signature(x.as_bytes()), signature(y.as_bytes()));
                assert!(similarity(&a, &b) <= similarity_ceiling(a.bytes, b.bytes), "{x:?} {y:?}");
            }
        }
    }

    #[test]
    fn similarity_counts_each_line_as_often_as_both_files_have_it() {
        let sim = |a: &str, b: &str| similarity(&signature(a.as_bytes()), &signature(b.as_bytes()));
        // identical files, and files with nothing in common
        assert_eq!(sim("x\ny\n", "x\ny\n"), 100);
        assert_eq!(sim("x\ny\n", "p\nq\n"), 0);
        // a line the other file has once counts once, however often this one has it ...
        assert_eq!(sim("x\ny\n", "x\nx\nz\n"), 40); // 2 * 2 / (4 + 6)
        assert_eq!(sim("x\nx\nz\n", "x\ny\n"), 40);
        // ... and twice when both have it twice
        assert_eq!(sim("x\nx\n", "x\nx\nz\n"), 80); // 2 * 4 / (4 + 6)
        // the order of the lines does not matter
        assert_eq!(sim("a\nb\nc\n", "c\na\nb\n"), 100);
        // bytes, not lines: the long line they share outweighs the short ones that differ
        assert_eq!(sim("0123456789\na\n", "0123456789\nb\n"), 84); // 2 * 11 / 26
    }

    #[test]
    fn a_signature_is_the_sorted_lines_with_their_lengths() {
        let s = signature(b"b\nlonger line\nb\nlast");
        assert_eq!(s.bytes, 20);
        assert_eq!(s.lines.len(), 4);
        assert!(s.lines.windows(2).all(|w| w[0] <= w[1]));
        let mut lengths: Vec<u32> = s.lines.iter().map(|l| l.1).collect();
        lengths.sort_unstable();
        assert_eq!(lengths, vec![2, 2, 4, 12]);
        assert_eq!(s.lines.iter().filter(|l| l.0 == fnv1a(b"b\n")).count(), 2);
    }
}
