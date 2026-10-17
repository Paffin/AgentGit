#![no_main]
//! Tree manifests. A decoded tree re-encodes to exactly the input, obeys the file/directory rule, and every
//! derived view (listing, diff against the empty tree) is consistent with its entries.

use ag_tree::Tree;
use ag_types::Limits;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    let Ok(tree) = Tree::decode(data, &limits) else { return };
    assert_eq!(tree.encode(), data, "non-canonical manifest was accepted");
    assert_eq!(tree.id(), tree.id());

    // No path lies beneath another entry.
    for (path, _) in tree.iter() {
        for anc in path.ancestors() {
            assert!(tree.iter().all(|(p, _)| p.as_str() != anc), "{anc} is both a file and a directory");
        }
    }

    // diff against the empty tree reproduces the tree when applied.
    let empty = Tree::empty();
    let ops = empty.diff(&tree);
    assert_eq!(ops.len(), tree.len());
    let rebuilt = empty.apply(&ops, &limits).expect("a diff applies to its source");
    assert_eq!(rebuilt, tree);
    assert_eq!(tree.diff(&tree), vec![]);
    assert!(tree.changes(&tree).is_empty());

    // Listings are total and agree with the entry count at the root.
    let root = tree.list_dir("");
    assert!(root.len() <= tree.len());
    for entry in &root {
        if let ag_tree::DirEntry::Dir { name } = entry {
            let _ = tree.list_dir(name);
        }
    }
});
