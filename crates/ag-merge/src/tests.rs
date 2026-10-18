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
            assert_eq!(String::from_utf8(rendered).unwrap(), "a\n<<<<<<< ours\nX\n=======\nY\n>>>>>>> theirs\nc\n");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn every_marker_of_a_conflict_starts_on_its_own_line() {
    // a side without a final newline gets one before the next marker ...
    assert_eq!(render_conflict_markers(b"x", b"y"), b"<<<<<<< ours\nx\n=======\ny\n>>>>>>> theirs\n".to_vec());
    // ... one that has it does not get a second ...
    assert_eq!(render_conflict_markers(b"x\n", b"y\n"), b"<<<<<<< ours\nx\n=======\ny\n>>>>>>> theirs\n".to_vec());
    // ... and an empty side stays empty: no blank line between the markers
    assert_eq!(render_conflict_markers(b"", b"y\n"), b"<<<<<<< ours\n=======\ny\n>>>>>>> theirs\n".to_vec());
    assert_eq!(render_conflict_markers(b"x\n", b""), b"<<<<<<< ours\nx\n=======\n>>>>>>> theirs\n".to_vec());
    assert_eq!(render_conflict_markers(b"", b""), b"<<<<<<< ours\n=======\n>>>>>>> theirs\n".to_vec());
}

