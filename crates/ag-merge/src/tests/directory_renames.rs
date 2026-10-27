//! Directory renames (`ag-merge/3`): scenarios, and properties against the retained references.

use super::*;
use crate::rename::exact_pairs;
use crate::tree::{merge_full, merge_incremental, plan_renames};

fn v3(base: &Tree, ours: &Tree, theirs: &Tree, io: &Mem) -> TreeMerge {
    merge_trees_with(Algorithm::V3, base, ours, theirs, io, &lim()).unwrap()
}
fn v2(base: &Tree, ours: &Tree, theirs: &Tree, io: &Mem) -> TreeMerge {
    merge_trees_with(Algorithm::V2, base, ours, theirs, io, &lim()).unwrap()
}

type Files = Vec<(String, String)>;

fn lib() -> Files {
    vec![("lib/a.rs".into(), lines("a", 8)), ("lib/b.rs".into(), lines("b", 8)), ("lib/c.rs".into(), lines("c", 8))]
}
fn mk(io: &Mem, list: &[(String, String)]) -> Tree {
    let refs: Vec<(&str, &str)> = list.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
    tree(io, &refs)
}
/// `lib/…` of the list moved to `to/…`.
fn moved(list: &[(String, String)], to: &str) -> Files {
    list.iter().map(|(p, c)| (p.replacen("lib/", &format!("{to}/"), 1), c.clone())).collect()
}
fn with(mut list: Files, extra: &[(&str, &str)]) -> Files {
    for (p, c) in extra {
        list.retain(|(q, _)| q != p);
        list.push((p.to_string(), c.to_string()));
    }
    list
}

#[test]
fn a_file_added_beneath_a_renamed_directory_follows_it_in_both_directions() {
    let io = Mem::default();
    let base_files = lib();
    let base = mk(&io, &base_files);
    let renamed = mk(&io, &moved(&base_files, "src"));
    let added = mk(&io, &with(base_files.clone(), &[("lib/new.rs", "brand new\n")]));

    for ours_renamed in [true, false] {
        let (ours, theirs) = if ours_renamed { (&renamed, &added) } else { (&added, &renamed) };
        let r = v3(&base, ours, theirs, &io);
        assert!(r.is_clean(), "{:?}", r.conflicts);
        assert_eq!(content(&io, &r.tree, "src/new.rs"), "brand new\n");
        assert!(r.tree.get(&p("lib/new.rs")).is_none(), "nothing is left behind in the old directory");
        assert_eq!(r.tree.len(), 4);
        let [
            MergeNote::DirectoryRename {
                renamed_by,
                from,
                to,
                moved_files,
                total_files,
                relocated,
                relocated_count,
                ..
            },
        ] = r.notes.as_slice()
        else {
            panic!("{:?}", r.notes)
        };
        assert_eq!(*renamed_by, if ours_renamed { Side::Ours } else { Side::Theirs });
        assert_eq!((from.as_str(), to.as_str()), ("lib", "src"));
        assert_eq!((*moved_files, *total_files, *relocated_count), (3, 3, 1));
        assert_eq!(relocated, &[(p("lib/new.rs"), p("src/new.rs"))]);
        assert_eq!(r.notes[0].code(), "directory_rename");
        assert!(r.notes[0].to_string().contains("'lib' was renamed to 'src'"), "{}", r.notes[0]);

        // Under ag-merge/2 the new file stays where it was added and the old directory lives on.
        let old = v2(&base, ours, theirs, &io);
        assert!(old.is_clean() && old.notes.is_empty());
        assert!(old.tree.get(&p("lib/new.rs")).is_some() && old.tree.get(&p("src/new.rs")).is_none());
    }
}

