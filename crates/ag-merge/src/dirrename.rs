//! Directory-rename detection for the tree merge (`ag-merge/3`).
//!
//! A *directory rename* on one side is derived from the file renames [`crate::detect_renames_in`] found on that
//! side, never from names alone. A directory `D` of the merge base counts as renamed to `E` when
//!
//! * `D` is **vacated**: every base entry beneath it is gone on that side and nothing was added beneath it,
//! * more than half of its base entries were renamed (exact or similar content) to the *same relative path* beneath
//!   `E`, and
//! * neither directory lies inside the other.
//!
//! A vacated directory that files left for several targets, or for one target with half or fewer of the files,
//! is **ambiguous**; a vacated directory that merely lost its files has no rename at all. For a path the other
//! side added, the *deepest vacated ancestor* decides, so a nested directory that went somewhere else than its
//! parent is handled on its own terms. The caller turns "moved" into a relocation, "ambiguous" into a conflict
//! and everything else into the version-2 behaviour (the file stays where it was added).
//!
//! The analysis is linear in the changed paths plus one pass over the base tree (only when some file was renamed),
//! and gives up (no directory renames for that side) beyond [`MAX_DIRECTORY_PAIRS`] distinct (old, new) directory
//! pairs, a property of the inputs alone. No output depends on hash-map iteration order.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ag_tree::{Change, Tree};
use ag_types::{FileType, RepoPath};

/// Most distinct (old directory, new directory) pairs examined per side; beyond it that side's directory renames
/// are not detected (file renames still are).
pub const MAX_DIRECTORY_PAIRS: usize = 50_000;
/// Most candidate targets named in an ambiguity conflict.
pub const MAX_AMBIGUOUS_CANDIDATES: usize = 8;

/// What became of a vacated directory.
#[derive(Clone, Debug)]
pub(crate) enum Decision {
    /// Moved to `to`; `votes` of its `total` base entries went there with their relative paths intact.
    Moved { to: String, votes: usize, total: usize },
    /// Files left for several places, or too few of them left for one: no guess.
    Ambiguous { candidates: Vec<String> },
    /// Vacated with no file moved anywhere: a deletion, not a rename.
    Plain,
}

/// Directory decisions of one side relative to the base.
#[derive(Default)]
pub(crate) struct Analysis {
    dirs: HashMap<String, Decision>,
}

/// A base file whose new location follows from its directory's rename, although its content did not match.
pub(crate) struct DirPair {
    pub from: RepoPath,
    pub to: RepoPath,
    pub dir: String,
}

fn nested(a: &str, b: &str) -> bool {
    let inside = |outer: &str, inner: &str| {
        inner.len() > outer.len() && inner.starts_with(outer) && inner.as_bytes()[outer.len()] == b'/'
    };
    inside(a, b) || inside(b, a)
}

