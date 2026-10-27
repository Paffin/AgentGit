//! Cost of the tree merge by size. Ignored by default (a measurement, not a check):
//!
//!   cargo test --release -p agentgit-merge --test scale -- --ignored --nocapture
#![allow(missing_docs)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

use ag_merge::{Algorithm, BlobIo, IoError, detect_renames, merge_trees, merge_trees_with};
use ag_object::hash_blob;
use ag_tree::{Entry, Tree};
use ag_types::{BlobId, FileType, Limits, RepoPath};

#[derive(Default)]
struct Mem(RefCell<HashMap<BlobId, Vec<u8>>>);

impl BlobIo for Mem {
    fn read_blob(&self, id: &BlobId) -> Result<Vec<u8>, IoError> {
        self.0.borrow().get(id).cloned().ok_or_else(|| "missing".into())
    }
    fn write_blob(&self, data: Vec<u8>) -> Result<BlobId, IoError> {
        let id = hash_blob(&data);
        self.0.borrow_mut().insert(id, data);
        Ok(id)
    }
}

fn path(i: usize) -> String {
    format!("pkg{:03}/mod{:02}/file{i}.rs", i % 200, i % 40)
}

fn timed<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let v = f();
    println!("    {label:<46} {:>9.2} ms", t.elapsed().as_secs_f64() * 1000.0);
    v
}

#[test]
#[ignore = "measurement"]
fn merge_costs() {
    let limits = Limits { max_tree_entries: 2_000_000, ..Limits::default() };
    let io = Mem::default();
    for n in [10_000usize, 100_000, 500_000] {
        println!("\n{n} entries");
        let mk = |edits: &[(usize, &str)]| {
            let edits: HashMap<usize, &str> = edits.iter().copied().collect();
            Tree::from_entries(
                (0..n).map(|i| {
                    let body = edits.get(&i).map(|t| format!("{t}\n")).unwrap_or_else(|| format!("file {i}\nline\n"));
                    let content = io.write_blob(body.into_bytes()).unwrap();
                    (
                        RepoPath::new(path(i)).unwrap(),
                        Entry { content, file_type: FileType::Regular, executable: false },
                    )
                }),
                &limits,
            )
            .unwrap()
        };
        let base = mk(&[]);
        let ours = mk(&[(7, "ours edit")]);
        let theirs = mk(&[(n / 2, "theirs edit")]);
        timed("detect_renames(base, ours): nothing deleted/added", || detect_renames(&base, &ours, &io).unwrap().len());
        let m = timed("merge_trees: one file each side", || merge_trees(&base, &ours, &theirs, &io, &limits).unwrap());
        assert!(m.is_clean());
        // One side renames a file and the other edits it.
        let moved = {
            let mut entries: Vec<(RepoPath, Entry)> = base.iter().map(|(p, e)| (p.clone(), *e)).collect();
            let (p, e) = entries.remove(10);
            entries.push((RepoPath::new(format!("moved/{}", p.as_str())).unwrap(), e));
            Tree::from_entries(entries, &limits).unwrap()
        };
        let edited = {
            let (p, _) = base.iter().nth(10).unwrap();
            let content = io.write_blob(b"edited\n".to_vec()).unwrap();
            let ops = vec![ag_changeset::Op::Put {
                path: p.clone(),
                content,
                file_type: FileType::Regular,
                executable: false,
            }];
            base.apply(&ops, &limits).unwrap()
        };
        let m = timed("merge_trees: rename on one side, edit on other", || {
            merge_trees(&base, &moved, &edited, &io, &limits).unwrap()
        });
        assert!(m.is_clean(), "{:?}", m.conflicts);
    }
}

/// A directory of `n` files is renamed on one side; the other side edits one file, adds 20 and (in the last
/// case) every file of the tree is identical. Compares ag-merge/3 with ag-merge/2.
#[test]
#[ignore = "measurement"]
fn directory_rename_costs() {
    let limits = Limits { max_tree_entries: 2_000_000, ..Limits::default() };
    for n in [10_000usize, 100_000] {
        for identical in [false, true] {
            println!(
                "\n{n} entries in one renamed directory{}",
                if identical { ", all with identical content" } else { "" }
            );
            let io = Mem::default();
            let body = |i: usize| if identical { "same\n".to_string() } else { format!("file {i}\nline\n") };
            let file = |dir: &str, i: usize| format!("{dir}/pkg{:03}/mod{:02}/file{i}.rs", i % 200, i % 40);
            let entry = |text: String| Entry {
                content: io.write_blob(text.into_bytes()).unwrap(),
                file_type: FileType::Regular,
                executable: false,
            };
            let tree = |dir: &str, extra: &[(String, String)]| {
                let mut v: Vec<(RepoPath, Entry)> =
                    (0..n).map(|i| (RepoPath::new(file(dir, i)).unwrap(), entry(body(i)))).collect();
                v.extend(extra.iter().map(|(p, c)| (RepoPath::new(p.as_str()).unwrap(), entry(c.clone()))));
                Tree::from_entries(v, &limits).unwrap()
            };
            let base = tree("lib", &[]);
            let ours = tree("src", &[]);
            let mut extra: Vec<(String, String)> =
                (0..20).map(|i| (format!("lib/new{i}.rs"), format!("new {i}\n"))).collect();
            extra.push((file("lib", 5), "edited\n".to_string()));
            let theirs = {
                let mut v: Vec<(RepoPath, Entry)> = base.iter().map(|(p, e)| (p.clone(), *e)).collect();
                for (path, c) in &extra {
                    v.retain(|(p, _)| p.as_str() != path);
                    v.push((RepoPath::new(path.as_str()).unwrap(), entry(c.clone())));
                }
                Tree::from_entries(v, &limits).unwrap()
            };
            for algo in [Algorithm::V2, Algorithm::V3] {
                let m = timed(&format!("merge_trees_with({})", algo.version()), || {
                    merge_trees_with(algo, &base, &ours, &theirs, &io, &limits).unwrap()
                });
                println!("      conflicts: {}, notes: {}", m.conflicts.len(), m.notes.len());
                if algo == Algorithm::V3 {
                    assert!(m.is_clean(), "{:?}", m.conflicts.iter().take(3).collect::<Vec<_>>());
                    assert_eq!(m.tree.len(), n + 20);
                }
            }
            let _ = merge_trees;
        }
    }
}
