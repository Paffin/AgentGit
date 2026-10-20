//! An in-process cache of decoded trees under a memory budget.
//!
//! Deriving the tree of a changeset costs a SQLite/PostgreSQL read, a strict decode and a verification of the
//! manifest (a re-encode and a SHA-256) - about 105 ms of a 345 ms fast-forward at 100 000 files
//! (`docs/architecture/performance.md`). A tree is a pure function of the changeset's history, so the answer
//! for a changeset id never changes and the cache cannot be stale; it can only be absent. Everything here is
//! therefore an optimization that the caller may skip: `get` is a hit or a miss, and a miss falls back to the
//! slow path.
//!
//! * **One budget per process**, shared by every repository the process hosts: [`DecodedTrees::new`] takes the
//!   number of bytes, [`DecodedTrees::scope`] gives each repository a [`TreeScope`] (its entries are invisible to
//!   other scopes, so a repository can never be answered with a tree that belongs to another one).
//! * **Least recently used out first.** An insertion that does not fit evicts the coldest entries until it does;
//!   a tree larger than the whole budget is not cached at all (and is counted as `rejected`).
//! * **The size is an estimate** ([`approx_tree_bytes`]), not an allocator measurement; the real footprint is
//!   within a small factor of it.
//! * Hits return a clone of the tree (`Tree` owns its map); cloning is about 7 ms at 100 000 files against about
//!   105 ms to decode and verify.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ag_tree::Tree;
use ag_types::ChangesetId;

/// Counters of a cache, for `/metrics` and for tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreeCacheStats {
    /// Lookups that found a tree.
    pub hits: u64,
    /// Lookups that did not.
    pub misses: u64,
    /// Entries removed to make room.
    pub evictions: u64,
    /// Trees not cached because one alone is larger than the budget.
    pub rejected: u64,
    /// Estimated bytes held now.
    pub bytes: u64,
    /// Entries held now.
    pub entries: u64,
    /// The budget, in bytes.
    pub budget: u64,
}

/// An estimate of the memory a tree holds, in bytes: per entry the path's text and handle, the entry itself and
/// the map's per-entry share (three machine words of node bookkeeping). Close enough to size a cache by; it is not an
/// allocator measurement.
pub fn approx_tree_bytes(tree: &Tree) -> usize {
    let per_entry = std::mem::size_of::<ag_types::RepoPath>()
        + std::mem::size_of::<ag_tree::Entry>()
        + 3 * std::mem::size_of::<usize>();
    std::mem::size_of::<Tree>() + tree.iter().map(|(path, _)| per_entry + path.as_str().len()).sum::<usize>()
}

type Key = (u64, ChangesetId);

struct Slot {
    tree: Arc<Tree>,
    bytes: usize,
    /// Position in the recency order (larger = more recent).
    tick: u64,
}

#[derive(Default)]
struct State {
    slots: HashMap<Key, Slot>,
    /// `tick -> key`, oldest first: the eviction order.
    order: BTreeMap<u64, Key>,
    bytes: usize,
    tick: u64,
}

/// A byte-budgeted LRU of decoded trees, shared by the repositories of one process.
pub struct DecodedTrees {
    budget: usize,
    state: Mutex<State>,
    scopes: Mutex<HashMap<String, u64>>,
    next_scope: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
    rejected: AtomicU64,
}

impl std::fmt::Debug for DecodedTrees {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodedTrees").field("stats", &self.stats()).finish()
    }
}

