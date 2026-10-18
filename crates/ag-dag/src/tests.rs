use super::*;
use ag_changeset::{Changeset, MemoryHistory};
use ag_types::{Identity, Limits, Namespace, ObjectId, Timestamp};
use proptest::prelude::*;

/// Simple adjacency-list graph; node ids are derived from the index.
#[derive(Debug)]
struct TestGraph {
    parents: Vec<Vec<usize>>,
}

fn nid(i: usize) -> ChangesetId {
    let mut b = [0u8; 32];
    b[..8].copy_from_slice(&(i as u64).to_be_bytes());
    ChangesetId::new(ObjectId::from_bytes(b))
}

fn idx(id: &ChangesetId) -> usize {
    u64::from_be_bytes(id.as_bytes()[..8].try_into().unwrap()) as usize
}

impl Graph for TestGraph {
    fn parents(&self, id: &ChangesetId) -> Result<Vec<ChangesetId>, DagError> {
        Ok(self.parents[idx(id)].iter().map(|p| nid(*p)).collect())
    }

    fn generation(&self, id: &ChangesetId) -> Result<u64, DagError> {
        fn go(g: &TestGraph, i: usize, memo: &mut HashMap<usize, u64>) -> u64 {
            if let Some(v) = memo.get(&i) {
                return *v;
            }
            let v = 1 + g.parents[i].iter().map(|p| go(g, *p, memo)).max().unwrap_or(0);
            memo.insert(i, v);
            v
        }
        Ok(go(self, idx(id), &mut HashMap::new()))
    }
}

fn naive_ancestors(g: &TestGraph, i: usize) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    let mut stack = vec![i];
    while let Some(c) = stack.pop() {
        if out.insert(c) {
            stack.extend(&g.parents[c]);
        }
    }
    out
}

fn naive_merge_bases(g: &TestGraph, a: usize, b: usize) -> BTreeSet<usize> {
    let common: BTreeSet<usize> = naive_ancestors(g, a).intersection(&naive_ancestors(g, b)).copied().collect();
    common.iter().filter(|c| !common.iter().any(|o| o != *c && naive_ancestors(g, *o).contains(c))).copied().collect()
}

#[test]
fn linear_and_fork() {
    //   0 - 1 - 2
    //        \
    //         3
    let g = TestGraph { parents: vec![vec![], vec![0], vec![1], vec![1]] };
    assert_eq!(merge_bases(&g, &nid(2), &nid(3)).unwrap(), vec![nid(1)]);
    assert_eq!(merge_bases(&g, &nid(2), &nid(1)).unwrap(), vec![nid(1)]);
    assert_eq!(merge_bases(&g, &nid(2), &nid(2)).unwrap(), vec![nid(2)]);
    assert!(is_ancestor(&g, &nid(0), &nid(3)).unwrap());
    assert!(!is_ancestor(&g, &nid(2), &nid(3)).unwrap());
    assert!(is_ancestor(&g, &nid(3), &nid(3)).unwrap());
}

#[test]
fn criss_cross_has_two_bases() {
    //      1       3
    //    /   \   /   \
    //  0       X       5(=a)  ... classic criss-cross
    // a = merge(1,2), b = merge(1,2) differently
    let g = TestGraph {
        parents: vec![
            vec![],     // 0 root
            vec![0],    // 1
            vec![0],    // 2
            vec![1, 2], // 3 = a
            vec![2, 1], // 4 = b
        ],
    };
    let bases = merge_bases(&g, &nid(3), &nid(4)).unwrap();
    assert_eq!(bases.len(), 2);
    assert!(bases.contains(&nid(1)) && bases.contains(&nid(2)));
}

#[test]
fn unrelated_histories_have_no_base() {
    let g = TestGraph { parents: vec![vec![], vec![], vec![0], vec![1]] };
    assert!(merge_bases(&g, &nid(2), &nid(3)).unwrap().is_empty());
}

