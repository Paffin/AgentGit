use std::cell::RefCell;
use std::collections::HashMap;

use ag_object::hash_blob;
use ag_tree::{Entry, Tree};
use ag_types::{BlobId, FileType, Limits, RepoPath};
use proptest::prelude::*;

use crate::*;

fn m(base: &str, ours: &str, theirs: &str) -> TextMerge {
    merge_text(base.as_bytes(), ours.as_bytes(), theirs.as_bytes())
}

fn clean(r: TextMerge) -> String {
    match r {
        TextMerge::Clean(b) => String::from_utf8(b).unwrap(),
        other => panic!("expected clean merge, got {other:?}"),
    }
}

#[test]
fn disjoint_edits_merge_cleanly() {
    let base = "a\nb\nc\nd\ne\nf\n";
    let ours = "A\nb\nc\nd\ne\nf\n";
    let theirs = "a\nb\nc\nd\ne\nF\n";
    assert_eq!(clean(m(base, ours, theirs)), "A\nb\nc\nd\ne\nF\n");
}

#[test]
fn same_change_on_both_sides_is_clean() {
    assert_eq!(clean(m("a\nb\n", "a\nX\n", "a\nX\n")), "a\nX\n");
    // identical insertions at the same spot
    assert_eq!(clean(m("a\nb\n", "a\nN\nb\n", "a\nN\nb\n")), "a\nN\nb\n");
}