#[test]
fn nested_directories_and_new_subdirectories_move_along() {
    let io = Mem::default();
    let mut base_files = lib();
    base_files.push(("lib/x/p.rs".into(), lines("p", 6)));
    base_files.push(("lib/x/q.rs".into(), lines("q", 6)));
    let base = mk(&io, &base_files);
    // lib/ goes to src/, except lib/x/, which goes to other/x/: the deepest vacated directory decides.
    let ours_files: Files = base_files
        .iter()
        .map(|(p_, c)| {
            let np = if p_.starts_with("lib/x/") {
                p_.replacen("lib/x/", "other/x/", 1)
            } else {
                p_.replacen("lib/", "src/", 1)
            };
            (np, c.clone())
        })
        .collect();
    let ours = mk(&io, &ours_files);
    let theirs = mk(
        &io,
        &with(
            base_files.clone(),
            &[("lib/x/new.rs", "in x\n"), ("lib/new.rs", "top\n"), ("lib/deep/er/f.rs", "deep\n")],
        ),
    );
    let r = v3(&base, &ours, &theirs, &io);
    assert!(r.is_clean(), "{:?}", r.conflicts);
    assert_eq!(content(&io, &r.tree, "other/x/new.rs"), "in x\n");
    assert_eq!(content(&io, &r.tree, "src/new.rs"), "top\n");
    assert_eq!(content(&io, &r.tree, "src/deep/er/f.rs"), "deep\n");
    assert!(r.tree.iter().all(|(p_, _)| !p_.as_str().starts_with("lib/")));
    let dirs: Vec<(&str, &str)> = r
        .notes
        .iter()
        .map(|n| match n {
            MergeNote::DirectoryRename { from, to, .. } => (from.as_str(), to.as_str()),
        })
        .collect();
    assert_eq!(dirs, vec![("lib", "src"), ("lib/x", "other/x")]);
}

#[test]
fn an_edit_of_a_file_rewritten_while_moving_follows_the_directory() {
    let io = Mem::default();
    let ten: String = (1..=10).map(|i| format!("old {i}\n")).collect();
    let mut base_files = lib();
    base_files.push(("lib/big.rs".into(), ten.clone()));
    let base = mk(&io, &base_files);
    // ours moves the directory and rewrites big.rs so little is left of it (36 % in common)
    let rewritten = format!("{}{}", (7..=10).map(|i| format!("old {i}\n")).collect::<String>(), lines("fresh", 8));
    let ours = mk(&io, &with(moved(&base_files, "src"), &[("src/big.rs", &rewritten)]));
    let theirs_text = ten.replace("old 8\n", "theirs 8\n");
    let theirs = mk(&io, &with(base_files.clone(), &[("lib/big.rs", &theirs_text)]));

    let r = v3(&base, &ours, &theirs, &io);
    assert!(r.is_clean(), "{:?}", r.conflicts);
    assert_eq!(content(&io, &r.tree, "src/big.rs"), rewritten.replace("old 8\n", "theirs 8\n"));
    assert!(r.tree.get(&p("lib/big.rs")).is_none());
    let [MergeNote::DirectoryRename { followed_edits, relocated_count, .. }] = r.notes.as_slice() else {
        panic!("{:?}", r.notes)
    };
    assert_eq!((*followed_edits, *relocated_count), (1, 0));
    // ag-merge/2 could not tell that src/big.rs is lib/big.rs: a delete/modify conflict at the old path
    let old = v2(&base, &ours, &theirs, &io);
    assert!(old.conflicts.iter().any(|c| c.path == p("lib/big.rs") && c.kind.code() == "delete_modify"));
}