#[test]
fn a_conflict_at_the_end_of_a_file_without_a_final_newline_renders_cleanly() {
    match m("a\nb", "a\nX", "a\nY") {
        TextMerge::Conflicted { regions, rendered } => {
            assert_eq!(regions.len(), 1);
            assert_eq!(String::from_utf8(rendered).unwrap(), "a\n<<<<<<< ours\nX\n=======\nY\n>>>>>>> theirs\n");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn hunks_of_both_sides_at_the_same_place_do_not_depend_on_which_side_is_which() {
    // both sides insert at the same position, and both replace the same line: one cluster either way
    for (base, ours, theirs) in [("a\nb\n", "a\nX\nb\n", "a\nY\nb\n"), ("a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n")] {
        let ab = m(base, ours, theirs);
        let ba = m(base, theirs, ours);
        match (ab, ba) {
            (TextMerge::Conflicted { regions: r1, .. }, TextMerge::Conflicted { regions: r2, .. }) => {
                assert_eq!((r1.len(), r2.len()), (1, 1));
                assert_eq!((&r1[0].ours, &r1[0].theirs), (&r2[0].theirs, &r2[0].ours));
                assert_eq!(r1[0].base, r2[0].base);
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn adjacent_edits_conflict_by_design() {
    // ours edits line 2, theirs edits line 3: touching hunks.
    assert!(matches!(m("a\nb\nc\nd\n", "a\nB\nc\nd\n", "a\nb\nC\nd\n"), TextMerge::Conflicted { .. }));
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
    Entry {
        content: io.write_blob(content.as_bytes().to_vec()).unwrap(),
        file_type: FileType::Regular,
        executable: false,
    }
}

fn tree(io: &Mem, files: &[(&str, &str)]) -> Tree {
    Tree::from_entries(files.iter().map(|(p, c)| (RepoPath::new(*p).unwrap(), entry(io, c))), &Limits::default())
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
    assert_eq!(r.conflicts, vec![Conflict { path: p("f"), kind: ConflictKind::Content { regions: 1 }, origin: None }]);
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
            [(
                p("img"),
                Entry {
                    content: io.write_blob(bytes.to_vec()).unwrap(),
                    file_type: FileType::Regular,
                    executable: false,
                },
            )],
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
            [(
                p("l"),
                Entry {
                    content: io.write_blob(target.as_bytes().to_vec()).unwrap(),
                    file_type: FileType::Symlink,
                    executable: false,
                },
            )],
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
    assert!(
        r.conflicts
            .iter()
            .any(|c| c.path == p("x")
                && matches!(&c.kind, ConflictKind::FileDirectory { inside } if inside == &p("x/y")))
    );
}

#[test]
fn a_file_is_text_merged_up_to_16_mib_and_conflicts_beyond() {
    let io = Mem::default();
    // 16384 lines of 1 KiB are exactly 16 MiB; ours edits the first line, theirs the last
    let line = |i: usize| format!("{i:01023}\n");
    let base: String = (0..16_384).map(line).collect();
    assert_eq!(base.len(), 16 * 1024 * 1024);
    let ours = format!("{}{}", "o".repeat(1023) + "\n", &base[1024..]);
    let theirs = format!("{}{}", &base[..base.len() - 1024], "t".repeat(1023) + "\n");
    let merge = |theirs: &str| {
        let (b, o, t) = (tree(&io, &[("f", &base)]), tree(&io, &[("f", &ours)]), tree(&io, &[("f", theirs)]));
        merge_trees(&b, &o, &t, &io, &lim()).unwrap()
    };
    let r = merge(&theirs);
    assert!(r.is_clean(), "{:?}", r.conflicts);
    let merged = content(&io, &r.tree, "f");
    assert!(merged.starts_with("ooo") && merged.ends_with("ttt\n") && merged.len() == base.len());
    // one byte more on one side
    let r = merge(&format!("{theirs}z"));
    assert_eq!(r.conflicts.len(), 1, "{:?}", r.conflicts);
    assert_eq!(r.conflicts[0].kind, ConflictKind::TooLarge);
    assert_eq!(MAX_TEXT_MERGE_SIZE, 16 * 1024 * 1024);
}

#[test]
fn content_is_only_merged_when_all_three_sides_are_regular_files() {
    use FileType::{Regular as R, Symlink as S};
    let io = Mem::default();
    let mk = |content: &str, file_type: FileType| {
        let entry =
            Entry { content: io.write_blob(content.as_bytes().to_vec()).unwrap(), file_type, executable: false };
        Tree::from_entries([(p("f"), entry)], &lim()).unwrap()
    };
    // every combination with a link in it, whichever side it is on
    for (base, ours, theirs) in [(S, R, R), (R, S, R), (R, R, S), (S, S, R), (S, R, S), (R, S, S), (S, S, S)] {
        let r = merge_trees(&mk("b\n", base), &mk("o\n", ours), &mk("t\n", theirs), &io, &lim()).unwrap();
        assert_eq!(r.conflicts.len(), 1, "{base:?} {ours:?} {theirs:?}");
        assert_eq!(r.conflicts[0].kind, ConflictKind::TypeChange, "{base:?} {ours:?} {theirs:?}");
    }
    // three regular files that differ are an ordinary content conflict
    let r = merge_trees(&mk("b\n", R), &mk("o\n", R), &mk("t\n", R), &io, &lim()).unwrap();
    assert_eq!(r.conflicts[0].kind, ConflictKind::Content { regions: 1 });
}

#[test]
fn the_executable_bit_goes_to_the_side_that_changed_it() {
    let io = Mem::default();
    let body = "1\n2\n3\n4\n5\n6\n7\n8\n";
    let (edit_first, edit_last) = (body.replacen("1\n", "ONE\n", 1), body.replacen("8\n", "EIGHT\n", 1));
    let mk = |content: &str, executable: bool| {
        let entry = Entry {
            content: io.write_blob(content.as_bytes().to_vec()).unwrap(),
            file_type: FileType::Regular,
            executable,
        };
        Tree::from_entries([(p("run"), entry)], &lim()).unwrap()
    };
    // (executable in base, in ours, in theirs) -> in the result; both sides edit the file, so it is a content merge
    let table = [
        (false, false, false, false),
        (false, true, true, true),
        (false, true, false, true), // ours made it executable
        (false, false, true, true), // theirs made it executable
        (true, true, true, true),
        (true, false, false, false),
        (true, false, true, false), // ours cleared it
        (true, true, false, false), // theirs cleared it
    ];
    for (base, ours, theirs, expected) in table {
        let r = merge_trees(&mk(body, base), &mk(&edit_first, ours), &mk(&edit_last, theirs), &io, &lim()).unwrap();
        assert!(r.is_clean(), "{:?}", r.conflicts);
        assert_eq!(r.tree.get(&p("run")).unwrap().executable, expected, "base {base} ours {ours} theirs {theirs}");
        assert_eq!(content(&io, &r.tree, "run"), "ONE\n2\n3\n4\n5\n6\n7\nEIGHT\n");
    }
}

#[test]
fn every_kind_of_conflict_has_a_sentence() {
    let cases = [
        (ConflictKind::Content { regions: 2 }, "content conflict (2 region(s))"),
        (ConflictKind::BothAdded, "both sides added different content"),
        (ConflictKind::DeleteModify { ours_deleted: true }, "deleted on our side, modified on theirs"),
        (ConflictKind::DeleteModify { ours_deleted: false }, "modified on our side, deleted on theirs"),
        (ConflictKind::Binary, "binary file changed on both sides"),
        (ConflictKind::TypeChange, "file type or symlink target changed on both sides"),
        (ConflictKind::TooLarge, "file too large to merge"),
        (ConflictKind::FileDirectory { inside: p("x/y") }, "file/directory conflict with 'x/y'"),
        (
            ConflictKind::RenameRename { ours: p("a"), theirs: p("b") },
            "renamed to 'a' on our side and to 'b' on theirs",
        ),
        (ConflictKind::RenameDelete { ours_renamed: true }, "renamed on our side, deleted on theirs"),
        (ConflictKind::RenameDelete { ours_renamed: false }, "deleted on our side, renamed on theirs"),
    ];
    for (kind, text) in cases {
        assert_eq!(kind.to_string(), text);
    }
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

// ---- renames ------------------------------------------------------------------------------

fn content(io: &Mem, t: &Tree, path: &str) -> String {
    String::from_utf8(io.read_blob(&t.get(&p(path)).unwrap_or_else(|| panic!("{path} missing")).content).unwrap())
        .unwrap()
}

fn lines(prefix: &str, n: usize) -> String {
    (0..n).map(|i| format!("{prefix} line {i}\n")).collect()
}

fn pairs(r: &[Rename]) -> Vec<(String, String, u32)> {
    r.iter().map(|r| (r.from.to_string(), r.to.to_string(), r.similarity)).collect()
}

#[test]
fn exact_renames_are_found_and_everything_else_is_not() {
    let io = Mem::default();
    let old = tree(&io, &[("a.txt", "alpha\n"), ("keep", "k\n"), ("gone", "bye\n")]);
    let new = tree(&io, &[("sub/a.txt", "alpha\n"), ("keep", "k\n"), ("fresh", "new\n")]);
    assert_eq!(pairs(&detect_renames(&old, &new, &io).unwrap()), vec![("a.txt".into(), "sub/a.txt".into(), 100)]);
    // Nothing deleted, or nothing added: nothing to pair.
    assert!(detect_renames(&old, &old, &io).unwrap().is_empty());
    let grown = tree(&io, &[("a.txt", "alpha\n"), ("keep", "k\n"), ("gone", "bye\n"), ("x", "x")]);
    assert!(detect_renames(&old, &grown, &io).unwrap().is_empty());
}

#[test]
fn similar_renames_need_enough_common_lines() {
    let io = Mem::default();
    let original = lines("orig", 20);
    // 19 of 20 lines survive, plus one new line: ~95% similar.
    let edited = format!("{}changed\n", lines("orig", 19));
    let old = tree(&io, &[("old.rs", &original)]);
    let new = tree(&io, &[("new.rs", &edited)]);
    let r = detect_renames(&old, &new, &io).unwrap();
    assert_eq!(r.len(), 1);
    assert!(r[0].similarity >= 90 && r[0].similarity < 100, "{}", r[0].similarity);
    // Rewritten beyond recognition: not a rename.
    let rewritten = tree(&io, &[("new.rs", &format!("{}{}", lines("orig", 5), lines("other", 15)))]);
    assert!(detect_renames(&old, &rewritten, &io).unwrap().is_empty());
}

#[test]
fn binary_and_huge_files_only_match_exactly() {
    let io = Mem::default();
    let mut bin = vec![0u8; 100];
    bin.extend(b"payload");
    let mut bin2 = bin.clone();
    bin2.push(b'!');
    let mk = |data: &[u8]| Entry {
        content: io.write_blob(data.to_vec()).unwrap(),
        file_type: FileType::Regular,
        executable: false,
    };
    let t = |name: &str, e: Entry| Tree::from_entries([(p(name), e)], &lim()).unwrap();
    assert_eq!(detect_renames(&t("a.bin", mk(&bin)), &t("b.bin", mk(&bin)), &io).unwrap().len(), 1);
    assert!(detect_renames(&t("a.bin", mk(&bin)), &t("b.bin", mk(&bin2)), &io).unwrap().is_empty());
}

#[test]
fn copies_pair_by_file_name_then_path_and_never_twice() {
    let io = Mem::default();
    // Two deleted files and two added files with identical content.
    let old = tree(&io, &[("x/util.rs", "same\n"), ("y/main.rs", "same\n")]);
    let new = tree(&io, &[("p/main.rs", "same\n"), ("q/util.rs", "same\n")]);
    let r = pairs(&detect_renames(&old, &new, &io).unwrap());
    assert_eq!(r, vec![("x/util.rs".into(), "q/util.rs".into(), 100), ("y/main.rs".into(), "p/main.rs".into(), 100)]);
    // The same answer however the inputs were built.
    let r2 = pairs(&detect_renames(&old, &new, &io).unwrap());
    assert_eq!(r, r2);
}

fn symlink_tree(io: &Mem, path: &str, target: &str) -> Tree {
    let link = Entry {
        content: io.write_blob(target.as_bytes().to_vec()).unwrap(),
        file_type: FileType::Symlink,
        executable: false,
    };
    Tree::from_entries([(p(path), link)], &lim()).unwrap()
}

#[test]
fn a_symlink_is_never_renamed_even_to_a_file_with_the_same_bytes() {
    let io = Mem::default();
    let file = tree(&io, &[("f", "target")]);
    // link -> file, file -> link and link -> link, all with the same blob behind them
    assert!(detect_renames(&symlink_tree(&io, "l", "target"), &file, &io).unwrap().is_empty());
    assert!(detect_renames(&file, &symlink_tree(&io, "l", "target"), &io).unwrap().is_empty());
    assert!(
        detect_renames(&symlink_tree(&io, "l1", "target"), &symlink_tree(&io, "l2", "target"), &io).unwrap().is_empty()
    );
}

#[test]
fn a_path_is_paired_once_whether_the_content_is_identical_or_only_similar() {
    let io = Mem::default();
    // identical: one old file, two new ones - and two old files, one new one
    let same = "same\n";
    let one_to_two =
        detect_renames(&tree(&io, &[("a.txt", same)]), &tree(&io, &[("b.txt", same), ("c.txt", same)]), &io);
    assert_eq!(pairs(&one_to_two.unwrap()), vec![("a.txt".into(), "b.txt".into(), 100)]);
    let two_to_one =
        detect_renames(&tree(&io, &[("a.txt", same), ("b.txt", same)]), &tree(&io, &[("c.txt", same)]), &io);
    assert_eq!(pairs(&two_to_one.unwrap()), vec![("a.txt".into(), "c.txt".into(), 100)]);

    // similar: the better match wins and the other file stays unpaired, in both directions
    let original = lines("orig", 20);
    let close = format!("{}changed\n", lines("orig", 19)); // 19 of 20 lines
    let looser = format!("{}{}", lines("orig", 15), lines("other", 5)); // 15 of 20 lines
    let one_to_two =
        detect_renames(&tree(&io, &[("old.rs", &original)]), &tree(&io, &[("x.rs", &looser), ("y.rs", &close)]), &io);
    assert_eq!(pairs(&one_to_two.unwrap()), vec![("old.rs".into(), "y.rs".into(), 95)]);
    let two_to_one =
        detect_renames(&tree(&io, &[("x.rs", &looser), ("y.rs", &original)]), &tree(&io, &[("new.rs", &close)]), &io);
    assert_eq!(pairs(&two_to_one.unwrap()), vec![("y.rs".into(), "new.rs".into(), 95)]);
}

/// `n` files that share no line with each other, in `dir`; `f0.txt` is `lines(&format!("{dir}0"), 8)`.
fn crowd(io: &Mem, dir: &str, n: usize) -> Tree {
    Tree::from_entries(
        (0..n).map(|i| (p(&format!("{dir}/f{i}.txt")), entry(io, &lines(&format!("{dir}{i}"), 8)))),
        &lim(),
    )
    .unwrap()
}

/// What `crowd(.., "old"|"new", ..)`'s `f0.txt` looks like after its last line was edited.
fn edited_f0(dir: &str) -> String {
    format!("{}edited\n", lines(&format!("{dir}0"), 7))
}

#[test]
fn similarity_is_only_tried_for_up_to_two_hundred_files_on_each_side() {
    let io = Mem::default();
    let moved = |old: &Tree, new: &Tree| pairs(&detect_renames(old, new, &io).unwrap());
    for (n, found) in [(200, true), (201, false)] {
        // many deleted files, one added file that is an edited copy of one of them
        let added = tree(&io, &[("g.txt", &edited_f0("old"))]);
        let r = moved(&crowd(&io, "old", n), &added);
        assert_eq!(r.len(), usize::from(found), "{n} deleted: {r:?}");
        // one deleted file, many added files
        let deleted = tree(&io, &[("g.txt", &edited_f0("new"))]);
        let r = moved(&deleted, &crowd(&io, "new", n));
        assert_eq!(r.len(), usize::from(found), "{n} added: {r:?}");
    }
    // ... and a search that is not attempted costs no read: only the file ids are compared
    let spy = Counting::new(&io);
    let added = tree(&io, &[("g.txt", &edited_f0("old"))]);
    assert!(detect_renames(&crowd(&io, "old", 201), &added, &spy).unwrap().is_empty());
    assert!(
        detect_renames(&tree(&io, &[("g.txt", &edited_f0("new"))]), &crowd(&io, "new", 201), &spy).unwrap().is_empty()
    );
    assert_eq!(spy.reads(), 0);
}

/// A [`BlobIo`] that counts the blobs read through it.
struct Counting<'a> {
    inner: &'a Mem,
    reads: std::cell::Cell<usize>,
}

impl<'a> Counting<'a> {
    fn new(inner: &'a Mem) -> Self {
        Self { inner, reads: std::cell::Cell::new(0) }
    }
    fn reads(&self) -> usize {
        self.reads.get()
    }
}

impl BlobIo for Counting<'_> {
    fn read_blob(&self, id: &BlobId) -> Result<Vec<u8>, IoError> {
        self.reads.set(self.reads.get() + 1);
        self.inner.read_blob(id)
    }
    fn write_blob(&self, data: Vec<u8>) -> Result<BlobId, IoError> {
        self.inner.write_blob(data)
    }
}

#[test]
fn files_are_read_only_when_there_is_something_left_to_compare_them_with() {
    let io = Mem::default();
    let spy = Counting::new(&io);
    // an exact rename needs the ids alone
    detect_renames(&tree(&io, &[("a", "same\n")]), &tree(&io, &[("b", "same\n")]), &spy).unwrap();
    assert_eq!(spy.reads(), 0);
    // everything deleted was matched exactly: the new file has nothing to be compared with
    let found = detect_renames(&tree(&io, &[("a", "same\n")]), &tree(&io, &[("b", "same\n"), ("c", "other\n")]), &spy);
    assert_eq!(pairs(&found.unwrap()), vec![("a".into(), "b".into(), 100)]);
    assert_eq!(spy.reads(), 0);
    // and the other way round
    let found = detect_renames(&tree(&io, &[("a", "same\n"), ("c", "other\n")]), &tree(&io, &[("b", "same\n")]), &spy);
    assert_eq!(pairs(&found.unwrap()), vec![("a".into(), "b".into(), 100)]);
    assert_eq!(spy.reads(), 0);
    // two candidates, one on each side: both are read once
    detect_renames(&tree(&io, &[("a", "one\n")]), &tree(&io, &[("b", "two\n")]), &spy).unwrap();
    assert_eq!(spy.reads(), 2);
}

#[test]
fn the_similarity_threshold_is_sixty_percent_of_the_bytes() {
    let io = Mem::default();
    let renamed = |old: &str, new: &str| {
        let found = detect_renames(&tree(&io, &[("old", old)]), &tree(&io, &[("new", new)]), &io).unwrap();
        found.iter().map(|r| r.similarity).collect::<Vec<_>>()
    };
    // 59 bytes of 100 are shared: 2 * 59 / 200 = 59 percent, one short
    let (shared, ours, theirs) = ("p".repeat(58) + "\n", "u".repeat(40) + "\n", "v".repeat(40) + "\n");
    assert_eq!(renamed(&format!("{shared}{ours}"), &format!("{shared}{theirs}")), Vec::<u32>::new());
    // 60 of 100: exactly the threshold
    let (shared, ours, theirs) = ("p".repeat(59) + "\n", "u".repeat(39) + "\n", "v".repeat(39) + "\n");
    assert_eq!(renamed(&format!("{shared}{ours}"), &format!("{shared}{theirs}")), vec![60]);
    // a small file all of whose bytes are in a larger one: 2 * 3 / (3 + 7) = 60 percent, whichever way round
    assert_eq!(renamed("xy\n", "xy\nabc\n"), vec![60]);
    assert_eq!(renamed("xy\nabc\n", "xy\n"), vec![60]);
    // a few bytes more and it is 54
    assert_eq!(renamed("xy\n", "xy\nabcd\n"), Vec::<u32>::new());
}

#[test]
fn similarity_matching_stops_at_exactly_256_kib() {
    let io = Mem::default();
    // 4096 lines of 64 bytes, and the same with the first line replaced by another of the same length
    let big: String = (0..4096).map(|i| format!("{i:063}\n")).collect();
    let edited = format!("{}\n{}", "x".repeat(63), &big[64..]);
    let renamed =
        |old: &str, new: &str| detect_renames(&tree(&io, &[("old", old)]), &tree(&io, &[("new", new)]), &io).unwrap();
    assert_eq!((big.len(), edited.len()), (262_144, 262_144));
    assert_eq!(pairs(&renamed(&big, &edited)), vec![("old".into(), "new".into(), 99)]);
    // one byte more, on both sides: only identical files match
    assert!(renamed(&format!("{big}z"), &format!("{edited}z")).is_empty());
    assert_eq!(pairs(&renamed(&format!("{big}z"), &format!("{big}z"))), vec![("old".into(), "new".into(), 100)]);
}

#[test]
fn a_file_with_a_nul_byte_only_matches_exactly() {
    let io = Mem::default();
    let renamed = |old: String, new: String| {
        detect_renames(&tree(&io, &[("old", &old)]), &tree(&io, &[("new", &new)]), &io).unwrap()
    };
    let (old, new) = (format!("{}tail\n", lines("shared", 20)), format!("{}changed\ntail\n", lines("shared", 19)));
    // the same two files without a NUL byte are a rename ...
    assert_eq!(renamed(old.clone(), new.clone()).len(), 1);
    // ... and with one they are not, although nearly all of their lines are the same
    assert!(renamed(old.replacen("tail", "\0tail", 1), new.replacen("tail", "\0tail", 1)).is_empty());
    // the identical binary file is
    assert_eq!(renamed(old.replacen("tail", "\0tail", 1), old.replacen("tail", "\0tail", 1)).len(), 1);
}

#[test]
fn a_rename_on_one_side_and_an_edit_on_the_other_merge_at_the_new_path() {
    let io = Mem::default();
    let body = lines("fn", 12);
    let base = tree(&io, &[("src/old.rs", &body), ("README", "r\n")]);
    // ours moves the file; theirs edits the last line.
    let ours = tree(&io, &[("src/new.rs", &body), ("README", "r\n")]);
    let edited = body.replace("fn line 11", "fn line 11 EDITED");
    let theirs = tree(&io, &[("src/old.rs", &edited), ("README", "r\n")]);
    let r = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    assert!(r.is_clean(), "{:?}", r.conflicts);
    assert!(r.tree.get(&p("src/old.rs")).is_none());
    assert_eq!(content(&io, &r.tree, "src/new.rs"), edited);
    // And the mirror image: theirs moves, ours edits.
    let r = merge_trees(&base, &theirs, &ours, &io, &lim()).unwrap();
    assert!(r.is_clean(), "{:?}", r.conflicts);
    assert_eq!(content(&io, &r.tree, "src/new.rs"), edited);
    assert!(r.tree.get(&p("src/old.rs")).is_none());
}

#[test]
fn rename_with_edits_on_both_sides_merges_when_the_edits_are_apart() {
    let io = Mem::default();
    let body = lines("l", 20);
    let base = tree(&io, &[("a.txt", &body)]);
    let ours = tree(&io, &[("moved/a.txt", &body.replacen("l line 1\n", "l line 1 ours\n", 1))]);
    let theirs = tree(&io, &[("a.txt", &body.replacen("l line 18\n", "l line 18 theirs\n", 1))]);
    let r = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    assert!(r.is_clean(), "{:?}", r.conflicts);
    let merged = content(&io, &r.tree, "moved/a.txt");
    assert!(merged.contains("l line 1 ours\n") && merged.contains("l line 18 theirs\n"));
    assert!(r.tree.get(&p("a.txt")).is_none());
}

#[test]
fn conflicting_edits_after_a_rename_are_reported_at_the_new_path_with_their_origin() {
    let io = Mem::default();
    let base = tree(&io, &[("a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n")]);
    let ours = tree(&io, &[("b.txt", "1\n2\nOURS\n4\n5\n6\n7\n8\n9\n")]);
    let theirs = tree(&io, &[("a.txt", "1\n2\nTHEIRS\n4\n5\n6\n7\n8\n9\n")]);
    let r = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    assert_eq!(r.conflicts.len(), 1, "{:?}", r.conflicts);
    let c = &r.conflicts[0];
    assert_eq!(c.path, p("b.txt"));
    assert_eq!(c.kind, ConflictKind::Content { regions: 1 });
    assert_eq!(c.origin, Some(ConflictOrigin { base: p("a.txt"), ours: p("b.txt"), theirs: p("a.txt") }));
    // The best-effort tree has our file at the new location and nothing at the old one.
    assert!(r.tree.get(&p("b.txt")).is_some() && r.tree.get(&p("a.txt")).is_none());
}

#[test]
fn both_sides_renaming_to_different_places_conflicts_and_to_the_same_place_does_not() {
    let io = Mem::default();
    let body = lines("x", 10);
    let base = tree(&io, &[("a.txt", &body)]);
    let ours = tree(&io, &[("b.txt", &body)]);
    let theirs = tree(&io, &[("c.txt", &body)]);
    let r = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    assert_eq!(r.conflicts.len(), 1, "{:?}", r.conflicts);
    assert_eq!(r.conflicts[0].path, p("a.txt"));
    assert_eq!(r.conflicts[0].kind, ConflictKind::RenameRename { ours: p("b.txt"), theirs: p("c.txt") });
    assert!(
        r.tree.get(&p("b.txt")).is_some() && r.tree.get(&p("c.txt")).is_some(),
        "both files survive in the best-effort tree"
    );

    // Same destination, with an edit on each side far apart: one merged file.
    let ours = tree(&io, &[("b.txt", &body.replacen("x line 0\n", "x line 0 o\n", 1))]);
    let theirs = tree(&io, &[("b.txt", &body.replacen("x line 9\n", "x line 9 t\n", 1))]);
    let r = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    assert!(r.is_clean(), "{:?}", r.conflicts);
    let merged = content(&io, &r.tree, "b.txt");
    assert!(merged.contains("x line 0 o\n") && merged.contains("x line 9 t\n"));
}

#[test]
fn renaming_a_file_the_other_side_deleted_conflicts() {
    let io = Mem::default();
    let body = lines("d", 8);
    let base = tree(&io, &[("a.txt", &body), ("z", "z\n")]);
    let ours = tree(&io, &[("b.txt", &body), ("z", "z\n")]);
    let theirs = tree(&io, &[("z", "z\n")]);
    let r = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    assert_eq!(r.conflicts.len(), 1, "{:?}", r.conflicts);
    assert_eq!(r.conflicts[0].path, p("b.txt"));
    assert_eq!(r.conflicts[0].kind, ConflictKind::RenameDelete { ours_renamed: true });
    let r = merge_trees(&base, &theirs, &ours, &io, &lim()).unwrap();
    assert_eq!(r.conflicts[0].kind, ConflictKind::RenameDelete { ours_renamed: false });
}

#[test]
fn a_rename_onto_a_path_the_other_side_also_created_is_an_ordinary_conflict() {
    let io = Mem::default();
    let body = lines("q", 8);
    let base = tree(&io, &[("a.txt", &body)]);
    let ours = tree(&io, &[("b.txt", &body)]);
    let theirs = tree(&io, &[("a.txt", &format!("{body}more\n")), ("b.txt", "something else\n")]);
    let r = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    assert!(!r.is_clean());
    assert!(r.conflicts.iter().any(|c| c.path == p("b.txt") && c.kind == ConflictKind::BothAdded), "{:?}", r.conflicts);
}

#[test]
fn renames_do_not_disturb_merges_without_them() {
    // The pre-rename test cases still hold; this one pins the "delete + unrelated add" case:
    // dissimilar content is not a rename, so a delete/modify conflict stays a delete/modify conflict.
    let io = Mem::default();
    let base = tree(&io, &[("a", &lines("one", 6))]);
    let ours = tree(&io, &[("b", &lines("zzz", 6))]);
    let theirs = tree(&io, &[("a", &format!("{}x\n", lines("one", 6)))]);
    let r = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    assert!(
        r.conflicts
            .iter()
            .any(|c| c.path == p("a") && matches!(c.kind, ConflictKind::DeleteModify { ours_deleted: true })),
        "{:?}",
        r.conflicts
    );
}

#[test]
fn rename_aware_merge_is_deterministic() {
    let io = Mem::default();
    let body = lines("det", 15);
    let base = tree(&io, &[("a", &body), ("b", &body.replace("det", "other"))]);
    let ours = tree(&io, &[("c", &body), ("d", &body.replace("det", "other"))]);
    let theirs = tree(&io, &[("a", &format!("{body}t\n")), ("b", &format!("{}t\n", body.replace("det", "other")))]);
    let ids: Vec<_> = (0..5).map(|_| merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap().tree.id()).collect();
    assert!(ids.windows(2).all(|w| w[0] == w[1]));
    let r = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
    assert!(r.is_clean(), "{:?}", r.conflicts);
    assert!(content(&io, &r.tree, "c").ends_with("t\n") && content(&io, &r.tree, "d").ends_with("t\n"));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn detected_renames_are_disjoint_and_come_from_the_difference(
        files in proptest::collection::btree_map("[a-d]{1,2}(/[a-d]{1,2})?", "[a-c]{0,3}(\n[a-c]{0,3}){0,6}", 1..8),
        moved in proptest::collection::vec(any::<bool>(), 8),
    ) {
        let io = Mem::default();
        let entries: Vec<(&str, &str)> = files.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        // Skip inputs where a path is also a directory prefix of another (not a valid tree).
        let Ok(old) = Tree::from_entries(entries.iter().map(|(p_, c)| (p(p_), entry(&io, c))), &lim()) else { return Ok(()) };
        let moved_entries: Vec<(String, &str)> = entries
            .iter()
            .zip(moved.iter().cycle())
            .map(|((k, c), m)| (if *m { format!("moved/{k}") } else { (*k).to_string() }, *c))
            .collect();
        let Ok(new) = Tree::from_entries(moved_entries.iter().map(|(p_, c)| (p(p_), entry(&io, c))), &lim()) else { return Ok(()) };
        let r = detect_renames(&old, &new, &io).unwrap();
        let mut froms = std::collections::BTreeSet::new();
        let mut tos = std::collections::BTreeSet::new();
        for rn in &r {
            prop_assert!(old.get(&rn.from).is_some() && new.get(&rn.from).is_none());
            prop_assert!(new.get(&rn.to).is_some() && old.get(&rn.to).is_none());
            prop_assert!(froms.insert(rn.from.clone()) && tos.insert(rn.to.clone()), "a path was paired twice");
            prop_assert!(rn.similarity >= MIN_SIMILARITY && rn.similarity <= 100);
        }
        // Same inputs, same answer.
        prop_assert_eq!(r, detect_renames(&old, &new, &io).unwrap());
    }

    #[test]
    fn rename_plus_append_always_merges_to_the_renamed_file_with_the_append(n_files in 1usize..6, victim in 0usize..6, size in 6usize..20) {
        let io = Mem::default();
        let victim = victim % n_files;
        // Unique content per file, so only the intended pair can match.
        let base_files: Vec<(String, String)> = (0..n_files).map(|i| (format!("dir/f{i}.txt"), lines(&format!("file{i}"), size))).collect();
        let base = Tree::from_entries(base_files.iter().map(|(k, v)| (p(k), entry(&io, v))), &lim()).unwrap();
        let ours = Tree::from_entries(
            base_files.iter().enumerate().map(|(i, (k, v))| (p(&if i == victim { "renamed/target.txt".to_string() } else { k.clone() }), entry(&io, v))),
            &lim(),
        ).unwrap();
        let theirs = Tree::from_entries(
            base_files.iter().enumerate().map(|(i, (k, v))| (p(k), entry(&io, &if i == victim { format!("{v}appended\n") } else { v.clone() }))),
            &lim(),
        ).unwrap();
        let r = merge_trees(&base, &ours, &theirs, &io, &lim()).unwrap();
        prop_assert!(r.is_clean(), "{:?}", r.conflicts);
        prop_assert_eq!(content(&io, &r.tree, "renamed/target.txt"), format!("{}appended\n", base_files[victim].1));
        prop_assert!(r.tree.get(&p(&base_files[victim].0)).is_none());
        prop_assert_eq!(r.tree.len(), n_files);
    }
}

// ---- the incremental merge agrees with the reference algorithm --------------------------------

mod strategies_agree {
    use super::*;
    use crate::tree::{merge_full, merge_incremental, plan_renames};

    const PATHS: &[&str] = &["a", "b", "a/x", "a/y", "b/x", "c", "d/e", "d/e/f", "d/g", "h/i/j", "h"];
    const BODIES: &[&str] = &[
        "1\n2\n3\n4\n5\n6\n7\n8\n",
        "ONE\n2\n3\n4\n5\n6\n7\n8\n",
        "1\n2\n3\n4\n5\n6\n7\nEIGHT\n",
        "1\n2\n3\nFOUR\n5\n6\n7\n8\n",
        "x\ny\nz\n",
        "completely\ndifferent\ntext\n",
    ];

    /// Optional body index per path.
    fn side() -> impl Strategy<Value = Vec<Option<usize>>> {
        proptest::collection::vec(proptest::option::weighted(0.6, 0..BODIES.len()), PATHS.len())
    }

    fn build(io: &Mem, sel: &[Option<usize>]) -> Option<Tree> {
        let entries = PATHS.iter().zip(sel).filter_map(|(p_, b)| b.map(|b| (p(p_), entry(io, BODIES[b]))));
        Tree::from_entries(entries, &lim()).ok()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(400))]

        #[test]
        fn merge_incremental_equals_merge_full(base in side(), ours in side(), theirs in side()) {
            let io = Mem::default();
            let (Some(b), Some(o), Some(t)) = (build(&io, &base), build(&io, &ours), build(&io, &theirs)) else { return Ok(()) };
            let (oc, tc) = (b.changes(&o), b.changes(&t));
            let plan = plan_renames(&b, &o, &t, &oc, &tc, &io).unwrap();
            let full = merge_full(&b, &o, &t, &plan, &io, &lim()).unwrap();
            let fast = merge_incremental(&o, &tc, &plan, &io, &lim()).unwrap();
            let structural = full.conflicts.iter().any(|c| matches!(c.kind, ConflictKind::FileDirectory { .. }));
            match fast {
                Some(f) => {
                    prop_assert!(!structural, "the fast path must decline when the result breaks the file/directory rule");
                    prop_assert_eq!(f.tree.id(), full.tree.id());
                    prop_assert_eq!(&f.conflicts, &full.conflicts);
                }
                None => prop_assert!(structural, "the fast path declined although the reference found no structural conflict"),
            }
            // And the public entry point is the reference answer either way.
            let public = merge_trees(&b, &o, &t, &io, &lim()).unwrap();
            prop_assert_eq!(public.tree.id(), full.tree.id());
            prop_assert_eq!(&public.conflicts, &full.conflicts);
        }
    }
}

// ---- the entry limit is an error, never a "clean" merge that dropped the other side ----------

mod tight_limits {
    use super::*;
    use crate::tree::{merge_full, merge_incremental, plan_renames};
    use ag_tree::TreeError;

    fn tight(max_tree_entries: usize) -> Limits {
        Limits { max_tree_entries, ..Limits::default() }
    }

    fn is_too_many(err: &IoError, entries: usize) -> bool {
        matches!(err.downcast_ref::<TreeError>(), Some(TreeError::TooManyEntries(n)) if *n == entries)
    }

    #[test]
    fn a_union_over_the_limit_is_an_error_in_every_entry_point() {
        // Ours and theirs are each valid; together they hold 6 entries, the limit is 4.
        let io = Mem::default();
        let base = tree(&io, &[("a", "1"), ("b", "1")]);
        let ours = tree(&io, &[("a", "1"), ("b", "1"), ("o1", "x"), ("o2", "x")]);
        let theirs = tree(&io, &[("a", "1"), ("b", "1"), ("t1", "y"), ("t2", "y")]);
        let limits = tight(4);

        let err = merge_trees(&base, &ours, &theirs, &io, &limits).expect_err("not a clean merge of `ours`");
        assert!(is_too_many(&err, 6), "{err}");

        let (oc, tc) = (base.changes(&ours), base.changes(&theirs));
        let plan = plan_renames(&base, &ours, &theirs, &oc, &tc, &io).unwrap();
        let err = merge_full(&base, &ours, &theirs, &plan, &io, &limits).expect_err("the reference algorithm");
        assert!(is_too_many(&err, 6), "{err}");
        let err = merge_incremental(&ours, &tc, &plan, &io, &limits).expect_err("the fast path");
        assert!(is_too_many(&err, 6), "{err}");

        // With room for the union the same inputs merge cleanly and keep both sides' files.
        let r = merge_trees(&base, &ours, &theirs, &io, &tight(6)).unwrap();
        assert!(r.is_clean(), "{:?}", r.conflicts);
        assert_eq!(r.tree.len(), 6);
        assert!(r.tree.get(&p("o1")).is_some() && r.tree.get(&p("t2")).is_some());
    }

    #[test]
    fn a_file_directory_collision_is_still_a_conflict_not_an_error() {
        // The fallback to `ours` is for this case only, and only because a conflict says why.
        let io = Mem::default();
        let base = tree(&io, &[("x", "1")]);
        let ours = tree(&io, &[("x", "1"), ("d", "file")]);
        let theirs = tree(&io, &[("x", "1"), ("d/inner", "dir")]);
        let r = merge_trees(&base, &ours, &theirs, &io, &Limits::default()).unwrap();
        assert!(r.conflicts.iter().any(|c| matches!(c.kind, ConflictKind::FileDirectory { .. })), "{:?}", r.conflicts);
        assert_eq!(r.tree.id(), ours.id(), "the best-effort tree is ours");
    }

    const PATHS: &[&str] = &["a", "b", "c", "d", "e/x", "e/y", "f", "g/h"];

    fn side() -> impl Strategy<Value = Vec<bool>> {
        proptest::collection::vec(any::<bool>(), PATHS.len())
    }

    fn build(io: &Mem, sel: &[bool], body: &str) -> Tree {
        let entries = PATHS.iter().zip(sel).filter(|(_, on)| **on).map(|(p_, _)| (p(p_), entry(io, body)));
        Tree::from_entries(entries, &Limits::default()).unwrap()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(400))]

        /// Whatever the limit, the two strategies agree about whether the merge fits, and a merge that is
        /// returned always fits: there is no limit at which a side's changes vanish without a word.
        #[test]
        fn strategies_agree_on_what_fits(base in side(), ours in side(), theirs in side(), max in 1usize..10) {
            let io = Mem::default();
            let (b, o, t) = (build(&io, &base, "same\n"), build(&io, &ours, "ours\n"), build(&io, &theirs, "theirs\n"));
            let limits = tight(max);
            let (oc, tc) = (b.changes(&o), b.changes(&t));
            let plan = plan_renames(&b, &o, &t, &oc, &tc, &io).unwrap();
            let full = merge_full(&b, &o, &t, &plan, &io, &limits);
            let fast = merge_incremental(&o, &tc, &plan, &io, &limits);
            match (&fast, &full) {
                (Ok(Some(f)), Ok(full)) => {
                    prop_assert_eq!(f.tree.id(), full.tree.id());
                    prop_assert_eq!(&f.conflicts, &full.conflicts);
                    prop_assert!(full.tree.len() <= max);
                }
                // The fast path hands over only a file/directory collision, and the reference then reports it.
                (Ok(None), Ok(full)) => prop_assert!(
                    full.conflicts.iter().any(|c| matches!(c.kind, ConflictKind::FileDirectory { .. })),
                    "declined without a structural conflict"
                ),
                (Err(_), Err(_)) => {}
                (fast, full) => prop_assert!(
                    false,
                    "disagree: fast {:?}, full {:?}",
                    fast.as_ref().map(|r| r.is_some()),
                    full.as_ref().map(|_| ())
                ),
            }
            if let Ok(merged) = merge_trees(&b, &o, &t, &io, &limits) {
                prop_assert!(merged.tree.len() <= max);
            }
        }
    }
}

#[test]
fn algorithm_version_embeds_the_diff_version() {
    // The tag is an explicit literal on purpose (changing it changes the ids of merge changesets); this keeps it
    // from going stale when the diff algorithm's version is bumped.
    assert!(
        crate::ALGORITHM_VERSION.ends_with(&format!("+{}", ag_diff::ALGORITHM_VERSION)),
        "bump ag-merge's ALGORITHM_VERSION when ag-diff's changes: {} vs {}",
        crate::ALGORITHM_VERSION,
        ag_diff::ALGORITHM_VERSION
    );
}

#[test]
fn every_conflict_kind_has_its_own_stable_code() {
    let all = [
        (ConflictKind::Content { regions: 2 }, "content"),
        (ConflictKind::BothAdded, "both_added"),
        (ConflictKind::DeleteModify { ours_deleted: true }, "delete_modify"),
        (ConflictKind::DeleteModify { ours_deleted: false }, "delete_modify"),
        (ConflictKind::Binary, "binary"),
        (ConflictKind::TypeChange, "type_change"),
        (ConflictKind::TooLarge, "too_large"),
        (ConflictKind::FileDirectory { inside: RepoPath::new("a/b").unwrap() }, "file_directory"),
        (
            ConflictKind::RenameRename { ours: RepoPath::new("x").unwrap(), theirs: RepoPath::new("y").unwrap() },
            "rename_rename",
        ),
        (ConflictKind::RenameDelete { ours_renamed: true }, "rename_delete"),
        (ConflictKind::RenameDelete { ours_renamed: false }, "rename_delete"),
    ];
    for (kind, code) in &all {
        assert_eq!(kind.code(), *code);
        assert!(code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{code}");
        assert_ne!(kind.to_string(), *code, "the prose is separate from the code");
    }
    let distinct: std::collections::BTreeSet<_> = all.iter().map(|(_, c)| *c).collect();
    assert_eq!(distinct.len(), 9, "nine kinds, nine codes");
}
