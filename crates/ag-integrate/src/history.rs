use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use ag_changeset::ChangesetSource;
use ag_dag::{DagError, Graph, walk_ancestors};
use ag_storage::{Meta, ObjectStore, StoreHistory};
use ag_types::{ChangesetId, ObjectId, Timestamp};
use serde::Serialize;

use crate::{Error, Repository, Result};

/// A [`Graph`] that prefers the metadata index (published changesets, with
/// stored generation numbers) and falls back to the object store for
/// changesets that are not published yet, such as workspace snapshots.
pub struct HybridGraph<'a> {
    meta: &'a Meta,
    source: &'a StoreHistory<Arc<dyn ObjectStore>>,
    memo: RwLock<HashMap<ChangesetId, u64>>,
}

impl<'a> HybridGraph<'a> {
    pub(crate) fn new(meta: &'a Meta, source: &'a StoreHistory<Arc<dyn ObjectStore>>) -> Self {
        Self { meta, source, memo: RwLock::new(HashMap::new()) }
    }

    fn meta_err(e: ag_storage::meta::MetaError) -> DagError {
        DagError::Source(ag_changeset::SourceError::Other(e.to_string()))
    }
}

impl Graph for HybridGraph<'_> {
    fn parents(&self, id: &ChangesetId) -> std::result::Result<Vec<ChangesetId>, DagError> {
        if self.meta.read(|tx| tx.is_ready(id)).map_err(Self::meta_err)? {
            return self.meta.read(|tx| tx.parents(id)).map_err(Self::meta_err);
        }
        Ok(self.source.changeset(id)?.parents.clone())
    }

    fn generation(&self, id: &ChangesetId) -> std::result::Result<u64, DagError> {
        if let Some(g) = self.memo.read().expect("lock").get(id) {
            return Ok(*g);
        }
        let mut stack = vec![*id];
        while let Some(cur) = stack.last().copied() {
            if self.memo.read().expect("lock").contains_key(&cur) {
                stack.pop();
                continue;
            }
            if let Some(g) = self.meta.read(|tx| tx.generation(&cur)).map_err(Self::meta_err)? {
                self.memo.write().expect("lock").insert(cur, g);
                stack.pop();
                continue;
            }
            let parents = self.source.changeset(&cur)?.parents.clone();
            let unknown: Vec<_> = {
                let memo = self.memo.read().expect("lock");
                parents.iter().filter(|p| !memo.contains_key(p)).copied().collect()
            };
            if unknown.is_empty() {
                let g = 1 + parents.iter().map(|p| self.memo.read().expect("lock")[p]).max().unwrap_or(0);
                self.memo.write().expect("lock").insert(cur, g);
                stack.pop();
            } else {
                // Parents that are published resolve through the index on the next loop turn.
                stack.extend(unknown);
            }
        }
        Ok(self.memo.read().expect("lock")[id])
    }
}

/// One entry of a history listing.
#[derive(Clone, Debug, Serialize)]
pub struct LogEntry {
    /// Changeset id.
    pub id: ChangesetId,
    /// Generation number.
    pub generation: u64,
    /// Author.
    pub author: String,
    /// Author timestamp.
    pub timestamp: Timestamp,
    /// Full message.
    pub message: String,
    /// Parents.
    pub parents: Vec<ChangesetId>,
}