#[test]
fn ancestors_are_ordered_and_limited() {
    let g = TestGraph { parents: vec![vec![], vec![0], vec![1], vec![1], vec![2, 3]] };
    let a = ancestors(&g, &[nid(4)], 100).unwrap();
    assert_eq!(a.len(), 5);
    assert_eq!(a[0], nid(4));
    assert_eq!(*a.last().unwrap(), nid(0));
    assert_eq!(ancestors(&g, &[nid(4)], 3), Err(DagError::LimitExceeded(3)));
    assert_eq!(first_parent_chain(&g, &nid(4), 100).unwrap(), vec![nid(4), nid(2), nid(1), nid(0)]);
    assert_eq!(reachable(&g, [nid(3)]).unwrap().len(), 3);
}

#[test]
fn source_graph_computes_generations_over_deep_history() {
    let h = MemoryHistory::new();
    let ns = Namespace::new("t/t").unwrap();
    let mut prev: Vec<ChangesetId> = vec![];
    let mut last = None;
    // 3000 deep: would overflow the stack with naive recursion.
    for i in 0..3000i64 {
        let cs = Changeset::build(
            ns.clone(),
            prev.clone(),
            Identity::new("user:t").unwrap(),
            Timestamp::from_millis(i),
            "m",
            vec![],
            &Limits::default(),
        )
        .unwrap();
        let id = h.insert(cs);
        prev = vec![id];
        last = Some(id);
    }
    let g = SourceGraph::new(&h);
    assert_eq!(g.generation(&last.unwrap()).unwrap(), 3000);
}

fn arb_graph() -> impl Strategy<Value = TestGraph> {
    (2usize..14).prop_flat_map(|n| {
        let per_node: Vec<_> = (0..n)
            .map(|i| {
                if i == 0 {
                    Just(Vec::<usize>::new()).boxed()
                } else {
                    proptest::collection::btree_set(0..i, 0..3usize)
                        .prop_map(|s| s.into_iter().collect::<Vec<_>>())
                        .boxed()
                }
            })
            .collect();
        per_node.prop_map(|parents| TestGraph { parents })
    })
}

proptest! {
    #[test]
    fn merge_bases_match_naive(g in arb_graph(), a in 0usize..14, b in 0usize..14) {
        let n = g.parents.len();
        let (a, b) = (a % n, b % n);
        let got: BTreeSet<usize> = merge_bases(&g, &nid(a), &nid(b)).unwrap().iter().map(idx).collect();
        prop_assert_eq!(got, naive_merge_bases(&g, a, b));
    }

    #[test]
    fn is_ancestor_matches_naive(g in arb_graph(), a in 0usize..14, b in 0usize..14) {
        let n = g.parents.len();
        let (a, b) = (a % n, b % n);
        prop_assert_eq!(is_ancestor(&g, &nid(a), &nid(b)).unwrap(), naive_ancestors(&g, b).contains(&a));
    }

    #[test]
    fn ancestors_match_naive_and_are_generation_ordered(g in arb_graph(), a in 0usize..14) {
        let a = a % g.parents.len();
        let got = ancestors(&g, &[nid(a)], 1000).unwrap();
        let set: BTreeSet<usize> = got.iter().map(idx).collect();
        prop_assert_eq!(set, naive_ancestors(&g, a));
        let gens: Vec<u64> = got.iter().map(|i| g.generation(i).unwrap()).collect();
        prop_assert!(gens.windows(2).all(|w| w[0] >= w[1]));
    }
}

/// A graph that counts how many parent lookups the algorithms make.
struct Counting<'a> {
    inner: &'a TestGraph,
    parent_calls: std::cell::Cell<usize>,
}

impl Graph for Counting<'_> {
    fn parents(&self, id: &ChangesetId) -> Result<Vec<ChangesetId>, DagError> {
        self.parent_calls.set(self.parent_calls.get() + 1);
        self.inner.parents(id)
    }

    fn generation(&self, id: &ChangesetId) -> Result<u64, DagError> {
        self.inner.generation(id)
    }
}

