//! Algorithms over the changeset DAG.
//!
//! Everything here is written against the small [`Graph`] trait, so the same
//! code runs over the metadata index (which stores generation numbers), a
//! plain [`ChangesetSource`] (generation numbers are computed and memoized)
//! or a test fixture. All results are deterministic: ties are broken by
//! identifier.
//!
//! The *generation number* of a changeset is `1 + max(generation(parent))`
//! with `generation(root) = 1`. It strictly increases from parent to child,
//! which lets traversals stop early without visiting the whole history.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap, HashSet};
use std::sync::RwLock;

use ag_changeset::{ChangesetSource, SourceError};
use ag_types::ChangesetId;

/// Errors from DAG traversal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DagError {
    /// A changeset could not be loaded.
    #[error(transparent)]
    Source(#[from] SourceError),
    /// A traversal visited more changesets than allowed.
    #[error("traversal exceeded the limit of {0} changesets")]
    LimitExceeded(usize),
}

/// Read-only view of the DAG.
pub trait Graph {
    /// Ordered parents of `id`.
    fn parents(&self, id: &ChangesetId) -> Result<Vec<ChangesetId>, DagError>;
    /// Generation number of `id` (>= 1).
    fn generation(&self, id: &ChangesetId) -> Result<u64, DagError>;
}

/// Adapts any [`ChangesetSource`] into a [`Graph`], computing generation
/// numbers on demand (iteratively, so deep histories cannot overflow the stack).
pub struct SourceGraph<S> {
    source: S,
    generations: RwLock<HashMap<ChangesetId, u64>>,
}

impl<S: ChangesetSource> SourceGraph<S> {
    /// Wraps a source.
    pub fn new(source: S) -> Self {
        Self { source, generations: RwLock::new(HashMap::new()) }
    }
}

impl<S: ChangesetSource> Graph for SourceGraph<S> {
    fn parents(&self, id: &ChangesetId) -> Result<Vec<ChangesetId>, DagError> {
        Ok(self.source.changeset(id)?.parents.clone())
    }

    fn generation(&self, id: &ChangesetId) -> Result<u64, DagError> {
        if let Some(g) = self.generations.read().expect("lock").get(id) {
            return Ok(*g);
        }
        // Iterative post-order: resolve all unknown ancestors first.
        let mut stack = vec![*id];
        while let Some(cur) = stack.last().copied() {
            if self.generations.read().expect("lock").contains_key(&cur) {
                stack.pop();
                continue;
            }
            let parents = self.source.changeset(&cur)?.parents.clone();
            let known = self.generations.read().expect("lock");
            let unknown: Vec<_> = parents.iter().filter(|p| !known.contains_key(p)).copied().collect();
            if unknown.is_empty() {
                let g = 1 + parents.iter().map(|p| known[p]).max().unwrap_or(0);
                drop(known);
                self.generations.write().expect("lock").insert(cur, g);
                stack.pop();
            } else {
                drop(known);
                stack.extend(unknown);
            }
        }
        Ok(self.generations.read().expect("lock")[id])
    }
}

/// Visits the ancestors of `heads` (including the heads), newest generation
/// first, ties broken by identifier, until `visit` returns `Ok(false)`.
/// Parents are only fetched for nodes that are actually visited, so taking the
/// first few entries of a huge history stays cheap.
pub fn walk_ancestors<G, F>(g: &G, heads: &[ChangesetId], mut visit: F) -> Result<(), DagError>
where
    G: Graph + ?Sized,
    F: FnMut(&ChangesetId) -> Result<bool, DagError>,
{
    let mut heap = BinaryHeap::new();
    let mut seen = HashSet::new();
    for h in heads {
        if seen.insert(*h) {
            heap.push((g.generation(h)?, *h));
        }
    }
    while let Some((_, id)) = heap.pop() {
        if !visit(&id)? {
            return Ok(());
        }
        for p in g.parents(&id)? {
            if seen.insert(p) {
                heap.push((g.generation(&p)?, p));
            }
        }
    }
    Ok(())
}

/// All ancestors of `heads` (including the heads), newest generation first,
/// ties broken by identifier. Stops with [`DagError::LimitExceeded`] after
/// `limit` changesets.
pub fn ancestors<G: Graph + ?Sized>(g: &G, heads: &[ChangesetId], limit: usize) -> Result<Vec<ChangesetId>, DagError> {
    let mut out = Vec::new();
    walk_ancestors(g, heads, |id| {
        if out.len() >= limit {
            return Err(DagError::LimitExceeded(limit));
        }
        out.push(*id);
        Ok(true)
    })?;
    Ok(out)
}

