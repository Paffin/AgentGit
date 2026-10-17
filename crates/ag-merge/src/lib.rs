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
//! | situation                              | result                         |
//! |----------------------------------------|--------------------------------|
//! | both regular files, text               | line merge, may conflict       |
//! | both regular files, binary             | conflict                       |
//! | one side deleted, other modified       | conflict                       |
//! | both added different content           | conflict                       |
//! | symlink or type changes on both sides  | conflict                       |
//! | file vs directory at the same path     | conflict                       |
//!
//! The executable bit merges independently of content.
//!
//! **Text** is merged by line. The hunks `base→ours` and `base→theirs` come
//! from [`ag_diff`]. Hunks of the two sides that overlap **or touch** (share
//! a boundary) form a cluster; a cluster is clean only if both sides produced
//! identical text for it. Touching hunks conflict deliberately: the engine
//! cannot know that adjacent edits are semantically independent, and a
//! needless conflict is cheaper than a silent bad merge.
//!
//! Merge does not detect renames in this version; a rename is a delete plus an
//! add, so a rename on one side and an edit on the other conflicts.
//!
//! "Mergeable" never means "correct": a clean merge says only that an
//! unambiguous result tree exists. Validation and approval are separate steps.

mod rename;
mod text;
mod tree;

pub use rename::{
    MAX_SIMILARITY_CANDIDATES, MAX_SIMILARITY_FILE_SIZE, MAX_SIMILARITY_WORK, MIN_SIMILARITY, Rename, detect_renames,
};
pub use text::{ConflictRegion, TextMerge, merge_text, render_conflict_markers};
pub use tree::{BlobIo, Conflict, ConflictKind, ConflictOrigin, IoError, MAX_TEXT_MERGE_SIZE, TreeMerge, merge_trees};

/// Version tag of the merge semantics (includes the diff algorithm).
pub const ALGORITHM_VERSION: &str = "ag-merge/2+ag-diff/1";

#[cfg(test)]
mod tests;
