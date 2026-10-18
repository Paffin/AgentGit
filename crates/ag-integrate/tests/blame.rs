//! Line-level blame over real histories, checked against an independent invariant.
#![allow(missing_docs)]

mod common;

use ag_integrate::{Blame, BlameOptions, Repository};
use ag_tree::Tree;
use ag_types::{ChangesetId, FileType, RepoPath};
use common::*;

/// A changeset on `parents` whose complete state is `files`.
fn step(f: &Fx, parents: &[ChangesetId], files: &[(&str, &str)], who: &str, message: &str) -> ChangesetId {
    let base = f.repo.tree_or_empty(parents.first()).unwrap();
    let want = Tree::from_entries(
        files.iter().map(|(p, c)| {
            (
                RepoPath::new(*p).unwrap(),
                ag_tree::Entry {
                    content: f.repo.put_blob(c.as_bytes().to_vec()).unwrap(),
                    file_type: FileType::Regular,
                    executable: false,
                },
            )
        }),
        f.repo.limits(),
    )
    .unwrap();
    f.repo.commit(parents.to_vec(), &id(who), message, base.diff(&want)).unwrap()
}

fn blame(f: &Fx, rev: &ChangesetId, path: &str) -> Blame {
    let b = f.repo.blame(rev, path, &BlameOptions::default()).unwrap();
    assert_consistent(&f.repo, &b);
    b
}

/// The defining property of blame: every line's text really is that line of the attributed changeset's file.
fn assert_consistent(repo: &Repository, b: &Blame) {
    let final_text = {
        let t = repo.tree_of(&b.rev).unwrap();
        String::from_utf8(repo.blob(&t.get(&RepoPath::new(&b.path).unwrap()).unwrap().content).unwrap()).unwrap()
    };
    let final_lines: Vec<&str> = final_text.lines().collect();
    for l in &b.lines {
        assert_eq!(l.text, final_lines[l.line - 1], "line {} text", l.line);
        if l.boundary {
            continue;
        }
        let t = repo.tree_of(&l.changeset).unwrap();
        let e = t
            .get(&RepoPath::new(&l.path).unwrap())
            .unwrap_or_else(|| panic!("{} has no {}", l.changeset.short(), l.path));
        let there = String::from_utf8(repo.blob(&e.content).unwrap()).unwrap();
        assert_eq!(
            there.lines().nth(l.original_line - 1),
            Some(l.text.as_str()),
            "line {} is not in {}",
            l.line,
            l.changeset.short()
        );
    }
}

fn who(b: &Blame) -> Vec<String> {
    b.lines.iter().map(|l| format!("{}:{}", l.author.trim_start_matches("user:"), l.summary)).collect()
}

#[test]
fn lines_are_attributed_to_the_changeset_that_introduced_them() {
    let f = fx();
    let c1 = step(&f, &[], &[("f.txt", "one\ntwo\nthree\n")], "user:alice", "create");
    let c2 = step(&f, &[c1], &[("f.txt", "one\nTWO\nthree\n"), ("other", "x\n")], "user:bob", "shout two");
    let c3 = step(&f, &[c2], &[("f.txt", "one\nTWO\nthree\nfour\n"), ("other", "x\n")], "user:carol", "add four");
    let c4 = step(&f, &[c3], &[("f.txt", "one\nTWO\nthree\nfour\n"), ("other", "y\n")], "user:dave", "unrelated");

    let b = blame(&f, &c4, "f.txt");
    assert_eq!(who(&b), ["alice:create", "bob:shout two", "alice:create", "carol:add four"]);
    assert_eq!(b.lines[1].changeset, c2);
    assert_eq!(b.lines[3].original_line, 4);
    assert!(!b.truncated);
    // The unrelated changeset was walked past without touching the file.
    assert!(b.visited >= 4);
    // Blaming an older revision answers as of that revision.
    let old = blame(&f, &c2, "f.txt");
    assert_eq!(who(&old), ["alice:create", "bob:shout two", "alice:create"]);
}

#[test]
fn a_line_moved_within_the_file_is_attributed_by_the_diff_not_by_content_search() {
    let f = fx();
    let c1 = step(&f, &[], &[("f", "a\nb\nc\nd\n")], "user:alice", "create");
    // bob deletes b and appends it at the end: the diff sees a delete and an insert.
    let c2 = step(&f, &[c1], &[("f", "a\nc\nd\nb\n")], "user:bob", "move b");
    let b = blame(&f, &c2, "f");
    assert_eq!(who(&b), ["alice:create", "alice:create", "alice:create", "bob:move b"]);
}

