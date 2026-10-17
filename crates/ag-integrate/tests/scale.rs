//! How the engine behaves as a repository grows. Ignored by default (it is a measurement, not a check):
//!
//!   cargo test --release -p ag-integrate --test scale -- --ignored --nocapture
//!
//! It reports wall-clock time for the operations whose cost depends on the number of files, so the decision
//! about hierarchical trees (ADR-0006) rests on numbers. Memory and SQLite/object-store I/O are in-process
//! here (the `fx()` fixture), so these are CPU costs of the engine, not of a deployment.
#![allow(missing_docs)]

mod common;

use std::time::Instant;

use ag_integrate::{Outcome, Strategy};
use ag_tree::{Entry, Tree};
use ag_types::{FileType, RepoPath};
use common::*;

fn path(i: usize) -> String {
    format!("pkg{:03}/mod{:02}/file{i}.rs", i % 200, i % 40)
}

fn timed<T>(label: &str, f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let v = f();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("    {label:<46} {ms:>9.1} ms");
    (v, ms)
}

#[test]
#[ignore = "measurement"]
fn cost_by_repository_size() {
    for n in [1_000usize, 10_000, 100_000] {
        println!("\n{n} files");
        let f = fx();
        let (tree, _) = timed("build the in-memory tree (put every blob)", || {
            Tree::from_entries(
                (0..n).map(|i| {
                    let content = f.repo.put_blob(format!("// file {i}\nfn f{i}() {{}}\n").into_bytes()).unwrap();
                    (
                        RepoPath::new(path(i)).unwrap(),
                        Entry { content, file_type: FileType::Regular, executable: false },
                    )
                }),
                f.repo.limits(),
            )
            .unwrap()
        });
        timed("Tree::encode (the manifest)", || tree.encode().len());
        timed("Tree::id (encode + hash)", || tree.id());
        let ops = Tree::empty().diff(&tree);
        let (root, _) = timed("commit one changeset with every file", || {
            f.repo.commit(vec![], &id("user:alice"), "import", ops).unwrap()
        });
        let prop = f.propose(root, "import", "user:alice");
        let (r, _) = timed("integrate it (publish, verify, derive, CAS)", || f.integrate(&prop, "user:alice"));
        assert!(matches!(r.outcome, Outcome::Integrated { .. }));
        let head = f.head().unwrap();

        // Two proposals from the same base, each touching one file.
        let edit = |k: usize, tag: &str| {
            let base = f.repo.tree_of(&head).unwrap();
            let p = RepoPath::new(path(k)).unwrap();
            let content = f.repo.put_blob(format!("// edited by {tag}\n").into_bytes()).unwrap();
            let mut want = base.clone();
            let ops = vec![ag_changeset::Op::Put { path: p, content, file_type: FileType::Regular, executable: false }];
            want = want.apply(&ops, f.repo.limits()).unwrap();
            f.repo.commit(vec![head], &id("user:bob"), tag, base.diff(&want)).unwrap()
        };
        let (a, _) = timed("tree_of(head) + one-file edit + commit (A)", || edit(7, "a"));
        let b = edit(n / 2, "b");
        let pa = f.propose(a, "a", "user:bob");
        let pb = f.propose(b, "b", "user:bob");
        let (ra, _) = timed("integrate A (fast-forward)", || f.integrate(&pa, "user:alice"));
        assert!(matches!(ra.outcome, Outcome::Integrated { strategy: Strategy::FastForward, .. }));
        let (rb, _) = timed("integrate B (three-way merge, 1 file vs 1 file)", || f.integrate(&pb, "user:alice"));
        assert!(matches!(rb.outcome, Outcome::Integrated { strategy: Strategy::Merge, .. }), "{:?}", rb.outcome);
        timed("tree_of(new head) (cache hit)", || f.repo.tree_of(&f.head().unwrap()).unwrap().len());
        timed("path history of one file (ag log --path), 3 changesets", || {
            f.repo.log(&f.head().unwrap(), 10, Some(&path(7))).unwrap().len()
        });
        timed("blame of one file", || {
            f.repo.blame(&f.head().unwrap(), &path(7), &Default::default()).unwrap().lines.len()
        });
    }
}
