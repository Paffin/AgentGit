use ag_changeset::{Changeset, Op};
use ag_types::{ChangesetId, Timestamp};
use serde::Serialize;

use super::db::params;
use super::{MetaError, Tx, escape_like, parse, ts};

/// Index row of a published changeset.
#[derive(Clone, Debug, Serialize)]
pub struct ChangesetInfo {
    /// Identifier.
    pub id: ChangesetId,
    /// Generation number (1 for roots).
    pub generation: u64,
    /// Author identity.
    pub author: String,
    /// Author timestamp.
    pub timestamp: Timestamp,
    /// Ordered parents.
    pub parents: Vec<ChangesetId>,
    /// When the changeset became ready (durably published).
    pub ready_at: Timestamp,
    /// First line of the message.
    pub summary: String,
}

fn summary_of(message: &str) -> String {
    let line = message.lines().next().unwrap_or("").trim();
    line.chars().take(200).collect()
}

impl Tx<'_> {
    /// Marks a changeset as ready (published). Every parent must already be
    /// ready; the generation number is derived from them. Idempotent.
    pub fn register_changeset(&self, id: &ChangesetId, cs: &Changeset) -> Result<u64, MetaError> {
        if let Some(g) = self.generation(id)? {
            return Ok(g);
        }
        let mut max_gen = 0u64;
        for p in &cs.parents {
            let g = self.generation(p)?.ok_or(MetaError::ParentNotReady(*p))?;
            max_gen = max_gen.max(g);
        }
        let generation = max_gen + 1;
        // Depth along first parents: the keyframe schedule of the derived-tree cache.
        let fp_depth = match cs.first_parent() {
            Some(p) => self.fp_depth(&p)?.ok_or(MetaError::ParentNotReady(p))? + 1,
            None => 1,
        };
        if generation > self.limits.max_dag_depth {
            return Err(MetaError::Invalid(format!("history deeper than {} changesets", self.limits.max_dag_depth)));
        }
        self.conn.execute(
            "INSERT INTO changesets (id, generation, fp_depth, author, ts, first_parent, parent_count, ready_at, summary)
             VALUES (?1, ?2, ?9, ?3, ?4, ?5, ?6, ?7, ?8)",
            &params![
                id.to_hex(),
                generation as i64,
                cs.author.as_str(),
                cs.timestamp.as_millis(),
                cs.first_parent().map(|p| p.to_hex()),
                cs.parents.len() as i64,
                self.now().as_millis(),
                summary_of(&cs.message),
                fp_depth as i64,
            ],
        )?;
        for (i, p) in cs.parents.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO changeset_parents (child, ord, parent) VALUES (?1, ?2, ?3)",
                &params![id.to_hex(), i as i64, p.to_hex()],
            )?;
        }
        self.insert_paths(id, cs)?;
        Ok(generation)
    }

    fn insert_paths(&self, id: &ChangesetId, cs: &Changeset) -> Result<(), MetaError> {
        for op in &cs.ops {
            let kind = match op {
                Op::Put { .. } => "put",
                Op::Delete { .. } => "delete",
            };
            self.conn.execute(
                "INSERT INTO changeset_paths (changeset, path, op) VALUES (?1, ?2, ?3)",
                &params![id.to_hex(), op.path().as_str(), kind],
            )?;
        }
        Ok(())
    }

    /// Generation number of a published changeset.
    pub fn generation(&self, id: &ChangesetId) -> Result<Option<u64>, MetaError> {
        self.conn
            .query_opt("SELECT generation FROM changesets WHERE id = ?1", &params![id.to_hex()])?
            .map(|r| Ok(r.i64(0)? as u64))
            .transpose()
    }

    /// Depth of the first-parent chain ending at a published changeset (1 for roots).
    pub fn fp_depth(&self, id: &ChangesetId) -> Result<Option<u64>, MetaError> {
        self.conn
            .query_opt("SELECT fp_depth FROM changesets WHERE id = ?1", &params![id.to_hex()])?
            .map(|r| Ok(r.i64(0)? as u64))
            .transpose()
    }

    /// True if the changeset is published.
    pub fn is_ready(&self, id: &ChangesetId) -> Result<bool, MetaError> {
        Ok(self.generation(id)?.is_some())
    }

    /// Ordered parents of a published changeset.
    pub fn parents(&self, id: &ChangesetId) -> Result<Vec<ChangesetId>, MetaError> {
        let rows = self
            .conn
            .query("SELECT parent FROM changeset_parents WHERE child = ?1 ORDER BY ord", &params![id.to_hex()])?;
        rows.iter().map(|r| parse::<ChangesetId>(r.str(0)?, "parent id")).collect()
    }

    /// Index information about a published changeset.
    pub fn changeset_info(&self, id: &ChangesetId) -> Result<Option<ChangesetInfo>, MetaError> {
        let row = self.conn.query_opt(
            "SELECT generation, author, ts, ready_at, summary FROM changesets WHERE id = ?1",
            &params![id.to_hex()],
        )?;
        let Some(r) = row else { return Ok(None) };
        Ok(Some(ChangesetInfo {
            id: *id,
            generation: r.i64(0)? as u64,
            author: r.string(1)?,
            timestamp: ts(r.i64(2)?),
            parents: self.parents(id)?,
            ready_at: ts(r.i64(3)?),
            summary: r.string(4)?,
        }))
    }

    /// Published changesets whose id starts with `prefix` (lowercase hex).
    pub fn resolve_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<ChangesetId>, MetaError> {
        if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(MetaError::Invalid(format!("'{prefix}' is not a hex prefix")));
        }
        // id range scan: [prefix, prefix + 'g') since 'g' sorts after every hex digit
        // (byte-wise on both backends: the PostgreSQL columns use the "C" collation)
        let upper = format!("{prefix}g");
        let rows = self.conn.query(
            "SELECT id FROM changesets WHERE id >= ?1 AND id < ?2 ORDER BY id LIMIT ?3",
            &params![prefix, upper, limit as i64],
        )?;
        rows.iter().map(|r| parse::<ChangesetId>(r.str(0)?, "changeset id")).collect()
    }

    /// Paths touched by a changeset, with the operation kind.
    pub fn changeset_paths(&self, id: &ChangesetId) -> Result<Vec<(String, String)>, MetaError> {
        let rows = self
            .conn
            .query("SELECT path, op FROM changeset_paths WHERE changeset = ?1 ORDER BY path", &params![id.to_hex()])?;
        rows.iter().map(|r| Ok((r.string(0)?, r.string(1)?))).collect()
    }

    /// True if the changeset touched `path` itself or anything beneath it.
    pub fn touches_path(&self, id: &ChangesetId, path: &str) -> Result<bool, MetaError> {
        let like = format!("{}/%", escape_like(path));
        // `lower()` on both sides pins the case-insensitive (ASCII) comparison that
        // SQLite's LIKE has by itself, so PostgreSQL's case-sensitive LIKE agrees.
        let n = self
            .conn
            .query_one(
                "SELECT COUNT(*) FROM changeset_paths
                 WHERE changeset = ?1 AND (lower(path) = lower(?2) OR lower(path) LIKE lower(?3) ESCAPE '\\')",
                &params![id.to_hex(), path, like],
            )?
            .i64(0)?;
        Ok(n > 0)
    }

    /// Every published changeset id, oldest generation first.
    pub fn all_changeset_ids(&self) -> Result<Vec<ChangesetId>, MetaError> {
        let rows = self.conn.query("SELECT id FROM changesets ORDER BY generation, id", &[])?;
        rows.iter().map(|r| parse::<ChangesetId>(r.str(0)?, "changeset id")).collect()
    }

    /// Number of published changesets.
    pub fn count_changesets(&self) -> Result<u64, MetaError> {
        Ok(self.conn.query_one("SELECT COUNT(*) FROM changesets", &[])?.i64(0)? as u64)
    }

    /// Removes the index rows of changesets that were garbage collected.
    pub fn delete_changesets(&self, ids: &[ChangesetId]) -> Result<(), MetaError> {
        for id in ids {
            let h = id.to_hex();
            self.conn.execute("DELETE FROM changeset_parents WHERE child = ?1", &params![&h])?;
            self.conn.execute("DELETE FROM changeset_paths WHERE changeset = ?1", &params![&h])?;
            self.conn.execute("DELETE FROM changeset_trees WHERE changeset = ?1", &params![&h])?;
            self.conn.execute("DELETE FROM changesets WHERE id = ?1", &params![&h])?;
        }
        Ok(())
    }

    /// Rebuilds the path index rows of one changeset (used by reindex).
    pub fn rebuild_paths(&self, id: &ChangesetId, cs: &Changeset) -> Result<(), MetaError> {
        self.conn.execute("DELETE FROM changeset_paths WHERE changeset = ?1", &params![id.to_hex()])?;
        self.insert_paths(id, cs)
    }
}