impl DecodedTrees {
    /// A cache that holds at most `budget_bytes` (estimated) of trees. A budget of 0 holds nothing: every lookup
    /// misses and nothing is stored - the cache is off.
    pub fn new(budget_bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            budget: budget_bytes,
            state: Mutex::new(State::default()),
            scopes: Mutex::new(HashMap::new()),
            next_scope: AtomicU64::new(1),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
        })
    }

    /// The view of one repository (named by `name`; asking twice for a name gives the same scope). Its entries are
    /// visible to that name only.
    pub fn scope(self: &Arc<Self>, name: &str) -> TreeScope {
        let mut scopes = self.scopes.lock().unwrap_or_else(|e| e.into_inner());
        let id = *scopes.entry(name.to_string()).or_insert_with(|| self.next_scope.fetch_add(1, Ordering::Relaxed));
        TreeScope { cache: self.clone(), id }
    }

    /// Counters and the current fill.
    pub fn stats(&self) -> TreeCacheStats {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        TreeCacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
            bytes: st.bytes as u64,
            entries: st.slots.len() as u64,
            budget: self.budget as u64,
        }
    }

    fn get(&self, key: &Key) -> Option<Arc<Tree>> {
        if self.budget == 0 {
            return None;
        }
        let found = {
            let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let st = &mut *guard;
            match st.slots.get_mut(key) {
                Some(slot) => {
                    st.order.remove(&slot.tick);
                    st.tick += 1;
                    slot.tick = st.tick;
                    st.order.insert(st.tick, *key);
                    Some(slot.tree.clone())
                }
                None => None,
            }
        };
        let counter = if found.is_some() { &self.hits } else { &self.misses };
        counter.fetch_add(1, Ordering::Relaxed);
        found
    }

    fn put(&self, key: Key, tree: Arc<Tree>) {
        let bytes = approx_tree_bytes(&tree);
        if self.budget == 0 {
            return;
        }
        if bytes > self.budget {
            self.rejected.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let st = &mut *st;
        if let Some(old) = st.slots.remove(&key) {
            st.order.remove(&old.tick);
            st.bytes -= old.bytes;
        }
        while st.bytes + bytes > self.budget {
            let Some((_, coldest)) = st.order.pop_first() else { break };
            if let Some(gone) = st.slots.remove(&coldest) {
                st.bytes -= gone.bytes;
                self.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }
        st.tick += 1;
        let tick = st.tick;
        st.order.insert(tick, key);
        st.bytes += bytes;
        st.slots.insert(key, Slot { tree, bytes, tick });
    }

    fn clear_scope(&self, scope: u64) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let st = &mut *st;
        let doomed: Vec<Key> = st.slots.keys().filter(|(s, _)| *s == scope).copied().collect();
        for key in doomed {
            if let Some(gone) = st.slots.remove(&key) {
                st.order.remove(&gone.tick);
                st.bytes -= gone.bytes;
            }
        }
    }
}

/// One repository's view of a [`DecodedTrees`].
#[derive(Clone, Debug)]
pub struct TreeScope {
    cache: Arc<DecodedTrees>,
    id: u64,
}

impl TreeScope {
    /// The cached tree of `id`, if it is held.
    pub fn get(&self, id: &ChangesetId) -> Option<Tree> {
        self.cache.get(&(self.id, *id)).map(|t| (*t).clone())
    }

    /// Remembers the tree of `id` (evicting the coldest entries if the budget needs it).
    pub fn put(&self, id: &ChangesetId, tree: &Tree) {
        self.cache.put((self.id, *id), Arc::new(tree.clone()));
    }

    /// Forgets everything this repository has in the cache (after garbage collection removed changesets, so that
    /// a removed changeset is not still answered from memory).
    pub fn clear(&self) {
        self.cache.clear_scope(self.id);
    }

    /// The cache's counters.
    pub fn stats(&self) -> TreeCacheStats {
        self.cache.stats()
    }
}

#[cfg(test)]
mod tests {
    use ag_object::hash_blob;
    use ag_tree::Entry;
    use ag_types::{FileType, Limits, RepoPath};

    use super::*;

    fn tree(n: usize, tag: &str) -> Tree {
        Tree::from_entries(
            (0..n).map(|i| {
                (
                    RepoPath::new(format!("d{}/file{i}.txt", i % 7)).unwrap(),
                    Entry {
                        content: hash_blob(format!("{tag}{i}").as_bytes()),
                        file_type: FileType::Regular,
                        executable: false,
                    },
                )
            }),
            &Limits::default(),
        )
        .unwrap()
    }

    fn cid(n: u8) -> ChangesetId {
        ChangesetId::new(ag_types::ObjectId::from_bytes([n; 32]))
    }

