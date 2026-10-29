//! Bounded candidate-path metadata for explicit structural resolution.
use std::collections::BTreeMap;

use crate::{Conflict, ConflictKind};
use ag_tree::Entry;
use ag_types::RepoPath;

/// Most candidate paths described for one file/directory conflict, including its root.
pub const MAX_RESOLUTION_SCOPE_PATHS: usize = 100;
/// Most candidate paths copied across all structural scopes in one preview.
pub const MAX_RESOLUTION_PATHS_TOTAL: usize = 1000;

/// Candidate paths affected by one structural choice. Incomplete scopes cannot drive a whole-subtree choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolutionScope {
    /// Exact candidate entry paths in path order; a complete scope includes the colliding root.
    pub paths: Vec<RepoPath>,
    /// True only when every candidate entry at or beneath the root is present.
    pub complete: bool,
}

impl ResolutionScope {
    fn include(&mut self, path: &RepoPath, remaining: &mut usize) {
        if self.paths.len() < MAX_RESOLUTION_SCOPE_PATHS && *remaining > 0 {
            self.paths.push(path.clone());
            *remaining -= 1;
        } else {
            self.complete = false;
        }
    }
}

pub(crate) fn collect_scopes<'a>(
    conflicts: &'a [Conflict],
    entries: Option<&BTreeMap<RepoPath, Entry>>,
) -> BTreeMap<&'a str, ResolutionScope> {
    let Some(entries) = entries else {
        return BTreeMap::new();
    };
    let mut scopes: BTreeMap<&str, ResolutionScope> = conflicts
        .iter()
        .filter_map(|conflict| {
            matches!(conflict.kind, ConflictKind::FileDirectory { .. })
                .then_some((conflict.path.as_str(), ResolutionScope { paths: Vec::new(), complete: true }))
        })
        .collect();
    let mut remaining = MAX_RESOLUTION_PATHS_TOTAL;
    // A single candidate scan: look up only its own path and component ancestors.
    // Borrowed root keys avoid allocation for every ancestor of every candidate.
    for path in entries.keys() {
        for root in std::iter::once(path.as_str()).chain(path.ancestors()) {
            if let Some(scope) = scopes.get_mut(root) {
                scope.include(path, &mut remaining);
            }
        }
    }
    scopes
}
