#![no_main]
//! Tree algebra on generated trees: `a.apply(a.diff(b)) == b`, the changes list agrees with the diff, and
//! a manifest round-trips. Also feeds *arbitrary* operation lists to `Tree::apply`, which must reject bad
//! input (missing delete, no-op put, file under a file) and never panic.

use ag_changeset::Op;
use ag_tree::Tree;
use ag_types::{FileType, Limits};
use agentgit_fuzz::{MemIo, arb_edit, arb_path, arb_tree};
use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let io = MemIo::default();
    let limits = Limits::default();
    let Ok(a) = arb_tree(&mut u, &io, 10) else { return };
    let Ok(b) = arb_edit(&mut u, &io, &a) else { return };

    let ops = a.diff(&b);
    assert_eq!(a.apply(&ops, &limits).expect("a diff applies to its source"), b);
    assert_eq!(ops.len(), a.changes(&b).len());
    assert!(ops.windows(2).all(|w| w[0].path() < w[1].path()), "diff must be in canonical order");
    assert_eq!(Tree::decode(&b.encode(), &limits).unwrap().id(), b.id());

    // Arbitrary (possibly nonsensical) operations against `a`.
    let mut junk: Vec<Op> = Vec::new();
    for _ in 0..u.int_in_range(0..=6).unwrap_or(0) {
        let Ok(path) = arb_path(&mut u) else { break };
        let op = if u.arbitrary::<bool>().unwrap_or(false) {
            Op::Delete { path }
        } else {
            let content = ag_object::hash_blob(&[u.arbitrary::<u8>().unwrap_or(0)]);
            Op::Put { path, content, file_type: FileType::Regular, executable: u.arbitrary().unwrap_or(false) }
        };
        junk.push(op);
    }
    if let Ok(t) = a.apply(&junk, &limits) {
        // Whatever applied successfully is a valid tree.
        assert_eq!(Tree::decode(&t.encode(), &limits).unwrap(), t);
    }
});