impl Analysis {
    /// Analyses one side. `changes` are base→side; `ren` the file renames found in them; `probes` the paths the
    /// *other* side added (their ancestors are the directories whose fate is asked about).
    pub(crate) fn of(
        base: &Tree,
        changes: &[Change],
        ren: &BTreeMap<RepoPath, RepoPath>,
        probes: &[&RepoPath],
    ) -> Analysis {
        let mut votes: HashMap<(String, String), usize> = HashMap::new();
        for (from, to) in ren {
            let f: Vec<&str> = from.components().collect();
            let t: Vec<&str> = to.components().collect();
            let common = f.iter().rev().zip(t.iter().rev()).take_while(|(a, b)| a == b).count();
            for j in 1..=common {
                if f.len() <= j || t.len() <= j {
                    break;
                }
                let d = f[..f.len() - j].join("/");
                let e = t[..t.len() - j].join("/");
                if nested(&d, &e) {
                    continue;
                }
                *votes.entry((d, e)).or_default() += 1;
                if votes.len() > MAX_DIRECTORY_PAIRS {
                    return Analysis::default();
                }
            }
        }
        if votes.is_empty() {
            return Analysis::default();
        }

        // [base entries beneath, deleted beneath, added beneath] for every directory we may be asked about.
        let mut counts: HashMap<String, [usize; 3]> = HashMap::new();
        for (d, _) in votes.keys() {
            counts.entry(d.clone()).or_default();
        }
        for p in probes {
            for a in p.ancestors() {
                if !counts.contains_key(a) {
                    counts.insert(a.to_string(), [0; 3]);
                }
            }
        }
        for (path, _) in base.iter() {
            for a in path.ancestors() {
                if let Some(c) = counts.get_mut(a) {
                    c[0] += 1;
                }
            }
        }
        for ch in changes {
            let slot = match (&ch.before, &ch.after) {
                (Some(_), None) => 1,
                (None, Some(_)) => 2,
                _ => continue,
            };
            for a in ch.path.ancestors() {
                if let Some(c) = counts.get_mut(a) {
                    c[slot] += 1;
                }
            }
        }

        let mut by_dir: HashMap<&str, Vec<(&str, usize)>> = HashMap::new();
        for ((d, e), n) in &votes {
            by_dir.entry(d.as_str()).or_default().push((e.as_str(), *n));
        }
        let mut dirs = HashMap::new();
        for (d, c) in &counts {
            let [total, deleted, added] = *c;
            if total == 0 || deleted != total || added != 0 {
                continue; // not vacated
            }
            let decision = match by_dir.get(d.as_str()) {
                None => Decision::Plain,
                Some(targets) => {
                    // The target with most votes; a tie (never a majority) is broken by name for determinism.
                    let (best, n) = targets
                        .iter()
                        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
                        .copied()
                        .expect("a directory with votes has a target");
                    if n * 2 > total {
                        Decision::Moved { to: best.to_string(), votes: n, total }
                    } else {
                        let mut candidates: Vec<String> = targets.iter().map(|(e, _)| e.to_string()).collect();
                        candidates.sort();
                        candidates.truncate(MAX_AMBIGUOUS_CANDIDATES);
                        Decision::Ambiguous { candidates }
                    }
                }
            };
            dirs.insert(d.clone(), decision);
        }
        Analysis { dirs }
    }

    /// The deepest vacated ancestor directory of `path` and what became of it.
    pub(crate) fn decide(&self, path: &RepoPath) -> Option<(&str, &Decision)> {
        if self.dirs.is_empty() {
            return None;
        }
        let ancestors: Vec<&str> = path.ancestors().collect();
        ancestors.into_iter().rev().find_map(|a| self.dirs.get_key_value(a).map(|(k, v)| (k.as_str(), v)))
    }

    /// The votes and size of a moved directory, for the explanation.
    pub(crate) fn moved(&self, dir: &str) -> Option<(&str, usize, usize)> {
        match self.dirs.get(dir) {
            Some(Decision::Moved { to, votes, total }) => Some((to.as_str(), *votes, *total)),
            _ => None,
        }
    }

    /// Base files this side deleted from a moved directory, whose counterpart it added at the same relative path
    /// and which the other side **modified** (`other` is the other side's tree). Their content did not match (the
    /// file was rewritten while moving), but the directory's move says where they went.
    pub(crate) fn pairs(
        &self,
        base: &Tree,
        side: &Tree,
        other: &Tree,
        changes: &[Change],
        ren: &BTreeMap<RepoPath, RepoPath>,
    ) -> Vec<DirPair> {
        if self.dirs.is_empty() {
            return Vec::new();
        }
        let mut taken: BTreeSet<RepoPath> = ren.values().cloned().collect();
        let mut out = Vec::new();
        for ch in changes {
            let (Some(b), None) = (&ch.before, &ch.after) else { continue };
            if b.file_type != FileType::Regular || ren.contains_key(&ch.path) {
                continue;
            }
            let Some((dir, Decision::Moved { to, .. })) = self.decide(&ch.path) else { continue };
            // The other side must have changed this very file; files it left alone just follow their side.
            if !other.get(&ch.path).is_some_and(|o| o != b) {
                continue;
            }
            let Ok(target) = RepoPath::new(format!("{to}{}", &ch.path.as_str()[dir.len()..])) else { continue };
            let regular_new = side.get(&target).is_some_and(|e| e.file_type == FileType::Regular);
            if !regular_new || base.get(&target).is_some() || taken.contains(&target) {
                continue;
            }
            taken.insert(target.clone());
            out.push(DirPair { from: ch.path.clone(), to: target, dir: dir.to_string() });
        }
        out
    }
}