#[test]
fn renames_are_followed_through_the_history() {
    let f = fx();
    let body: String = (1..=10).map(|i| format!("line {i}\n")).collect();
    let c1 = step(&f, &[], &[("src/old.rs", &body)], "user:alice", "create");
    let moved = body.replace("line 10", "line 10 edited");
    let c2 = step(&f, &[c1], &[("lib/new.rs", &moved)], "user:bob", "move and edit");
    let c3 = step(&f, &[c2], &[("lib/new.rs", &format!("{moved}tail\n"))], "user:carol", "tail");

    let b = blame(&f, &c3, "lib/new.rs");
    assert_eq!(b.lines[0].changeset, c1, "line 1 predates the rename");
    assert_eq!(b.lines[0].path, "src/old.rs", "and is reported under its old name");
    assert_eq!(b.lines[9].changeset, c2);
    assert_eq!(b.lines[9].path, "lib/new.rs");
    assert_eq!(b.lines[10].changeset, c3);
}

#[test]
fn merges_pass_lines_to_the_side_that_had_them_and_own_only_what_they_wrote() {
    let f = fx();
    let base = step(&f, &[], &[("f", "1\n2\n3\n4\n5\n6\n7\n8\n")], "user:alice", "base");
    let left = step(&f, &[base], &[("f", "1\nLEFT\n3\n4\n5\n6\n7\n8\n")], "user:bob", "left edit");
    let right = step(&f, &[base], &[("f", "1\n2\n3\n4\n5\n6\nRIGHT\n8\n")], "user:carol", "right edit");
    // A clean merge: ops are relative to the first parent (left).
    let merged = step(&f, &[left, right], &[("f", "1\nLEFT\n3\n4\n5\n6\nRIGHT\n8\n")], "user:dave", "merge");
    let b = blame(&f, &merged, "f");
    assert_eq!(b.lines[1].changeset, left);
    assert_eq!(b.lines[6].changeset, right, "second-parent lines keep their origin");
    assert_eq!(b.lines[0].changeset, base);
    assert!(b.lines.iter().all(|l| l.changeset != merged));

    // A merge that had to write its own text takes the blame for it.
    let resolved = step(&f, &[left, right], &[("f", "1\nLEFT+RIGHT\n3\n4\n5\n6\nRIGHT\n8\n")], "user:dave", "resolve");
    let b = blame(&f, &resolved, "f");
    assert_eq!(b.lines[1].changeset, resolved);
    assert_eq!(b.lines[6].changeset, right);
}

#[test]
fn ranges_and_limits() {
    let f = fx();
    let c1 = step(&f, &[], &[("f", "a\nb\nc\nd\ne\n")], "user:alice", "one");
    let c2 = step(&f, &[c1], &[("f", "a\nb\nC\nd\ne\n")], "user:bob", "two");
    let c3 = step(&f, &[c2], &[("f", "a\nb\nC\nd\nE\n")], "user:carol", "three");

    let r = f.repo.blame(&c3, "f", &BlameOptions { range: Some((3, 4)), ..Default::default() }).unwrap();
    assert_eq!(r.lines.iter().map(|l| l.line).collect::<Vec<_>>(), [3, 4]);
    assert_eq!(r.lines[0].changeset, c2);
    assert!(f.repo.blame(&c3, "f", &BlameOptions { range: Some((0, 2)), ..Default::default() }).is_err());
    assert!(f.repo.blame(&c3, "f", &BlameOptions { range: Some((9, 12)), ..Default::default() }).is_err());

    // A walk limit makes the answer partial — and says so.
    let cut = f.repo.blame(&c3, "f", &BlameOptions { max_commits: 1, ..Default::default() }).unwrap();
    assert!(cut.truncated);
    assert!(cut.lines.iter().any(|l| l.boundary));
    assert_eq!(cut.lines[4].changeset, c3, "what was found stays correct");
}