#[test]
fn divergence_of_two_lines_that_forked_from_a_long_history_costs_only_the_fork() {
    // 0 <- 1 <- ... <- 4999 (shared), then a: 5000..5003 and b: 5004..5005 on top of 4999
    let mut parents: Vec<Vec<usize>> = vec![vec![]];
    for i in 1..5000 {
        parents.push(vec![i - 1]);
    }
    parents.push(vec![4999]); // 5000
    parents.extend((5001..5004).map(|i| vec![i - 1]));
    parents.push(vec![4999]); // 5004
    parents.push(vec![5004]); // 5005
    let g = TestGraph { parents };
    let counting = Counting { inner: &g, parent_calls: std::cell::Cell::new(0) };
    let (only_a, only_b) = divergence(&counting, &nid(5003), &nid(5005), 100).unwrap();
    assert_eq!(only_a, vec![nid(5003), nid(5002), nid(5001), nid(5000)]);
    assert_eq!(only_b, vec![nid(5005), nid(5004)]);
    assert!(counting.parent_calls.get() < 20, "{} lookups for a divergence of 6", counting.parent_calls.get());
    // the limit applies to the divergence, not to the history underneath it
    assert_eq!(divergence(&counting, &nid(5003), &nid(5005), 3), Err(DagError::LimitExceeded(3)));
    // the full ancestor walk would have failed on the same bound
    assert_eq!(ancestors(&g, &[nid(5003)], 100), Err(DagError::LimitExceeded(100)));
}

#[test]
fn divergence_of_related_and_identical_changesets() {
    //   0 - 1 - 2 - 3
    //        \
    //         4
    let g = TestGraph { parents: vec![vec![], vec![0], vec![1], vec![2], vec![1]] };
    assert_eq!(divergence(&g, &nid(3), &nid(3), 10).unwrap(), (vec![], vec![]));
    // an ancestor has nothing of its own
    assert_eq!(divergence(&g, &nid(3), &nid(1), 10).unwrap(), (vec![nid(3), nid(2)], vec![]));
    assert_eq!(divergence(&g, &nid(1), &nid(3), 10).unwrap(), (vec![], vec![nid(3), nid(2)]));
    assert_eq!(divergence(&g, &nid(3), &nid(4), 10).unwrap(), (vec![nid(3), nid(2)], vec![nid(4)]));
    // unrelated histories: everything is divergence
    let u = TestGraph { parents: vec![vec![], vec![0], vec![], vec![2]] };
    assert_eq!(divergence(&u, &nid(1), &nid(3), 10).unwrap(), (vec![nid(1), nid(0)], vec![nid(3), nid(2)]));
}

proptest! {
    #[test]
    fn divergence_matches_the_difference_of_the_ancestor_sets(g in arb_graph(), a in 0usize..14, b in 0usize..14) {
        let n = g.parents.len();
        let (a, b) = (a % n, b % n);
        let (only_a, only_b) = divergence(&g, &nid(a), &nid(b), 1000).unwrap();
        let (anc_a, anc_b) = (naive_ancestors(&g, a), naive_ancestors(&g, b));
        let want_a: BTreeSet<usize> = anc_a.difference(&anc_b).copied().collect();
        let want_b: BTreeSet<usize> = anc_b.difference(&anc_a).copied().collect();
        prop_assert_eq!(only_a.iter().map(idx).collect::<BTreeSet<_>>(), want_a);
        prop_assert_eq!(only_b.iter().map(idx).collect::<BTreeSet<_>>(), want_b);
        // no duplicates, newest generation first
        prop_assert_eq!(only_a.len(), only_a.iter().collect::<BTreeSet<_>>().len());
        let gens: Vec<u64> = only_a.iter().map(|i| g.generation(i).unwrap()).collect();
        prop_assert!(gens.windows(2).all(|w| w[0] >= w[1]));
    }
}
