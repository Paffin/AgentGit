use super::*;
use ag_changeset::MemoryHistory;
use ag_object::hash_blob;
use ag_types::{Identity, Namespace, Timestamp};
use proptest::prelude::*;

fn p(s: &str) -> RepoPath {
    RepoPath::new(s).unwrap()
}

fn put(path: &str, c: &str) -> Op {
    Op::Put { path: p(path), content: hash_blob(c.as_bytes()), file_type: FileType::Regular, executable: false }
}

fn del(path: &str) -> Op {
    Op::Delete { path: p(path) }
}

fn lim() -> Limits {
    Limits::default()
}

fn tree(ops: Vec<Op>) -> Tree {
    Tree::empty().apply(&ops, &lim()).unwrap()
}

#[test]
fn apply_put_and_delete() {
    let t = tree(vec![put("a.txt", "1"), put("d/b.txt", "2")]);
    assert_eq!(t.len(), 2);
    let t2 = t.apply(&[del("a.txt"), put("d/b.txt", "3")], &lim()).unwrap();
    assert_eq!(t2.len(), 1);
    assert_eq!(t2.get(&p("d/b.txt")).unwrap().content, hash_blob(b"3"));
}

#[test]
fn rejects_delete_missing_and_noop_put() {
    let t = tree(vec![put("a", "1")]);
    assert_eq!(t.apply(&[del("zzz")], &lim()), Err(TreeError::DeleteMissing(p("zzz"))));
    assert_eq!(t.apply(&[put("a", "1")], &lim()), Err(TreeError::NoOpPut(p("a"))));
}

#[test]
fn file_directory_conflicts() {
    let t = tree(vec![put("a", "1")]);
    assert!(matches!(
        t.apply(&[put("a/b", "2")], &lim()),
        Err(TreeError::FileDirectoryConflict { .. })
    ));
    let t = tree(vec![put("a/b", "1")]);
    assert!(matches!(
        t.apply(&[put("a", "2")], &lim()),
        Err(TreeError::FileDirectoryConflict { .. })
    ));
    // replacing a directory by a file in one changeset is fine
    let ok = t.apply(&[del("a/b"), put("a", "2")], &lim()).unwrap();
    assert_eq!(ok.len(), 1);
    // sibling with a common string prefix is not a conflict
    let t = tree(vec![put("a-b", "1"), put("a.txt", "2")]);
    assert!(t.apply(&[put("a/c", "3")], &lim()).is_ok());
}

#[test]
fn diff_is_minimal_and_roundtrips() {
    let a = tree(vec![put("keep", "k"), put("gone", "g"), put("chg", "1")]);
    let b = tree(vec![put("keep", "k"), put("chg", "2"), put("new/x", "n")]);
    let ops = a.diff(&b);
    assert_eq!(ops.len(), 3);
    assert_eq!(a.apply(&ops, &lim()).unwrap(), b);
    assert!(a.diff(&a).is_empty());
}

#[test]
fn diff_handles_dir_to_file_replacement() {
    let a = tree(vec![put("a/b", "1")]);
    let b = tree(vec![put("a", "2")]);
    let ops = a.diff(&b);
    assert_eq!(a.apply(&ops, &lim()).unwrap(), b);
}

#[test]
fn tree_id_is_state_function() {
    // Same state reached by different paths has the same id.
    let t1 = tree(vec![put("a", "1"), put("b", "2")]);
    let t2 = tree(vec![put("b", "2")]).apply(&[put("a", "1")], &lim()).unwrap();
    assert_eq!(t1.id(), t2.id());
    let t3 = tree(vec![put("a", "1")]);
    assert_ne!(t1.id(), t3.id());
    assert_eq!(Tree::empty().id(), Tree::empty().id());
}

#[test]
fn encode_decode_roundtrip_and_strictness() {
    let t = tree(vec![put("a", "1"), put("d/e", "2"), put("d/f", "3")]);
    let bytes = t.encode();
    assert_eq!(Tree::decode(&bytes, &lim()).unwrap(), t);

    // conflicting manifest (file 'a' and 'a/b') is rejected on decode
    let mut w = Writer::new();
    w.varint(2);
    for path in ["a", "a/b"] {
        w.str(path).u8(0).u8(0).fixed32(hash_blob(b"x").as_bytes());
    }
    assert!(matches!(
        Tree::decode(&w.finish(), &lim()),
        Err(TreeError::FileDirectoryConflict { .. })
    ));

    // out-of-order manifest is rejected
    let mut w = Writer::new();
    w.varint(2);
    for path in ["b", "a"] {
        w.str(path).u8(0).u8(0).fixed32(hash_blob(b"x").as_bytes());
    }
    assert!(matches!(Tree::decode(&w.finish(), &lim()), Err(TreeError::Malformed(_))));
}

