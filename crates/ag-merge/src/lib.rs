//! Deterministic three-way merge of text and of trees.
//!
//! Merge is a *pure function* of its three inputs and the algorithm version
//! ([`ALGORITHM_VERSION`]): the same base/ours/theirs always produce the same
//! result, so integration decisions can be reproduced and audited.
//!
//! # Semantics
//!
//! **Trees** are merged path by path with the usual three-way rule
//! (`ours == theirs` → that; `ours == base` → theirs; `theirs == base` →
//! ours). When both sides changed a path differently:
//!
//! | situation                                          | result                   |
//! |----------------------------------------------------|--------------------------|
//! | both regular files, text                           | line merge, may conflict |
//! | both regular files, binary                         | conflict                 |
//! | one side deleted, other modified                   | conflict                 |
//! | both added different content                       | conflict                 |
//! | symlink or type changes on both sides              | conflict                 |
//! | file vs directory at the same path                 | conflict                 |
//! | file over [`MAX_TEXT_MERGE_SIZE`], edited on both  | `TooLarge` conflict      |
//! | both sides moved a file to different paths         | `RenameRename` conflict  |
//! | one side moved a file, the other deleted it        | `RenameDelete` conflict  |
//! | file added beneath a directory the other side emptied ambiguously, or onto a taken path | `DirectoryRename*` conflict |
//!
//! The executable bit merges independently of content.
//!
//! A merged tree that would exceed the entry limit of [`ag_types::Limits`] is not a conflict (no path is at
//! fault) and never a clean result either: [`merge_trees`] returns an error.
//!
//! **Text** is merged by line. The hunks `base→ours` and `base→theirs` come
//! from [`ag_diff`]. Hunks of the two sides that overlap **or touch** (share
//! a boundary) form a cluster; a cluster is clean only if both sides produced
//! identical text for it. Touching hunks conflict deliberately: the engine
//! cannot know that adjacent edits are semantically independent, and a
//! needless conflict is cheaper than a silent bad merge.
//!
//! Since `ag-merge/2` the merge is **rename-aware**: a file that exists only in the base is paired with a
//! file that exists only in one side, first by identical bytes and then by line similarity (see
//! [`detect_renames`] and the [`MIN_SIMILARITY`] and `MAX_SIMILARITY_*` bounds). When one side moved a file and
//! the other edited it, the edit is merged at the new path. Copies are not tracked (a copy is an add). Under `ag-merge/2` a
//! directory rename was one rename per file. Since `ag-merge/3` a directory that one side vacated by
//! moving most of its files (same relative paths) to one new place takes the other side's additions and edits
//! along; ambiguous cases are conflicts and the move is reported as a [`MergeNote`]. [`Algorithm`] selects the
//! version, so merges recorded under `ag-merge/2` are still reproduced exactly.
//!
//! "Mergeable" never means "correct": a clean merge says only that an
//! unambiguous result tree exists. Validation and approval are separate steps.

mod dirrename;
mod rename;
mod text;
mod tree;

pub use dirrename::{MAX_AMBIGUOUS_CANDIDATES, MAX_DIRECTORY_PAIRS};

pub use rename::{
    MAX_COPY_SOURCES, MAX_SIMILARITY_CANDIDATES, MAX_SIMILARITY_FILE_SIZE, MAX_SIMILARITY_WORK, MIN_COPY_SIMILARITY,
    MIN_SIMILARITY, Rename, detect_copies, detect_renames, detect_renames_in,
};
pub use text::{ConflictRegion, TextMerge, merge_text, render_conflict_markers};
pub use tree::{
    BlobIo, Conflict, ConflictKind, ConflictOrigin, IoError, MAX_TEXT_MERGE_SIZE, MergeNote, Side, TreeMerge,
    merge_trees, merge_trees_with,
};

/// Version tag of the merge semantics (includes the diff algorithm). Written into the message of every merge
/// changeset; [`Algorithm::parse`] maps a recorded tag back to the algorithm that produced it.
pub const ALGORITHM_VERSION: &str = "ag-merge/3+ag-diff/1";

/// The tag of the previous merge semantics (rename-aware, no directory renames): merges recorded with it are
/// reproduced by [`Algorithm::V2`].
pub const ALGORITHM_VERSION_V2: &str = "ag-merge/2+ag-diff/1";

/// Most paths a [`MergeNote`] lists.
pub const MAX_NOTE_PATHS: usize = 100;

/// A version of the merge semantics. Every version stays runnable, so a recorded merge can be verified by running
/// the version it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Algorithm {
    /// `ag-merge/2`: renames of single files ([ADR-0005](../../docs/adr/0005-rename-aware-merge.md)).
    V2,
    /// `ag-merge/3`: V2 plus directory renames ([ADR-0013](../../docs/adr/0013-directory-renames.md)).
    V3,
}

impl Algorithm {
    /// The version new merges are computed (and recorded) with.
    pub const CURRENT: Algorithm = Algorithm::V3;

    /// The tag recorded in merge changesets.
    pub const fn version(self) -> &'static str {
        match self {
            Algorithm::V2 => ALGORITHM_VERSION_V2,
            Algorithm::V3 => ALGORITHM_VERSION,
        }
    }

    /// The algorithm a recorded tag (`Merge-Algorithm: ...`) names, or `None` for a tag this build does not know.
    pub fn parse(tag: &str) -> Option<Algorithm> {
        [Algorithm::V2, Algorithm::V3].into_iter().find(|a| a.version() == tag)
    }

    /// True if files added beneath a renamed directory follow the rename.
    pub const fn follows_directory_renames(self) -> bool {
        matches!(self, Algorithm::V3)
    }
}

#[cfg(test)]
mod tests;