    #[test]
    fn a_hit_returns_the_tree_and_a_miss_nothing() {
        let cache = DecodedTrees::new(10_000_000);
        let scope = cache.scope("acme/web");
        let t = tree(50, "a");
        assert!(scope.get(&cid(1)).is_none());
        scope.put(&cid(1), &t);
        assert_eq!(scope.get(&cid(1)).as_ref(), Some(&t), "the very tree that was stored");
        let s = cache.stats();
        assert_eq!((s.hits, s.misses, s.entries), (1, 1, 1));
        assert_eq!(s.bytes, approx_tree_bytes(&t) as u64);
    }

    #[test]
    fn the_coldest_trees_go_first_and_the_budget_holds() {
        let one = approx_tree_bytes(&tree(40, "x"));
        let cache = DecodedTrees::new(one * 3 + one / 2); // room for three
        let scope = cache.scope("r");
        for n in 1..=3 {
            scope.put(&cid(n), &tree(40, &format!("t{n}")));
        }
        // Touch 1 so that 2 is now the coldest, then add a fourth.
        assert!(scope.get(&cid(1)).is_some());
        scope.put(&cid(4), &tree(40, "t4"));
        assert!(scope.get(&cid(2)).is_none(), "the least recently used went");
        for n in [1, 3, 4] {
            assert!(scope.get(&cid(n)).is_some(), "{n} stays");
        }
        let s = cache.stats();
        assert_eq!(s.evictions, 1);
        assert!(s.bytes <= s.budget, "{s:?}");
        assert_eq!(s.entries, 3);
    }

    #[test]
    fn a_tree_larger_than_the_whole_budget_is_not_cached() {
        let cache = DecodedTrees::new(1000);
        let scope = cache.scope("r");
        scope.put(&cid(1), &tree(500, "big"));
        assert!(scope.get(&cid(1)).is_none());
        let s = cache.stats();
        assert_eq!((s.rejected, s.entries, s.bytes), (1, 0, 0));
    }

    #[test]
    fn a_budget_of_zero_is_off() {
        let cache = DecodedTrees::new(0);
        let scope = cache.scope("r");
        scope.put(&cid(1), &tree(3, "a"));
        assert!(scope.get(&cid(1)).is_none());
        assert_eq!(cache.stats().entries, 0);
    }

    #[test]
    fn repositories_never_see_each_others_trees() {
        let cache = DecodedTrees::new(10_000_000);
        let (a, b, a_again) = (cache.scope("acme/a"), cache.scope("acme/b"), cache.scope("acme/a"));
        a.put(&cid(1), &tree(5, "a"));
        assert!(b.get(&cid(1)).is_none(), "the same changeset id in another repository is a miss");
        assert!(a_again.get(&cid(1)).is_some(), "a name keeps its scope");
        a.clear();
        assert!(a_again.get(&cid(1)).is_none());
        assert_eq!(cache.stats().bytes, 0);
    }

    #[test]
    fn replacing_an_entry_does_not_leak_its_bytes() {
        let cache = DecodedTrees::new(10_000_000);
        let scope = cache.scope("r");
        let t = tree(10, "a");
        for _ in 0..5 {
            scope.put(&cid(1), &t);
        }
        assert_eq!((cache.stats().entries, cache.stats().bytes), (1, approx_tree_bytes(&t) as u64));
    }

    #[test]
    fn concurrent_use_keeps_the_accounting_exact() {
        let one = approx_tree_bytes(&tree(20, "z"));
        let cache = DecodedTrees::new(one * 8);
        std::thread::scope(|s| {
            for t in 0..4u8 {
                let scope = cache.scope(&format!("r{t}"));
                s.spawn(move || {
                    for round in 0..200u32 {
                        let id = cid((round % 16) as u8);
                        if scope.get(&id).is_none() {
                            scope.put(&id, &tree(20, "z"));
                        }
                    }
                });
            }
        });
        let s = cache.stats();
        assert!(s.bytes <= s.budget, "{s:?}");
        assert_eq!(s.bytes, s.entries * one as u64, "bytes agree with the entries held: {s:?}");
        assert!(s.hits + s.misses >= 800);
    }
}