/// The first-parent chain starting at `head` (the "mainline" of a stream).
pub fn first_parent_chain<G: Graph + ?Sized>(
    g: &G,
    head: &ChangesetId,
    limit: usize,
) -> Result<Vec<ChangesetId>, DagError> {
    let mut out = vec![*head];
    let mut cur = *head;
    while let Some(p) = g.parents(&cur)?.first().copied() {
        if out.len() >= limit {
            return Err(DagError::LimitExceeded(limit));
        }
        out.push(p);
        cur = p;
    }
    Ok(out)
}

/// True if `ancestor` is reachable from `descendant` (a changeset is its own ancestor).
pub fn is_ancestor<G: Graph + ?Sized>(
    g: &G,
    ancestor: &ChangesetId,
    descendant: &ChangesetId,
) -> Result<bool, DagError> {
    if ancestor == descendant {
        return Ok(true);
    }
    let target_gen = g.generation(ancestor)?;
    let mut stack = vec![*descendant];
    let mut seen = HashSet::from([*descendant]);
    while let Some(cur) = stack.pop() {
        if g.generation(&cur)? <= target_gen {
            continue;
        }
        for p in g.parents(&cur)? {
            if &p == ancestor {
                return Ok(true);
            }
            if seen.insert(p) {
                stack.push(p);
            }
        }
    }
    Ok(false)
}

const P1: u8 = 1;
const P2: u8 = 2;
const STALE: u8 = 4;
const RESULT: u8 = 8;

/// All *best* common ancestors of `a` and `b`: common ancestors that are not
/// themselves ancestors of another common ancestor. Usually exactly one;
/// several indicate a criss-cross history. Sorted newest generation first,
/// ties by identifier. An empty result means the histories are unrelated.
pub fn merge_bases<G: Graph + ?Sized>(g: &G, a: &ChangesetId, b: &ChangesetId) -> Result<Vec<ChangesetId>, DagError> {
    if a == b {
        return Ok(vec![*a]);
    }
    let mut flags: HashMap<ChangesetId, u8> = HashMap::new();
    let mut heap: BinaryHeap<(u64, Reverse<ChangesetId>)> = BinaryHeap::new();
    flags.insert(*a, P1);
    heap.push((g.generation(a)?, Reverse(*a)));
    *flags.entry(*b).or_insert(0) |= P2;
    heap.push((g.generation(b)?, Reverse(*b)));

    let mut results: Vec<ChangesetId> = Vec::new();
    // Continue while at least one queued node is not stale.
    while heap.iter().any(|(_, Reverse(id))| flags[id] & STALE == 0) {
        let (_, Reverse(id)) = heap.pop().expect("non-empty");
        let mut f = flags[&id] & (P1 | P2 | STALE);
        if f == (P1 | P2) {
            let entry = flags.get_mut(&id).expect("present");
            if *entry & RESULT == 0 {
                *entry |= RESULT;
                results.push(id);
            }
            f |= STALE;
        }
        for p in g.parents(&id)? {
            let pf = flags.entry(p).or_insert(0);
            if *pf & f == f {
                continue;
            }
            *pf |= f;
            heap.push((g.generation(&p)?, Reverse(p)));
        }
    }
    let mut candidates: Vec<ChangesetId> =
        results.into_iter().filter(|id| flags[id] & STALE == 0 || flags[id] & RESULT != 0).collect();
    candidates.sort();
    candidates.dedup();
    // Remove redundant candidates (those that are ancestors of another candidate).
    let mut best = Vec::new();
    for (i, c) in candidates.iter().enumerate() {
        let mut redundant = false;
        for (j, other) in candidates.iter().enumerate() {
            if i != j && is_ancestor(g, c, other)? {
                redundant = true;
                break;
            }
        }
        if !redundant {
            best.push(*c);
        }
    }
    let mut keyed = Vec::with_capacity(best.len());
    for id in best {
        keyed.push((Reverse(g.generation(&id)?), id));
    }
    keyed.sort();
    Ok(keyed.into_iter().map(|(_, id)| id).collect())
}

/// The set of changesets reachable from `roots` (including the roots).
pub fn reachable<G: Graph + ?Sized>(
    g: &G,
    roots: impl IntoIterator<Item = ChangesetId>,
) -> Result<BTreeSet<ChangesetId>, DagError> {
    let mut seen: BTreeSet<ChangesetId> = BTreeSet::new();
    let mut stack: Vec<ChangesetId> = roots.into_iter().collect();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        stack.extend(g.parents(&id)?);
    }
    Ok(seen)
}

#[cfg(test)]
mod tests;