impl Repository {
    /// Resolves a revision expression to a changeset.
    ///
    /// Grammar: `base` followed by any number of `~N` (N-th first-parent
    /// ancestor) and `^N` (N-th parent, default 1) suffixes.
    ///
    /// `base` is, in order: `@` (head of the default workspace), a full
    /// 64-hex changeset id, a proposal id (`prop_…`), a stream name, or a hex
    /// prefix of at least 6 characters that identifies one published
    /// changeset.
    pub fn resolve_rev(&self, spec: &str) -> Result<ChangesetId> {
        let split = spec.find(['~', '^']).unwrap_or(spec.len());
        let (base, mut rest) = spec.split_at(split);
        let mut cur = self.resolve_base(base)?;
        while !rest.is_empty() {
            let op = rest.as_bytes()[0];
            let digits_end = rest[1..].find(['~', '^']).map(|i| i + 1).unwrap_or(rest.len());
            let n_str = &rest[1..digits_end];
            let n: usize = if n_str.is_empty() { 1 } else { n_str.parse().map_err(|_| Error::Invalid(format!("bad revision suffix in '{spec}'")))? };
            if n > 100_000 {
                return Err(Error::Invalid(format!("revision suffix too large in '{spec}'")));
            }
            let cs = self.changeset(&cur)?;
            cur = if op == b'~' {
                let mut c = cur;
                for _ in 0..n {
                    c = self.changeset(&c)?.first_parent().ok_or_else(|| Error::NotFound(format!("'{spec}' goes past the root")))?;
                }
                c
            } else if n == 0 {
                cur
            } else {
                *cs.parents.get(n - 1).ok_or_else(|| Error::NotFound(format!("{} has no parent {n}", cur.short())))?
            };
            rest = &rest[digits_end..];
        }
        Ok(cur)
    }

    fn resolve_base(&self, base: &str) -> Result<ChangesetId> {
        if base.is_empty() {
            return Err(Error::Invalid("empty revision".into()));
        }
        if base == "@" {
            return self
                .meta
                .read(|tx| tx.workspace("default"))?
                .and_then(|w| w.head)
                .ok_or_else(|| Error::NotFound("the default workspace has no snapshots yet".into()));
        }
        if let Some(name) = base.strip_prefix('@') {
            return self
                .meta
                .read(|tx| tx.workspace(name))?
                .and_then(|w| w.head)
                .ok_or_else(|| Error::NotFound(format!("workspace '{name}' has no snapshots yet")));
        }
        if base.len() == 64 {
            if let Ok(id) = ChangesetId::from_str(base) {
                if self.objects.contains(&ObjectId::from(id))? {
                    return Ok(id);
                }
                return Err(Error::NotFound(format!("changeset {base}")));
            }
        }
        if base.starts_with("prop_") {
            return Ok(self.meta.read(|tx| tx.proposal(base))?.ok_or_else(|| Error::NotFound(format!("proposal '{base}'")))?.head);
        }
        if let Some(s) = self.meta.read(|tx| tx.stream(base))? {
            return s.state.head.ok_or_else(|| Error::NotFound(format!("stream '{base}' has no changesets yet")));
        }
        if base.len() >= 6 && base.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            let found = self.meta.read(|tx| tx.resolve_prefix(base, 3))?;
            return match found.as_slice() {
                [one] => Ok(*one),
                [] => Err(Error::NotFound(format!("no published changeset starts with '{base}'"))),
                _ => Err(Error::Invalid(format!("'{base}' is ambiguous; use more characters"))),
            };
        }
        Err(Error::NotFound(format!("cannot resolve '{base}' (not a stream, proposal, workspace or changeset)")))
    }

    /// History of `head`, newest generation first. With `path`, only
    /// changesets that touched it (or anything beneath it). Stops after
    /// `limit` entries without walking the rest of the history.
    pub fn log(&self, head: &ChangesetId, limit: usize, path: Option<&str>) -> Result<Vec<LogEntry>> {
        let g = self.graph();
        let walk_limit = self.limits.max_dag_depth.min(5_000_000) as usize;
        let mut out = Vec::new();
        let mut visited = 0usize;
        let mut failure: Option<Error> = None;
        walk_ancestors(&g, &[*head], |id| {
            visited += 1;
            if visited > walk_limit {
                return Err(DagError::LimitExceeded(walk_limit));
            }
            let step = (|| -> Result<()> {
                let cs = self.changeset(id)?;
                if let Some(p) = path {
                    let prefix = format!("{p}/");
                    if !cs.ops.iter().any(|op| op.path().as_str() == p || op.path().as_str().starts_with(&prefix)) {
                        return Ok(());
                    }
                }
                out.push(LogEntry {
                    id: *id,
                    generation: g.generation(id)?,
                    author: cs.author.to_string(),
                    timestamp: cs.timestamp,
                    message: cs.message.clone(),
                    parents: cs.parents.clone(),
                });
                Ok(())
            })();
            if let Err(e) = step {
                failure = Some(e);
                return Ok(false);
            }
            Ok(out.len() < limit)
        })?;
        match failure {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }
}