#[test]
fn files_blame_cannot_handle_are_refused_clearly() {
    let f = fx();
    let c = f.snap(None, &[("text", "x\n")], "user:alice");
    assert!(f.repo.blame(&c, "missing", &BlameOptions::default()).unwrap_err().to_string().contains("does not exist"));
    // binary
    let bin = f.repo.put_blob(vec![0, 1, 2, 3]).unwrap();
    let base = f.repo.tree_or_empty(Some(&c)).unwrap();
    let mut entries: Vec<_> = base.iter().map(|(p, e)| (p.clone(), *e)).collect();
    entries.push((
        RepoPath::new("bin").unwrap(),
        ag_tree::Entry { content: bin, file_type: FileType::Regular, executable: false },
    ));
    let want = Tree::from_entries(entries, f.repo.limits()).unwrap();
    let c2 = f.repo.commit(vec![c], &id("user:alice"), "bin", base.diff(&want)).unwrap();
    assert!(f.repo.blame(&c2, "bin", &BlameOptions::default()).unwrap_err().to_string().contains("binary"));
    // an empty file has no lines and no blame
    let c3 = step(&f, &[c2], &[("text", "x\n"), ("empty", "")], "user:alice", "empty");
    assert!(blame(&f, &c3, "empty").lines.is_empty());
}

#[test]
fn a_long_history_stays_correct() {
    // 60 changesets, each appending a line and sometimes rewriting an old one.
    let f = fx();
    let mut lines: Vec<String> = vec!["header".into()];
    let mut head = step(&f, &[], &[("log.txt", "header\n")], "user:u0", "c0");
    let mut author_of: Vec<String> = vec!["u0".into()];
    for i in 1..60 {
        lines.push(format!("entry {i}"));
        author_of.push(format!("u{i}"));
        if i % 7 == 0 {
            let k = i / 2;
            lines[k] = format!("entry {k} rewritten at {i}");
            author_of[k] = format!("u{i}");
        }
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        head = step(
            &f,
            &[head],
            &[("log.txt", &text), ("noise", &format!("{i}\n"))],
            &format!("user:u{i}"),
            &format!("c{i}"),
        );
    }
    let b = blame(&f, &head, "log.txt");
    for (i, l) in b.lines.iter().enumerate() {
        assert_eq!(l.author.trim_start_matches("user:"), author_of[i], "line {}", i + 1);
    }
    assert!(!b.truncated);
}

/// A history of unpublished changesets several keyframe intervals long (no tree of it is cached), in which the
/// file is deleted and created again. Blame looks the file up in each visited changeset without building its
/// tree; the lookup must give what the tree would: a deletion ends the file's history, a re-creation starts it.
#[test]
fn blame_over_a_long_uncached_chain_with_a_delete_and_a_recreate() {
    let f = fx();
    let mut text = String::from("one\ntwo\nthree\n");
    let mut head = step(&f, &[], &[("a.txt", &text), ("other.txt", "x\n")], "user:alice", "start");
    let mut recreated = None;
    let mut last_appended = head;
    for i in 0..80 {
        if i == 40 {
            // Gone in this changeset ...
            head = step(&f, &[head], &[("other.txt", "x\n")], "user:bob", "remove a.txt");
        } else if i == 41 {
            // ... and created anew, with different lines, by someone else.
            text = String::from("n1\nn2\nn3\n");
            head = step(&f, &[head], &[("a.txt", &text), ("other.txt", "x\n")], "user:carol", "recreate a.txt");
            recreated = Some(head);
        } else {
            text.push_str(&format!("line {i}\n"));
            head = step(&f, &[head], &[("a.txt", &text), ("other.txt", "x\n")], "user:bob", &format!("append {i}"));
            last_appended = head;
        }
    }
    let b = blame(&f, &head, "a.txt");
    let recreated = recreated.unwrap();
    assert_eq!(b.lines.len(), 3 + 38);
    for l in &b.lines[..3] {
        assert_eq!(l.changeset, recreated, "{l:?}: the old history ended with the deletion");
        assert!(!l.boundary);
    }
    assert_eq!(b.lines.last().unwrap().changeset, last_appended);
    assert_eq!(b.lines.last().unwrap().text, "line 79");
    assert!(!b.truncated);
    // Every appended line is blamed on exactly the changeset that appended it.
    let log = f.repo.log(&head, 200, Some("a.txt")).unwrap();
    assert!(log.iter().any(|e| e.id == recreated));
}