#[test]
fn files_that_left_for_several_places_or_mostly_not_at_all_are_ambiguous_never_guessed() {
    let io = Mem::default();
    let mut four = lib();
    four.push(("lib/d.rs".into(), lines("d", 8)));
    let base = mk(&io, &four);
    let theirs = mk(&io, &with(four.clone(), &[("lib/new.rs", "n\n")]));

    // two files went to src/, two to other/: no majority
    let split: Files = four
        .iter()
        .map(|(p_, c)| (p_.replacen("lib/", if p_.as_str() < "lib/c.rs" { "src/" } else { "other/" }, 1), c.clone()))
        .collect();
    // one went to src/, three were deleted: a rename of one file, not of the directory
    let few = moved(&four[..1], "src");
    for ours in [mk(&io, &split), mk(&io, &few)] {
        let r = v3(&base, &ours, &theirs, &io);
        let [c] = r.conflicts.as_slice() else { panic!("{:?}", r.conflicts) };
        assert_eq!((c.path.as_str(), c.kind.code()), ("lib/new.rs", "directory_rename_ambiguous"));
        let ConflictKind::DirectoryRenameAmbiguous { ours_renamed, directory, candidates } = &c.kind else { panic!() };
        assert!(*ours_renamed && directory.as_str() == "lib" && !candidates.is_empty());
        assert!(c.kind.to_string().contains("added on theirs beneath 'lib'"), "{}", c.kind);
        // the best-effort tree keeps the added file where it was added
        assert!(r.tree.get(&p("lib/new.rs")).is_some());
        assert!(r.notes.is_empty());
        // ag-merge/2 merged it without a word (the file stays in the old directory)
        assert!(v2(&base, &ours, &theirs, &io).is_clean());
    }
}

#[test]
fn a_directory_that_was_only_deleted_is_not_renamed() {
    let io = Mem::default();
    let base_files = lib();
    let base = mk(&io, &base_files);
    let ours = mk(&io, &[]);
    let theirs = mk(&io, &with(base_files, &[("lib/new.rs", "n\n")]));
    let r = v3(&base, &ours, &theirs, &io);
    assert!(r.is_clean(), "{:?}", r.conflicts);
    assert!(r.tree.get(&p("lib/new.rs")).is_some());
    assert_eq!(r.tree.len(), 1);
    assert!(r.notes.is_empty());
}

#[test]
fn a_directory_that_still_has_files_is_not_vacated() {
    let io = Mem::default();
    let base_files = lib();
    let base = mk(&io, &base_files);
    let mut keep = moved(&base_files[..2], "src");
    keep.push(base_files[2].clone());
    let ours = mk(&io, &keep);
    let theirs = mk(&io, &with(base_files, &[("lib/new.rs", "n\n")]));
    let r = v3(&base, &ours, &theirs, &io);
    assert!(r.is_clean(), "{:?}", r.conflicts);
    assert!(r.tree.get(&p("lib/new.rs")).is_some() && r.tree.get(&p("src/new.rs")).is_none());
    assert!(r.notes.is_empty());
}

#[test]
fn a_target_that_is_already_taken_conflicts() {
    let io = Mem::default();
    let base_files = lib();
    let base = mk(&io, &base_files);
    // we renamed lib/ to src/ and also created src/new.rs; they added lib/new.rs
    let ours = mk(&io, &with(moved(&base_files, "src"), &[("src/new.rs", "ours\n")]));
    let theirs = mk(&io, &with(base_files, &[("lib/new.rs", "theirs\n")]));
    let r = v3(&base, &ours, &theirs, &io);
    let [c] = r.conflicts.as_slice() else { panic!("{:?}", r.conflicts) };
    assert_eq!((c.path.as_str(), c.kind.code()), ("lib/new.rs", "directory_rename_collision"));
    assert!(c.kind.to_string().contains("'src/new.rs' is already taken"), "{}", c.kind);
    assert_eq!(content(&io, &r.tree, "src/new.rs"), "ours\n");
}

