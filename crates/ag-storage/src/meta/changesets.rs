use ag_changeset::{Changeset, Op};
use ag_types::{ChangesetId, Timestamp};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use super::{MetaError, Tx, parse, ts};

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
            params![
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
                params![id.to_hex(), i as i64, p.to_hex()],
            )?;
        }
        for op in &cs.ops {
            let kind = match op {
                Op::Put { .. } => "put",
                Op::Delete { .. } => "delete",
            };
            self.conn.execute(
                "INSERT INTO changeset_paths (changeset, path, op) VALUES (?1, ?2, ?3)",
                params![id.to_hex(), op.path().as_str(), kind],
            )?;
        }
        Ok(generation)
    }

    /// Generation number of a published changeset.
    pub fn generation(&self, id: &ChangesetId) -> Result<Option<u64>, MetaError> {
        Ok(self
            .conn
            .query_row("SELECT generation FROM changesets WHERE id = ?1", [id.to_hex()], |r| r.get::<_, i64>(0))
            .optional()?
            .map(|g| g as u64))
    }

    /// Depth of the first-parent chain ending at a published changeset (1 for roots).
    pub fn fp_depth(&self, id: &ChangesetId) -> Result<Option<u64>, MetaError> {
        Ok(self
            .conn
            .query_row("SELECT fp_depth FROM changesets WHERE id = ?1", [id.to_hex()], |r| r.get::<_, i64>(0))
            .optional()?
            .map(|g| g as u64))
    }

    /// True if the changeset is published.
    pub fn is_ready(&self, id: &ChangesetId) -> Result<bool, MetaError> {
        Ok(self.generation(id)?.is_some())
    }

    /// Ordered parents of a published changeset.
    pub fn parents(&self, id: &ChangesetId) -> Result<Vec<ChangesetId>, MetaError> {
        let mut stmt =
            self.conn.prepare_cached("SELECT parent FROM changeset_parents WHERE child = ?1 ORDER BY ord")?;
        let rows = stmt.query_map([id.to_hex()], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(parse::<ChangesetId>(&r?, "parent id")?);
        }
        Ok(out)
    }

    /// Index information about a published changeset.
    pub fn changeset_info(&self, id: &ChangesetId) -> Result<Option<ChangesetInfo>, MetaError> {
        let row = self
            .conn
            .query_row(
                "SELECT generation, author, ts, ready_at, summary FROM changesets WHERE id = ?1",
                [id.to_hex()],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((generation, author, t, ready_at, summary)) = row else { return Ok(None) };
        Ok(Some(ChangesetInfo {
            id: *id,
            generation: generation as u64,
            author,
            timestamp: ts(t),
            parents: self.parents(id)?,
            ready_at: ts(ready_at),
            summary,
        }))
    }

    /// Published changesets whose id starts with `prefix` (lowercase hex).
    pub fn resolve_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<ChangesetId>, MetaError> {
        if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(MetaError::Invalid(format!("'{prefix}' is not a hex prefix")));
        }
        let mut stmt =
            self.conn.prepare_cached("SELECT id FROM changesets WHERE id >= ?1 AND id < ?2 ORDER BY id LIMIT ?3")?;
        // id range scan: [prefix, prefix + 'g') since 'g' sorts after every hex digit
        let upper = format!("{prefix}g");
        let rows = stmt.query_map(params![prefix, upper, limit as i64], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(parse::<ChangesetId>(&r?, "changeset id")?);
        }
        Ok(out)
    }

    /// Paths touched by a changeset, with the operation kind.
    pub fn changeset_paths(&self, id: &ChangesetId) -> Result<Vec<(String, String)>, MetaError> {
        let mut stmt =
            self.conn.prepare_cached("SELECT path, op FROM changeset_paths WHERE changeset = ?1 ORDER BY path")?;
        let rows = stmt.query_map([id.to_hex()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// True if the changeset touched `path` itself or anything beneath it.
    pub fn touches_path(&self, id: &ChangesetId, path: &str) -> Result<bool, MetaError> {
        let like = format!("{}/%", path.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM changeset_paths WHERE changeset = ?1 AND (path = ?2 OR path LIKE ?3 ESCAPE '\\')",
            params![id.to_hex(), path, like],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Every published changeset id, oldest generation first.
    pub fn all_changeset_ids(&self) -> Result<Vec<ChangesetId>, MetaError> {
        let mut stmt = self.conn.prepare_cached("SELECT id FROM changesets ORDER BY generation, id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(parse::<ChangesetId>(&r?, "changeset id")?);
        }
        Ok(out)
    }

    /// Number of published changesets.
    pub fn count_changesets(&self) -> Result<u64, MetaError> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM changesets", [], |r| r.get::<_, i64>(0))? as u64)
    }

    /// Removes the index rows of changesets that were garbage collected.
    pub fn delete_changesets(&self, ids: &[ChangesetId]) -> Result<(), MetaError> {
        for id in ids {
            let h = id.to_hex();
            self.conn.execute("DELETE FROM changeset_parents WHERE child = ?1", [&h])?;
            self.conn.execute("DELETE FROM changeset_paths WHERE changeset = ?1", [&h])?;
            self.conn.execute("DELETE FROM changeset_trees WHERE changeset = ?1", [&h])?;
            self.conn.execute("DELETE FROM changesets WHERE id = ?1", [&h])?;
        }
        Ok(())
    }

    /// Rebuilds the path index rows of one changeset (used by reindex).
    pub fn rebuild_paths(&self, id: &ChangesetId, cs: &Changeset) -> Result<(), MetaError> {
        self.conn.execute("DELETE FROM changeset_paths WHERE changeset = ?1", [id.to_hex()])?;
        for op in &cs.ops {
            let kind = match op {
                Op::Put { .. } => "put",
                Op::Delete { .. } => "delete",
            };
            self.conn.execute(
                "INSERT INTO changeset_paths (changeset, path, op) VALUES (?1, ?2, ?3)",
                params![id.to_hex(), op.path().as_str(), kind],
            )?;
        }
        Ok(())
    }
}
