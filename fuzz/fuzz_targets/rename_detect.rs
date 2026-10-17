#![no_main]
//! Rename detection between two generated trees.
//!
//! Invariants: deterministic; every `from` exists only in the old tree and every `to` only in the new one;
//! each path is used at most once; exact renames have similarity 100 and identical content; similar ones are
//! at least `MIN_SIMILARITY`; the list is ordered by `from`; only regular files take part.

use ag_merge::{MIN_SIMILARITY, detect_renames};
use ag_types::FileType;
use agentgit_fuzz::{MemIo, arb_edit, arb_tree};
use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use std::collections::HashSet;

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let io = MemIo::default();
    let Ok(old) = arb_tree(&mut u, &io, 12) else { return };
    let Ok(new) = arb_edit(&mut u, &io, &old) else { return };

    let renames = detect_renames(&old, &new, &io).expect("all blobs are present");
    assert_eq!(renames, detect_renames(&old, &new, &io).unwrap(), "rename detection must be deterministic");

    let mut froms = HashSet::new();
    let mut tos = HashSet::new();
    for r in &renames {
        assert!(froms.insert(r.from.clone()) && tos.insert(r.to.clone()), "a path is used at most once");
        let (o, n) = (old.get(&r.from).expect("from exists in old"), new.get(&r.to).expect("to exists in new"));
        assert!(new.get(&r.from).is_none(), "from must have disappeared");
        assert!(old.get(&r.to).is_none(), "to must be new");
        assert!(o.file_type == FileType::Regular && n.file_type == FileType::Regular);
        if r.similarity == 100 {
            // Identical bytes, or (a line-by-line match) the same lines in another order.
            let sorted = |id| {
                let data = io.get(id).unwrap();
                let mut lines: Vec<Vec<u8>> = ag_diff::split_lines(&data).into_iter().map(<[u8]>::to_vec).collect();
                lines.sort();
                lines
            };
            assert_eq!(sorted(&o.content), sorted(&n.content), "similarity 100 means the same lines");
        } else {
            assert!((MIN_SIMILARITY..100).contains(&r.similarity), "similarity {} out of range", r.similarity);
        }
    }
    assert!(renames.windows(2).all(|w| w[0].from < w[1].from), "ordered by from");
});