#[test]
fn two_files_heading_for_one_target_both_conflict() {
    let io = Mem::default();
    // lib/ and lib2/ were both emptied into src/ by us; they added lib/n.rs and lib2/n.rs
    let base_files: Files = vec![
        ("lib/a.rs".into(), lines("a", 6)),
        ("lib/b.rs".into(), lines("b", 6)),
        ("lib2/c.rs".into(), lines("c", 6)),
        ("lib2/d.rs".into(), lines("d", 6)),
    ];
    let base = mk(&io, &base_files);
    let ours_files: Files = base_files
        .iter()
        .map(|(p_, c)| (p_.replacen("lib2/", "src/", 1).replacen("lib/", "src/", 1), c.clone()))
        .collect();
    let ours = mk(&io, &ours_files);
    let theirs = mk(&io, &with(base_files, &[("lib/n.rs", "one\n"), ("lib2/n.rs", "two\n")]));
    let r = v3(&base, &ours, &theirs, &io);
    let codes: Vec<_> = r.conflicts.iter().map(|c| (c.path.as_str(), c.kind.code())).collect();
    assert_eq!(codes, vec![("lib/n.rs", "directory_rename_collision"), ("lib2/n.rs", "directory_rename_collision")]);
}

#[test]
fn both_sides_moving_the_directory_apart_conflict_per_file_as_before() {
    let io = Mem::default();
    let base_files = lib();
    let base = mk(&io, &base_files);
    let ours = mk(&io, &moved(&base_files, "src"));
    let theirs = mk(&io, &moved(&base_files, "app"));
    let r = v3(&base, &ours, &theirs, &io);
    assert_eq!(r.conflicts.len(), 3);
    assert!(r.conflicts.iter().all(|c| c.kind.code() == "rename_rename"));
    let same = v3(&base, &ours, &ours, &io);
    assert!(same.is_clean() && same.notes.is_empty());
}

#[test]
fn the_algorithm_tags_round_trip_and_the_old_one_stays_runnable() {
    assert_eq!(ALGORITHM_VERSION, "ag-merge/3+ag-diff/1");
    assert_eq!(ALGORITHM_VERSION_V2, "ag-merge/2+ag-diff/1");
    assert_eq!(Algorithm::CURRENT.version(), ALGORITHM_VERSION);
    for a in [Algorithm::V2, Algorithm::V3] {
        assert_eq!(Algorithm::parse(a.version()), Some(a));
    }
    assert_eq!(Algorithm::parse("ag-merge/1+ag-diff/1"), None);
    assert_eq!(Algorithm::parse("ag-merge/3"), None);
    assert!(!Algorithm::V2.follows_directory_renames() && Algorithm::V3.follows_directory_renames());
}

// ---- properties ---------------------------------------------------------------------------------------

