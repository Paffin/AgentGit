#![no_main]
//! Three-way tree merge, including rename handling, on generated trees (small alphabets of paths and lines
//! so renames, edits of the same line and file/directory clashes actually happen).
//!
//! Invariants: never panics or errors on complete input; deterministic; the trivial cases hold
//! (`merge(b, x, x) = x`, `merge(b, b, t) = t`, `merge(b, o, b) = o`); conflicts are sorted; the merged tree is
//! a valid tree; conflict-freeness and a clean result are symmetric in `ours`/`theirs`.

use ag_merge::merge_trees;
use ag_tree::Tree;
use ag_types::Limits;
use agentgit_fuzz::{MemIo, arb_edit, arb_tree};
use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let io = MemIo::default();
    let limits = Limits::default();
    let Ok(base) = arb_tree(&mut u, &io, 8) else { return };
    let Ok(ours) = arb_edit(&mut u, &io, &base) else { return };
    let Ok(theirs) = arb_edit(&mut u, &io, &base) else { return };

    let merge = |b: &Tree, o: &Tree, t: &Tree| merge_trees(b, o, t, &io, &limits).expect("all blobs are present");

    let m = merge(&base, &ours, &theirs);
    let again = merge(&base, &ours, &theirs);
    assert_eq!(m.tree, again.tree, "merge must be deterministic");
    assert_eq!(m.conflicts, again.conflicts);
    assert!(m.conflicts.windows(2).all(|w| w[0].path <= w[1].path), "conflicts must be ordered by path");
    assert_eq!(Tree::decode(&m.tree.encode(), &limits).as_ref(), Ok(&m.tree), "merged tree must be valid");

    // Trivial cases.
    let same = merge(&base, &ours, &ours);
    assert!(same.is_clean() && same.tree == ours, "both sides making the same change must merge to it");
    let only_theirs = merge(&base, &base, &theirs);
    assert!(only_theirs.is_clean() && only_theirs.tree == theirs, "ours unchanged: take theirs");
    let only_ours = merge(&base, &ours, &base);
    assert!(only_ours.is_clean() && only_ours.tree == ours, "theirs unchanged: keep ours");

    // Symmetry of conflict status, and of the result when there is none.
    let swapped = merge(&base, &theirs, &ours);
    assert_eq!(m.is_clean(), swapped.is_clean(), "conflict status depends on which side is ours");
    if m.is_clean() {
        assert_eq!(m.tree, swapped.tree, "a clean merge must not depend on the side");
    }
});