#[test]
fn overlapping_edits_conflict() {
    match m("a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n") {
        TextMerge::Conflicted { regions, rendered } => {
            assert_eq!(regions.len(), 1);
            assert_eq!(regions[0].base, b"b\n");
            assert_eq!(regions[0].ours, b"X\n");
            assert_eq!(regions[0].theirs, b"Y\n");
            assert_eq!(
                String::from_utf8(rendered).unwrap(),
                "a\n<<<<<<< ours\nX\n=======\nY\n>>>>>>> theirs\nc\n"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn adjacent_edits_conflict_by_design() {
    // ours edits line 2, theirs edits line 3: touching hunks.
    assert!(matches!(
        m("a\nb\nc\nd\n", "a\nB\nc\nd\n", "a\nb\nC\nd\n"),
        TextMerge::Conflicted { .. }
    ));
    // one unchanged line between them is enough to merge
    assert_eq!(clean(m("a\nb\nc\nd\n", "a\nB\nc\nd\n", "a\nb\nc\nD\n")), "a\nB\nc\nD\n");
}

#[test]
fn different_inserts_at_same_position_conflict() {
    assert!(matches!(m("a\nb\n", "a\nX\nb\n", "a\nY\nb\n"), TextMerge::Conflicted { .. }));
}

#[test]
fn delete_vs_edit_in_text_conflicts_and_delete_vs_far_edit_merges() {
    assert!(matches!(m("a\nb\nc\n", "a\nc\n", "a\nB\nc\n"), TextMerge::Conflicted { .. }));
    assert_eq!(clean(m("a\nb\nc\nd\ne\n", "b\nc\nd\ne\n", "a\nb\nc\nd\nE\n")), "b\nc\nd\nE\n");
}

#[test]
fn trailing_newline_changes_are_edits() {
    assert_eq!(clean(m("a\nb", "a\nb\n", "a\nb")), "a\nb\n");
}

#[test]
fn clean_when_one_side_unchanged() {
    assert_eq!(clean(m("x\n", "x\n", "y\n")), "y\n");
    assert_eq!(clean(m("x\n", "y\n", "x\n")), "y\n");
}

proptest! {
    /// If only one side changes, the merge equals that side.
    #[test]
    fn one_sided_changes_win(base in "[ab\n]{0,20}", other in "[ab\n]{0,20}") {
        prop_assert_eq!(merge_text(base.as_bytes(), other.as_bytes(), base.as_bytes()), TextMerge::Clean(other.clone().into_bytes()));
        prop_assert_eq!(merge_text(base.as_bytes(), base.as_bytes(), other.as_bytes()), TextMerge::Clean(other.into_bytes()));
    }

    /// Merging is symmetric in whether it conflicts.
    #[test]
    fn conflict_status_is_symmetric(base in "[abc\n]{0,16}", o in "[abc\n]{0,16}", t in "[abc\n]{0,16}") {
        let a = merge_text(base.as_bytes(), o.as_bytes(), t.as_bytes());
        let b = merge_text(base.as_bytes(), t.as_bytes(), o.as_bytes());
        match (a, b) {
            (TextMerge::Clean(x), TextMerge::Clean(y)) => prop_assert_eq!(x, y),
            (TextMerge::Conflicted { .. }, TextMerge::Conflicted { .. }) => {}
            (l, r) => prop_assert!(false, "asymmetric: {l:?} vs {r:?}"),
        }
    }

    /// Merge is deterministic.
    #[test]
    fn deterministic(base in "[abc\n]{0,16}", o in "[abc\n]{0,16}", t in "[abc\n]{0,16}") {
        prop_assert_eq!(
            merge_text(base.as_bytes(), o.as_bytes(), t.as_bytes()),
            merge_text(base.as_bytes(), o.as_bytes(), t.as_bytes())
        );
    }
}

// ---- tree merge ----

#[derive(Default)]
struct Mem(RefCell<HashMap<BlobId, Vec<u8>>>);

impl BlobIo for Mem {
    fn read_blob(&self, id: &BlobId) -> Result<Vec<u8>, IoError> {
        self.0.borrow().get(id).cloned().ok_or_else(|| "missing blob".into())
    }
    fn write_blob(&self, data: Vec<u8>) -> Result<BlobId, IoError> {
        let id = hash_blob(&data);
        self.0.borrow_mut().insert(id, data);
        Ok(id)
    }
}

fn entry(io: &Mem, content: &str) -> Entry {
    Entry { content: io.write_blob(content.as_bytes().to_vec()).unwrap(), file_type: FileType::Regular, executable: false }
}

fn tree(io: &Mem, files: &[(&str, &str)]) -> Tree {
    Tree::from_entries(
        files.iter().map(|(p, c)| (RepoPath::new(*p).unwrap(), entry(io, c))),
        &Limits::default(),
    )
    .unwrap()
}

fn lim() -> Limits {
    Limits::default()
}

fn p(s: &str) -> RepoPath {
    RepoPath::new(s).unwrap()
}

#[test]
fn different_files_merge() {
    let io = Mem::default();
    let base = tree(&io, &[("a", "1"), ("b", "1")]);
    let ours = tree(&io, &[("a", "2"), ("b", "1")]);
    let theirs = tree(&io, &[("a", "1"), ("b", "2"), ("c", "new")]);
    let r = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    assert!(r.is_clean(), "{:?}", r.conflicts);
    assert_eq!(io.read_blob(&r.tree.get(&p("a")).unwrap().content).unwrap(), b"2");
    assert_eq!(io.read_blob(&r.tree.get(&p("b")).unwrap().content).unwrap(), b"2");
    assert!(r.tree.get(&p("c")).is_some());
}

#[test]
fn same_file_different_regions_merge_and_store_new_blob() {
    let io = Mem::default();
    let base = tree(&io, &[("f", "1\n2\n3\n4\n5\n")]);
    let ours = tree(&io, &[("f", "ONE\n2\n3\n4\n5\n")]);
    let theirs = tree(&io, &[("f", "1\n2\n3\n4\nFIVE\n")]);
    let r = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    assert!(r.is_clean());
    let merged = io.read_blob(&r.tree.get(&p("f")).unwrap().content).unwrap();
    assert_eq!(merged, b"ONE\n2\n3\n4\nFIVE\n");
}

#[test]
fn content_conflict_is_reported() {
    let io = Mem::default();
    let base = tree(&io, &[("f", "x\n")]);
    let ours = tree(&io, &[("f", "o\n")]);
    let theirs = tree(&io, &[("f", "t\n")]);
    let r = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    assert_eq!(r.conflicts, vec![Conflict { path: p("f"), kind: ConflictKind::Content { regions: 1 } }]);
    // best-effort tree keeps our version
    assert_eq!(r.tree.get(&p("f")), ours.get(&p("f")));
}

#[test]
fn delete_modify_both_directions() {
    let io = Mem::default();
    let base = tree(&io, &[("f", "x\n")]);
    let modified = tree(&io, &[("f", "y\n")]);
    let deleted = tree(&io, &[]);
    let r = merge_trees(&base, &deleted, &modified, &io, &lim()).unwrap();
    assert_eq!(r.conflicts[0].kind, ConflictKind::DeleteModify { ours_deleted: true });
    let r = merge_trees(&base, &modified, &deleted, &io, &lim()).unwrap();
    assert_eq!(r.conflicts[0].kind, ConflictKind::DeleteModify { ours_deleted: false });
    // both deleting is fine
    let r = merge_trees(&base, &deleted, &deleted, &io, &lim()).unwrap();
    assert!(r.is_clean() && r.tree.is_empty());
}

#[test]
fn both_added_different_conflicts_same_is_clean() {
    let io = Mem::default();
    let base = tree(&io, &[]);
    let r = merge_trees(&base, &tree(&io, &[("n", "1")]), &tree(&io, &[("n", "2")]), &io, &lim()).unwrap();
    assert_eq!(r.conflicts[0].kind, ConflictKind::BothAdded);
    let r = merge_trees(&base, &tree(&io, &[("n", "1")]), &tree(&io, &[("n", "1")]), &io, &lim()).unwrap();
    assert!(r.is_clean());
}

#[test]
fn binary_conflict() {
    let io = Mem::default();
    let mk = |bytes: &[u8]| {
        Tree::from_entries(
            [(p("img"), Entry { content: io.write_blob(bytes.to_vec()).unwrap(), file_type: FileType::Regular, executable: false })],
            &lim(),
        )
        .unwrap()
    };
    let r = merge_trees(&mk(b"\0a"), &mk(b"\0b"), &mk(b"\0c"), &io, &lim()).unwrap();
    assert_eq!(r.conflicts[0].kind, ConflictKind::Binary);
}

#[test]
fn executable_bit_merges_independently_of_content() {
    let io = Mem::default();
    let base = tree(&io, &[("run", "1\n")]);
    let mut ours = tree(&io, &[("run", "1\n")]);
    let mut e = *ours.get(&p("run")).unwrap();
    e.executable = true;
    ours = Tree::from_entries([(p("run"), e)], &lim()).unwrap();
    let theirs = tree(&io, &[("run", "2\n")]);
    let r = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    assert!(r.is_clean());
    let got = r.tree.get(&p("run")).unwrap();
    assert!(got.executable);
    assert_eq!(io.read_blob(&got.content).unwrap(), b"2\n");
}

#[test]
fn symlink_changes_conflict() {
    let io = Mem::default();
    let link = |target: &str| {
        Tree::from_entries(
            [(p("l"), Entry { content: io.write_blob(target.as_bytes().to_vec()).unwrap(), file_type: FileType::Symlink, executable: false })],
            &lim(),
        )
        .unwrap()
    };
    let r = merge_trees(&link("a"), &link("b"), &link("c"), &io, &lim()).unwrap();
    assert_eq!(r.conflicts[0].kind, ConflictKind::TypeChange);
}

#[test]
fn file_vs_directory_conflict() {
    let io = Mem::default();
    let base = tree(&io, &[]);
    let ours = tree(&io, &[("x", "file")]);
    let theirs = tree(&io, &[("x/y", "inside")]);
    let r = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    assert!(r.conflicts.iter().any(|c| c.path == p("x")
        && matches!(&c.kind, ConflictKind::FileDirectory { inside } if inside == &p("x/y"))));
}

#[test]
fn tree_merge_is_deterministic() {
    let io = Mem::default();
    let base = tree(&io, &[("a", "1\n2\n3\n"), ("b", "k")]);
    let ours = tree(&io, &[("a", "X\n2\n3\n"), ("b", "k"), ("c", "c")]);
    let theirs = tree(&io, &[("a", "1\n2\nZ\n"), ("d", "d")]);
    let r1 = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    let r2 = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    assert_eq!(r1.tree.id(), r2.tree.id());
    assert!(r1.is_clean());
}