/// The quadratic definition of the exact pairing (every deleted file against every added one, sorted, greedy)
/// that [`exact_pairs`] replaced.
fn exact_pairs_reference<'a>(
    deleted: &[(&'a RepoPath, &Entry)],
    added: &[(&'a RepoPath, &Entry)],
) -> Vec<(&'a RepoPath, &'a RepoPath)> {
    let mut all: Vec<(&RepoPath, &RepoPath)> = Vec::new();
    for (f, fe) in deleted {
        for (t, te) in added {
            if fe.content == te.content {
                all.push((f, t));
            }
        }
    }
    all.sort_by(|(af, at), (bf, bt)| {
        let a_same = af.file_name() == at.file_name();
        let b_same = bf.file_name() == bt.file_name();
        b_same.cmp(&a_same).then_with(|| af.cmp(bf)).then_with(|| at.cmp(bt))
    });
    let (mut uf, mut ut) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
    let mut out = Vec::new();
    for (f, t) in all {
        if uf.contains(f) || ut.contains(t) {
            continue;
        }
        uf.insert(f);
        ut.insert(t);
        out.push((f, t));
    }
    out
}

const NAMES: &[&str] = &["a/x", "a/y", "b/x", "b/z", "c/x", "d/q/x", "e/q/y", "x", "y"];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// The linear exact pairing picks exactly the pairs of the quadratic definition.
    #[test]
    fn exact_pairing_matches_the_quadratic_reference(
        del in proptest::collection::vec((0..NAMES.len(), 0u8..3), 0..9),
        add in proptest::collection::vec((0..NAMES.len(), 0u8..3), 0..9),
    ) {
        let io = Mem::default();
        let build = |v: &[(usize, u8)]| -> Vec<(RepoPath, Entry)> {
            let mut seen = std::collections::BTreeSet::new();
            v.iter()
                .filter(|(n, _)| seen.insert(*n))
                .map(|(n, c)| (p(NAMES[*n]), entry(&io, &format!("content {c}\n"))))
                .collect()
        };
        let (d, a) = (build(&del), build(&add));
        let dr: Vec<(&RepoPath, &Entry)> = d.iter().map(|(p_, e)| (p_, e)).collect();
        let ar: Vec<(&RepoPath, &Entry)> = a.iter().map(|(p_, e)| (p_, e)).collect();
        let mut fast = exact_pairs(&dr, &ar);
        let mut slow = exact_pairs_reference(&dr, &ar);
        fast.sort();
        slow.sort();
        prop_assert_eq!(fast, slow);
    }
}

#[test]
fn the_linear_exact_pairing_handles_thousands_of_identical_files() {
    let io = Mem::default();
    let e = entry(&io, "");
    let d: Vec<(RepoPath, Entry)> = (0..20_000).map(|i| (p(&format!("old/d{i}/__init__.py")), e)).collect();
    let a: Vec<(RepoPath, Entry)> = (0..20_000).map(|i| (p(&format!("new/d{i}/__init__.py")), e)).collect();
    let dr: Vec<_> = d.iter().map(|(p_, e)| (p_, e)).collect();
    let ar: Vec<_> = a.iter().map(|(p_, e)| (p_, e)).collect();
    let pairs = exact_pairs(&dr, &ar);
    assert_eq!(pairs.len(), 20_000);
    // same file name everywhere, so the sorted old paths pair with the sorted new paths
    assert!(pairs.iter().all(|(f, t)| f.as_str()["old/".len()..] == t.as_str()["new/".len()..]));
}

const SIDE_PATHS: &[&str] = &["lib/a", "lib/b", "lib/c", "lib/n/d", "src/a", "src/b", "src/c", "src/n/d", "top"];
const SIDE_BODIES: &[&str] =
    &["1\n2\n3\n4\n5\n6\n7\n8\n", "ONE\n2\n3\n4\n5\n6\n7\n8\n", "1\n2\n3\n4\n5\n6\n7\nEIGHT\n", "x\ny\nz\n"];

fn sel() -> impl Strategy<Value = Vec<Option<usize>>> {
    proptest::collection::vec(proptest::option::weighted(0.55, 0..SIDE_BODIES.len()), SIDE_PATHS.len())
}
fn build(io: &Mem, sel: &[Option<usize>]) -> Tree {
    let entries = SIDE_PATHS.iter().zip(sel).filter_map(|(p_, b)| b.map(|b| (p(p_), entry(io, SIDE_BODIES[b]))));
    Tree::from_entries(entries, &lim()).unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    /// On trees full of directory moves: the fast path equals the reference, the result is deterministic, and
    /// whenever ag-merge/3 reports no directory rename (no note, no directory conflict) it gives exactly the
    /// ag-merge/2 answer.
    #[test]
    fn v3_is_v2_unless_it_says_otherwise(base in sel(), ours in sel(), theirs in sel()) {
        let io = Mem::default();
        let (b, o, t) = (build(&io, &base), build(&io, &ours), build(&io, &theirs));
        let (oc, tc) = (b.changes(&o), b.changes(&t));
        let plan = plan_renames(Algorithm::V3, &b, &o, &t, &oc, &tc, &io).unwrap();
        let full = merge_full(&b, &o, &t, &plan, &io, &lim()).unwrap();
        let fast = merge_incremental(&o, &tc, &plan, &io, &lim()).unwrap();
        let structural = full.conflicts.iter().any(|c| matches!(c.kind, ConflictKind::FileDirectory { .. }));
        match fast {
            Some(f) => {
                prop_assert!(!structural);
                prop_assert_eq!(f.tree.id(), full.tree.id());
                prop_assert_eq!(&f.conflicts, &full.conflicts);
                prop_assert_eq!(&f.notes, &full.notes);
            }
            None => prop_assert!(structural),
        }
        let public = merge_trees(&b, &o, &t, &io, &lim()).unwrap();
        prop_assert_eq!(public.tree.id(), full.tree.id());
        prop_assert_eq!(&public.conflicts, &full.conflicts);
        prop_assert_eq!(&public.notes, &full.notes);

        let dir_conflict = |c: &Conflict| c.kind.code().starts_with("directory_rename");
        if public.notes.is_empty() && !public.conflicts.iter().any(dir_conflict) {
            let old = merge_trees_with(Algorithm::V2, &b, &o, &t, &io, &lim()).unwrap();
            prop_assert_eq!(public.tree.id(), old.tree.id());
            prop_assert_eq!(&public.conflicts, &old.conflicts);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// A reference model for the clean case: relocating their additions and edits beneath the old directory by
    /// hand and merging with the old algorithm gives the same tree.
    #[test]
    fn following_a_directory_move_equals_moving_the_other_side_by_hand(
        n in 1usize..6,
        edits in proptest::collection::vec(0u8..3, 6),
        adds in 0usize..4,
        nested_add in any::<bool>(),
        ours_renames in any::<bool>(),
    ) {
        let io = Mem::default();
        let body = |i: usize| format!("file {i}\nwith some\nlines {i}\n");
        let base_files: Files = (0..n).map(|i| (format!("lib/f{i}.txt"), body(i))).collect();
        let to_src = |v: &[(String, String)]| -> Files {
            v.iter().map(|(p_, c)| (p_.replacen("lib/", "src/", 1), c.clone())).collect()
        };
        // the other side edits (appends to) some files and adds some, some in a new subdirectory
        let mut other: Files = base_files
            .iter()
            .enumerate()
            .map(|(i, (p_, c))| if edits[i] == 1 { (p_.clone(), format!("{c}appended\n")) } else { (p_.clone(), c.clone()) })
            .collect();
        for j in 0..adds {
            let dir = if nested_add && j % 2 == 1 { "sub/" } else { "" };
            other.push((format!("lib/{dir}added{j}.txt"), format!("added {j}\n")));
        }
        let renamed = to_src(&base_files);
        let (b, renamer, other_t) = (mk(&io, &base_files), mk(&io, &renamed), mk(&io, &other));
        let (ours, theirs) = if ours_renames { (&renamer, &other_t) } else { (&other_t, &renamer) };
        let got = merge_trees_with(Algorithm::V3, &b, ours, theirs, &io, &lim()).unwrap();

        // by hand: move the other side and the base, then use the old algorithm
        let by_hand_base = mk(&io, &renamed);
        let other_moved = mk(&io, &to_src(&other));
        let (o2, t2) = if ours_renames { (&renamer, &other_moved) } else { (&other_moved, &renamer) };
        let want = merge_trees_with(Algorithm::V2, &by_hand_base, o2, t2, &io, &lim()).unwrap();
        prop_assert!(got.is_clean(), "{:?}", got.conflicts);
        prop_assert!(want.is_clean());
        prop_assert_eq!(got.tree.id(), want.tree.id());
        prop_assert_eq!(got.notes.len(), usize::from(adds > 0));
    }
}

// ---- copies -----------------------------------------------------------------------------------------

mod copies {
    use super::*;

    fn e(io: &Mem, content: &str) -> (Entry, String) {
        (entry(io, content), content.to_string())
    }
    fn set(io: &Mem, items: &[(&str, &str)]) -> Vec<(RepoPath, Entry)> {
        items.iter().map(|(path, c)| (p(path), e(io, c).0)).collect()
    }
    fn found(r: &[Rename]) -> Vec<(String, String, u32)> {
        r.iter().map(|r| (r.from.to_string(), r.to.to_string(), r.similarity)).collect()
    }

    #[test]
    fn identical_content_is_a_copy_and_the_source_may_serve_several_targets() {
        let io = Mem::default();
        let src = set(&io, &[("a/x.rs", "one\ntwo\n"), ("b/x.rs", "one\ntwo\n"), ("c.rs", "other\n")]);
        let targets = set(&io, &[("n/x.rs", "one\ntwo\n"), ("m/y.rs", "one\ntwo\n")]);
        let r = detect_copies(&src, &[], &targets, &io).unwrap();
        // the same file name wins, then the lowest path; one source for both targets is fine
        assert_eq!(
            found(&r),
            [("a/x.rs".to_string(), "m/y.rs".to_string(), 100), ("a/x.rs".to_string(), "n/x.rs".to_string(), 100)]
        );
    }

    #[test]
    fn a_similar_copy_needs_half_of_the_bytes_and_the_best_source_wins() {
        let io = Mem::default();
        let ten: String = (0..10).map(|i| format!("line {i}\n")).collect();
        let nine: String = (0..9).map(|i| format!("line {i}\n")).collect::<String>() + "other\n";
        let five: String = (0..5).map(|i| format!("line {i}\n")).chain((0..5).map(|i| format!("zz {i}\n"))).collect();
        let sim = set(&io, &[("ten.txt", &ten), ("five.txt", &five)]);
        let targets = set(&io, &[("copy.txt", &nine)]);
        let r = detect_copies(&sim, &sim, &targets, &io).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].from.as_str(), r[0].similarity >= 85), ("ten.txt", true));

        // exactly the threshold counts: 5 of 10 equal-length lines is 50 %
        let half: String = (0..5).map(|i| format!("line {i}\n")).chain((0..5).map(|i| format!("new {i}\n"))).collect();
        let only_five = set(&io, &[("ten.txt", &ten)]);
        let r = detect_copies(&only_five, &only_five, &set(&io, &[("h.txt", &half)]), &io).unwrap();
        assert_eq!(r.len(), 1);
        assert!(r[0].similarity >= MIN_COPY_SIMILARITY);
        // one line fewer in common is not enough
        let less: String = (0..4).map(|i| format!("line {i}\n")).chain((0..6).map(|i| format!("new {i}\n"))).collect();
        assert!(detect_copies(&only_five, &only_five, &set(&io, &[("l.txt", &less)]), &io).unwrap().is_empty());
    }

    #[test]
    fn similarity_is_only_tried_for_a_bounded_number_of_candidates() {
        let io = Mem::default();
        let near = "line a\nline b\nline c\nline d\n";
        let items: Vec<(String, String)> =
            (0..=MAX_COPY_SOURCES).map(|i| (format!("s{i:03}.txt"), format!("{near}x{i}\n"))).collect();
        let sources: Vec<(RepoPath, Entry)> = items.iter().map(|(path, c)| (p(path), entry(&io, c))).collect();
        let targets = set(&io, &[("t.txt", &format!("{near}t\n"))]);
        // one candidate too many: no similarity search, and the content is not identical
        assert!(detect_copies(&sources, &sources, &targets, &io).unwrap().is_empty());
        // within the bound the most similar candidate wins (the shortest tail here)
        let r = detect_copies(&sources, &sources[..MAX_COPY_SOURCES], &targets, &io).unwrap();
        assert_eq!(r[0].from.as_str(), "s000.txt");
    }

    #[test]
    fn binary_and_empty_files_only_copy_exactly_and_a_file_is_no_copy_of_itself() {
        let io = Mem::default();
        let src = set(&io, &[("bin", "a\0b\nc\nd\n"), ("empty", ""), ("t.txt", "same\nlines\n")]);
        let targets = set(&io, &[("bin2", "a\0b\nc\ne\n"), ("empty2", ""), ("t.txt", "same\nlines\n")]);
        let r = detect_copies(&src, &src, &targets, &io).unwrap();
        // "empty2" is an exact copy of "empty"; the binary file is not similar-matched; t.txt does not copy itself
        assert_eq!(found(&r), [("empty".to_string(), "empty2".to_string(), 100)]);
    }
}
