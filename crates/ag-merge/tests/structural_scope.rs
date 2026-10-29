//! Structural preview scopes enumerate the candidate, including relocated files.
use ag_merge::{BlobIo, IoError, merge_trees};
use ag_object::hash_blob;
use ag_tree::{Entry, Tree};
use ag_types::{BlobId, FileType, Limits, RepoPath};
use std::cell::RefCell;
use std::collections::BTreeMap;

#[derive(Default)]
struct Blobs(RefCell<BTreeMap<BlobId, Vec<u8>>>);
impl BlobIo for Blobs {
    fn read_blob(&self, id: &BlobId) -> Result<Vec<u8>, IoError> {
        Ok(self.0.borrow().get(id).unwrap().clone())
    }
    fn write_blob(&self, data: Vec<u8>) -> Result<BlobId, IoError> {
        let id = hash_blob(&data);
        self.0.borrow_mut().insert(id, data);
        Ok(id)
    }
}
fn tree(blobs: &Blobs, files: &[(String, String)]) -> Tree {
    Tree::from_entries(
        files.iter().map(|(path, content)| {
            (
                RepoPath::new(path).unwrap(),
                Entry {
                    content: blobs.write_blob(content.as_bytes().to_vec()).unwrap(),
                    file_type: FileType::Regular,
                    executable: false,
                },
            )
        }),
        &Limits::default(),
    )
    .unwrap()
}
fn files(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries.iter().map(|(path, content)| (path.to_string(), content.to_string())).collect()
}

#[test]
fn scope_includes_relocated_candidate_paths_and_excludes_similar_siblings() {
    let blobs = Blobs::default();
    let base = tree(&blobs, &files(&[("lib/a", "aaa"), ("lib/b", "bbb"), ("lib/c", "ccc")]));
    let ours = tree(&blobs, &files(&[("src/a", "aaa"), ("src/b", "bbb"), ("src/c", "ccc")]));
    let theirs = tree(
        &blobs,
        &files(&[
            ("lib/a", "aaa"),
            ("lib/b", "bbb"),
            ("lib/c", "ccc"),
            ("lib/new", "new"),
            ("src", "file"),
            ("src-other", "sibling"),
        ]),
    );
    assert!(ours.get(&RepoPath::new("src/new").unwrap()).is_none());
    assert!(theirs.get(&RepoPath::new("src/new").unwrap()).is_none());
    let merged = merge_trees(&base, &ours, &theirs, &blobs, &Limits::default()).unwrap();
    let scopes = merged.structural_resolution_scopes();
    assert_eq!(scopes.len(), 1);
    let scope = &scopes["src"];
    assert!(scope.complete);
    assert_eq!(
        scope.paths.iter().map(RepoPath::as_str).collect::<Vec<_>>(),
        ["src", "src/a", "src/b", "src/c", "src/new"]
    );
}

#[test]
fn scopes_are_bounded_per_root_and_globally_and_mark_omissions() {
    let blobs = Blobs::default();
    let base = Tree::empty();
    let ours = tree(&blobs, &files(&[("big", "file")]));
    let children = (0..100).map(|i| (format!("big/{i:03}"), format!("child{i}"))).collect::<Vec<_>>();
    let exact = tree(&blobs, &children[..99]);
    let exact_merge = merge_trees(&base, &ours, &exact, &blobs, &Limits::default()).unwrap();
    let exact_scope = exact_merge.structural_resolution_scopes().remove("big").unwrap();
    assert_eq!(exact_scope.paths.len(), 100);
    assert!(exact_scope.complete, "exact limit is still a complete preview");
    let theirs = tree(&blobs, &children);
    let merged = merge_trees(&base, &ours, &theirs, &blobs, &Limits::default()).unwrap();
    let scope = merged.structural_resolution_scopes().remove("big").unwrap();
    assert_eq!(scope.paths.len(), 100);
    assert_eq!(scope.paths[0].as_str(), "big");
    assert!(!scope.complete);

    let root_files = (0..501).map(|i| (format!("root{i:04}"), format!("file{i}"))).collect::<Vec<_>>();
    let child_files = (0..501).map(|i| (format!("root{i:04}/child"), format!("child{i}"))).collect::<Vec<_>>();
    let merged =
        merge_trees(&base, &tree(&blobs, &root_files), &tree(&blobs, &child_files), &blobs, &Limits::default())
            .unwrap();
    let scopes = merged.structural_resolution_scopes();
    assert_eq!(scopes.len(), 501);
    assert_eq!(scopes.values().map(|scope| scope.paths.len()).sum::<usize>(), 1000);
    assert!(scopes["root0499"].complete);
    assert!(!scopes["root0500"].complete);
    assert!(scopes["root0500"].paths.is_empty());
}

#[test]
fn ordinary_conflicts_do_not_have_structural_scopes() {
    let blobs = Blobs::default();
    let a = tree(&blobs, &files(&[("file", "a")]));
    let b = tree(&blobs, &files(&[("file", "b")]));
    let merged = merge_trees(&Tree::empty(), &a, &b, &blobs, &Limits::default()).unwrap();
    assert!(!merged.is_clean());
    assert!(merged.structural_resolution_scopes().is_empty());
}