#[test]
fn list_dir_derives_directories() {
    let t = tree(vec![put("README", "r"), put("src/a.rs", "a"), put("src/b/c.rs", "c"), put("src/b/d.rs", "d")]);
    let root: Vec<_> = t.list_dir("").iter().map(|e| e.name().to_string()).collect();
    assert_eq!(root, ["README", "src"]);
    let src = t.list_dir("src");
    assert_eq!(src.len(), 2);
    assert!(matches!(&src[1], DirEntry::Dir { name } if name == "b"));
    assert_eq!(t.list_dir("src/b").len(), 2);
    assert!(t.list_dir("nope").is_empty());
}

fn commit(h: &MemoryHistory, parents: Vec<ChangesetId>, ops: Vec<Op>) -> ChangesetId {
    h.insert(
        Changeset::build(
            Namespace::new("t/t").unwrap(),
            parents,
            Identity::new("user:t").unwrap(),
            Timestamp::from_millis(0),
            "m",
            ops,
            &lim(),
        )
        .unwrap(),
    )
}

#[test]
fn resolve_tree_replays_first_parent_chain() {
    let h = MemoryHistory::new();
    let c1 = commit(&h, vec![], vec![put("a", "1"), put("b", "1")]);
    let c2 = commit(&h, vec![c1], vec![put("a", "2")]);
    let side = commit(&h, vec![c1], vec![put("s", "s")]);
    // merge: first parent c2, result = c2 state + s
    let m = commit(&h, vec![c2, side], vec![put("s", "s")]);
    let t = resolve_tree(&h, &NoCache, &m, &lim()).unwrap();
    assert_eq!(t.len(), 3);
    assert_eq!(t.get(&p("a")).unwrap().content, hash_blob(b"2"));
    assert_eq!(resolve_tree(&h, &NoCache, &c1, &lim()).unwrap().len(), 2);
}

struct OneHit(ChangesetId, Tree);
impl TreeCache for OneHit {
    fn cached_tree(&self, id: &ChangesetId) -> Option<Tree> {
        (id == &self.0).then(|| self.1.clone())
    }
}

#[test]
fn resolve_tree_uses_cache() {
    let h = MemoryHistory::new();
    let c1 = commit(&h, vec![], vec![put("a", "1")]);
    let c2 = commit(&h, vec![c1], vec![put("b", "1")]);
    // A (deliberately wrong) cached tree for c1 proves the cache short-circuits.
    let fake = tree(vec![put("zzz", "z")]);
    let t = resolve_tree(&h, &OneHit(c1, fake), &c2, &lim()).unwrap();
    assert!(t.get(&p("zzz")).is_some() && t.get(&p("b")).is_some() && t.get(&p("a")).is_none());
}

#[test]
fn resolve_tree_reports_invalid_history() {
    let h = MemoryHistory::new();
    // changeset deleting something that never existed
    let bad = commit(&h, vec![], vec![del("ghost")]);
    assert!(matches!(
        resolve_tree(&h, &NoCache, &bad, &lim()),
        Err(TreeError::DeleteMissing(_))
    ));
    let missing = ChangesetId::new(ObjectId::from_bytes([9; 32]));
    assert!(matches!(
        resolve_tree(&h, &NoCache, &missing, &lim()),
        Err(TreeError::Source(SourceError::NotFound(_)))
    ));
}

#[test]
fn resolve_tree_enforces_depth_limit() {
    let h = MemoryHistory::new();
    let mut prev = commit(&h, vec![], vec![put("f", "0")]);
    for i in 1..5 {
        prev = commit(&h, vec![prev], vec![put("f", &i.to_string())]);
    }
    let tight = Limits { max_dag_depth: 3, ..Limits::default() };
    assert_eq!(resolve_tree(&h, &NoCache, &prev, &tight), Err(TreeError::TooDeep(3)));
}

fn arb_tree() -> impl Strategy<Value = Tree> {
    proptest::collection::btree_map("[a-c]{1,2}(/[a-c]{1,2}){0,2}", 0u8..4, 0..10).prop_map(|m| {
        let keys: Vec<String> = m.keys().cloned().collect();
        let entries = m
            .into_iter()
            .filter(|(k, _)| !keys.iter().any(|o| o != k && o.starts_with(&format!("{k}/"))))
            .filter(|(k, _)| !keys.iter().any(|o| k.starts_with(&format!("{o}/"))))
            .map(|(k, v)| {
                (
                    RepoPath::new(k).unwrap(),
                    Entry { content: hash_blob(&[v]), file_type: FileType::Regular, executable: v == 3 },
                )
            });
        Tree::from_entries(entries, &Limits::default()).unwrap()
    })
}

proptest! {
    #[test]
    fn diff_apply_roundtrip(a in arb_tree(), b in arb_tree()) {
        let ops = a.diff(&b);
        let got = a.apply(&ops, &Limits::default()).unwrap();
        prop_assert_eq!(&got, &b);
        prop_assert_eq!(got.id(), b.id());
    }

    #[test]
    fn encode_decode_roundtrip(t in arb_tree()) {
        let bytes = t.encode();
        let back = Tree::decode(&bytes, &Limits::default()).unwrap();
        prop_assert_eq!(&back, &t);
        prop_assert_eq!(back.encode(), bytes);
    }

    #[test]
    fn decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..128)) {
        let _ = Tree::decode(&bytes, &Limits::default());
    }
}
