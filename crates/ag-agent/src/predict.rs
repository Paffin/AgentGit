//! Early detection of overlapping work ("conflict prediction").
//!
//! This is a *path-level* signal computed from the metadata index alone:
//! cheap, conservative and meant for scheduling decisions. Whether two
//! overlapping edits actually conflict is decided by the merge engine
//! (`ag merge-preview` / integration), never by this module.

use std::collections::{BTreeMap, BTreeSet};

use ag_dag::{Graph, ancestors};
use ag_policy::PathMatcher;
use ag_storage::Meta;
use ag_storage::meta::Tx;
use serde::Serialize;

use crate::AgentError;
use crate::leases::scopes_may_overlap;

/// A pair of work items that touch the same area.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Overlap {
    /// First item, e.g. `proposal:prop_1`.
    pub a: String,
    /// Second item, e.g. `stream:main`.
    pub b: String,
    /// What kind of overlap.
    pub kind: String,
    /// Shared paths or scopes (sorted, truncated to 20).
    pub paths: Vec<String>,
}

/// Result of [`predict`].
#[derive(Clone, Debug, Serialize)]
pub struct Prediction {
    /// All overlaps found, sorted.
    pub overlaps: Vec<Overlap>,
}

const WALK_LIMIT: usize = 50_000;

fn paths_of(tx: &Tx<'_>, ids: &[ag_types::ChangesetId]) -> Result<BTreeSet<String>, AgentError> {
    let mut out = BTreeSet::new();
    for id in ids {
        for (p, _) in tx.changeset_paths(id)? {
            out.insert(p);
        }
    }
    Ok(out)
}

fn clip(set: impl IntoIterator<Item = String>) -> Vec<String> {
    set.into_iter().take(20).collect()
}

/// Predicts overlaps among the open proposals of `stream`, the stream's own
/// recent history, active leases and in-progress tasks.
pub fn predict(meta: &Meta, stream: &str) -> Result<Prediction, AgentError> {
    meta.read(|tx| {
        let mut overlaps: BTreeSet<Overlap> = BTreeSet::new();
        let head = tx.stream(stream)?.and_then(|s| s.state.head);
        let head_ancestors: BTreeSet<_> = match head {
            Some(h) => ancestors(tx as &dyn Graph, &[h], WALK_LIMIT).map_err(|e| ag_storage::meta::MetaError::Invalid(e.to_string()))?.into_iter().collect(),
            None => BTreeSet::new(),
        };

        // Per-proposal touched paths (changes not yet in the stream) and the
        // stream's own changes since the proposal diverged.
        let proposals: Vec<_> = tx.proposals_list(Some("open"), 1000)?.into_iter().filter(|p| p.stream == stream).collect();
        let mut touched: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for p in &proposals {
            let anc: BTreeSet<_> = ancestors(tx as &dyn Graph, &[p.head], WALK_LIMIT)
                .map_err(|e| ag_storage::meta::MetaError::Invalid(e.to_string()))?
                .into_iter()
                .collect();
            let mine: Vec<_> = anc.difference(&head_ancestors).copied().collect();
            let theirs: Vec<_> = head_ancestors.difference(&anc).copied().collect();
            let my_paths = paths_of(tx, &mine).map_err(|e| ag_storage::meta::MetaError::Invalid(e.to_string()))?;
            let stream_paths = paths_of(tx, &theirs).map_err(|e| ag_storage::meta::MetaError::Invalid(e.to_string()))?;
            let shared: Vec<String> = my_paths.intersection(&stream_paths).cloned().collect();
            if !shared.is_empty() {
                overlaps.insert(Overlap { a: format!("proposal:{}", p.id), b: format!("stream:{stream}"), kind: "stream_moved_under_proposal".into(), paths: clip(shared) });
            }
            touched.insert(p.id.clone(), my_paths);
        }

        // Proposal vs proposal.
        let ids: Vec<_> = touched.keys().cloned().collect();
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                let shared: Vec<String> = touched[a].intersection(&touched[b]).cloned().collect();
                if !shared.is_empty() {
                    overlaps.insert(Overlap { a: format!("proposal:{a}"), b: format!("proposal:{b}"), kind: "proposals_touch_same_paths".into(), paths: clip(shared) });
                }
            }
        }

        // Active leases vs proposals of other principals.
        let leases = tx.leases_active()?;
        for l in &leases {
            let Ok(m) = PathMatcher::new(std::slice::from_ref(&l.scope)) else { continue };
            for p in &proposals {
                if p.author.split('#').next() == l.holder.split('#').next() {
                    continue;
                }
                let hits: Vec<String> = touched[&p.id].iter().filter(|x| m.is_match(x)).cloned().collect();
                if !hits.is_empty() {
                    overlaps.insert(Overlap { a: format!("proposal:{}", p.id), b: format!("lease:{}", l.id), kind: "proposal_inside_foreign_lease".into(), paths: clip(hits) });
                }
            }
        }
        // Leases of different holders overlapping each other.
        for (i, a) in leases.iter().enumerate() {
            for b in &leases[i + 1..] {
                if a.holder.split('#').next() != b.holder.split('#').next() && scopes_may_overlap(&a.scope, &b.scope) {
                    overlaps.insert(Overlap { a: format!("lease:{}", a.id), b: format!("lease:{}", b.id), kind: "leases_overlap".into(), paths: vec![a.scope.clone(), b.scope.clone()] });
                }
            }
        }

        // Tasks in progress with overlapping scopes.
        let tasks = tx.tasks(Some("in_progress"))?;
        for (i, a) in tasks.iter().enumerate() {
            for b in &tasks[i + 1..] {
                let shared: Vec<String> = a
                    .scope
                    .iter()
                    .flat_map(|x| b.scope.iter().filter(move |y| scopes_may_overlap(x, y)).map(move |y| format!("{x} ~ {y}")))
                    .collect();
                if !shared.is_empty() {
                    overlaps.insert(Overlap { a: format!("task:{}", a.id), b: format!("task:{}", b.id), kind: "task_scopes_overlap".into(), paths: clip(shared) });
                }
            }
        }
        Ok(Prediction { overlaps: overlaps.into_iter().collect() })
    })
    .map_err(|e| match e {
        ag_storage::meta::MetaError::Invalid(m) => AgentError::Invalid(m),
        other => AgentError::Meta(other),
    })
}
