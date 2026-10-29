//! A structural resolution starts from every merged entry, not the fallback tree.
use ag_merge::{BlobIo, IoError, merge_trees};
use ag_object::hash_blob;
use ag_tree::{Entry, Tree, TreeError};
use ag_types::{BlobId, FileType, Limits, RepoPath};
use std::collections::BTreeMap;

struct NoBlobs;
impl BlobIo for NoBlobs {
    fn read_blob(&self, _: &BlobId) -> Result<Vec<u8>, IoError> {
        panic!("disjoint paths need no blob reads")
    }
    fn write_blob(&self, _: Vec<u8>) -> Result<BlobId, IoError> {
        panic!("resolving paths preserves existing blobs")
    }
}
fn path(value: &str) -> RepoPath {
    RepoPath::new(value).unwrap()
}
fn entry(value: &str, file_type: FileType, executable: bool) -> Entry {
    Entry { content: hash_blob(value.as_bytes()), file_type, executable }
}
fn tree(files: &[(&str, Entry)]) -> Tree {
    Tree::from_entries(files.iter().map(|(name, entry)| (path(name), *entry)), &Limits::default()).unwrap()
}

#[test]
fn multiple_structural_choices_preserve_all_exact_entries_and_the_original_fallback() {
    let regular = entry("file", FileType::Regular, true);
    let link = entry("../target", FileType::Symlink, false);
    let clean = entry("\0binary", FileType::Regular, false);
    let base = Tree::empty();
    let ours = tree(&[("first", regular), ("second/leaf", link), ("ours", regular)]);
    let theirs = tree(&[("first/link", link), ("first/executable", regular), ("second", regular), ("clean", clean)]);
    let merged = merge_trees(&base, &ours, &theirs, &NoBlobs, &Limits::default()).unwrap();
    assert_eq!(merged.conflicts.len(), 2);
    assert_eq!(merged.tree, ours, "the legacy best-effort tree does not change");
    let unresolved = merged.resolve_paths(&BTreeMap::new(), &Limits::default());
    assert!(matches!(unresolved, Err(TreeError::FileDirectoryConflict { .. })));
    let choices = BTreeMap::from([(path("first"), None), (path("second"), None)]);
    let resolved = merged.resolve_paths(&choices, &Limits::default()).unwrap();
    assert_eq!(resolved.get(&path("first/link")), Some(&link));
    assert_eq!(resolved.get(&path("first/executable")), Some(&regular));
    assert_eq!(resolved.get(&path("second/leaf")), Some(&link));
    assert_eq!(resolved.get(&path("ours")), Some(&regular));
    assert_eq!(resolved.get(&path("clean")), Some(&clean));
    assert_eq!(merged.tree, ours, "resolving never mutates the preview");
    let small = Limits { max_tree_entries: 1, ..Limits::default() };
    assert!(matches!(merged.resolve_paths(&choices, &small), Err(TreeError::TooManyEntries(5))));
}
