//! Cost of the flat tree by size. Ignored by default (a measurement, not a check):
//!
//!   cargo test --release -p agentgit-tree --test scale -- --ignored --nocapture
#![allow(missing_docs)]

use std::time::Instant;

use ag_changeset::Op;
use ag_object::hash_blob;
use ag_tree::{Entry, Tree};
use ag_types::{FileType, Limits, RepoPath};

fn path(i: usize) -> String {
    format!("pkg{:03}/mod{:02}/file{i}.rs", i % 200, i % 40)
}

fn timed<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let v = f();
    println!("    {label:<42} {:>9.2} ms", t.elapsed().as_secs_f64() * 1000.0);
    v
}

#[test]
#[ignore = "measurement"]
fn flat_tree_costs() {
    let limits = Limits { max_tree_entries: 2_000_000, ..Limits::default() };
    for n in [10_000usize, 100_000, 500_000] {
        println!("\n{n} entries");
        let entries: Vec<(RepoPath, Entry)> = (0..n)
            .map(|i| {
                let content = hash_blob(format!("file {i}").as_bytes());
                (RepoPath::new(path(i)).unwrap(), Entry { content, file_type: FileType::Regular, executable: false })
            })
            .collect();
        let tree = timed("Tree::from_entries (validate)", || Tree::from_entries(entries.clone(), &limits).unwrap());
        let bytes = timed("encode", || tree.encode());
        let decoded = timed("decode (strict, as for a cached manifest)", || Tree::decode(&bytes, &limits).unwrap());
        assert_eq!(decoded.len(), n);
        timed("id (encode + SHA-256)", || tree.id());
        let cloned = timed("clone", || tree.clone());
        let p = RepoPath::new(path(7)).unwrap();
        let op = Op::Put {
            path: p.clone(),
            content: hash_blob(b"changed"),
            file_type: FileType::Regular,
            executable: false,
        };
        let next = timed("apply one Put", || tree.apply(std::slice::from_ref(&op), &limits).unwrap());
        timed("diff (n vs n, 1 change)", || tree.diff(&next).len());
        timed("get x1000", || (0..1000).filter(|i| tree.get(&RepoPath::new(path(*i)).unwrap()).is_some()).count());
        drop(cloned);
    }
}
