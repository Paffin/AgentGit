use ag_tree::Tree;
use ag_types::{ChangesetId, TreeId};

use super::db::params;
use super::{MetaError, Tx, parse};

impl Tx<'_> {
    /// Stores the derived tree of a changeset (idempotent).
    pub fn put_derived_tree(&self, changeset: &ChangesetId, tree: &Tree) -> Result<TreeId, MetaError> {
        let id = tree.id();
        self.conn.execute(
            "INSERT INTO trees (tree_id, data) VALUES (?1, ?2) ON CONFLICT (tree_id) DO NOTHING",
            &params![id.to_hex(), tree.encode()],
        )?;
        self.conn.execute(
            "INSERT INTO changeset_trees (changeset, tree_id) VALUES (?1, ?2)
             ON CONFLICT (changeset) DO UPDATE SET tree_id = excluded.tree_id",
            &params![changeset.to_hex(), id.to_hex()],
        )?;
        Ok(id)
    }

    /// The tree id recorded for a changeset, if it has been derived.
    pub fn derived_tree_id(&self, changeset: &ChangesetId) -> Result<Option<TreeId>, MetaError> {
        self.conn
            .query_opt("SELECT tree_id FROM changeset_trees WHERE changeset = ?1", &params![changeset.to_hex()])?
            .map(|r| parse::<TreeId>(r.str(0)?, "tree id"))
            .transpose()
    }

    /// Loads and verifies the derived tree of a changeset.
    ///
    /// A cached tree whose manifest does not hash to its recorded id is
    /// treated as corruption of a *disposable* cache: it returns `None` so the
    /// caller recomputes from history.
    pub fn derived_tree(&self, changeset: &ChangesetId) -> Result<Option<Tree>, MetaError> {
        let row = self.conn.query_opt(
            "SELECT t.tree_id, t.data FROM changeset_trees c JOIN trees t ON t.tree_id = c.tree_id WHERE c.changeset = ?1",
            &params![changeset.to_hex()],
        )?;
        let Some(row) = row else { return Ok(None) };
        let Ok(tree) = Tree::decode(row.blob(1)?, self.limits) else { return Ok(None) };
        if tree.id().to_hex() != row.str(0)? {
            return Ok(None);
        }
        Ok(Some(tree))
    }

    /// Published changesets that have no derived tree yet, oldest first.
    pub fn changesets_without_tree(&self, limit: usize) -> Result<Vec<ChangesetId>, MetaError> {
        let rows = self.conn.query(
            "SELECT c.id FROM changesets c LEFT JOIN changeset_trees t ON t.changeset = c.id
             WHERE t.changeset IS NULL ORDER BY c.generation, c.id LIMIT ?1",
            &params![limit as i64],
        )?;
        rows.iter().map(|r| parse::<ChangesetId>(r.str(0)?, "changeset id")).collect()
    }

    /// Drops cached trees that are neither keyframes (`fp_depth % interval == 0`)
    /// nor the tree of a live head (stream, open proposal, workspace). Reading
    /// any other tree replays at most `interval - 1` deltas from the nearest
    /// keyframe, so the cache costs one manifest per `interval` changesets
    /// instead of one per changeset. Returns the number of entries dropped.
    pub fn prune_derived(&self, interval: u64) -> Result<u64, MetaError> {
        let n = self.conn.execute(
            "DELETE FROM changeset_trees WHERE changeset IN (
                SELECT t.changeset FROM changeset_trees t JOIN changesets c ON c.id = t.changeset
                WHERE c.fp_depth % ?1 != 0
                  AND t.changeset NOT IN (SELECT head FROM streams WHERE head IS NOT NULL)
                  AND t.changeset NOT IN (SELECT head FROM proposals WHERE state = 'open')
                  AND t.changeset NOT IN (SELECT head FROM workspaces WHERE head IS NOT NULL)
                  AND t.changeset NOT IN (SELECT base FROM workspaces WHERE base IS NOT NULL))",
            &params![interval as i64],
        )?;
        self.prune_orphan_trees()?;
        Ok(n)
    }

    /// Drops every derived tree (they can all be recomputed).
    pub fn clear_derived(&self) -> Result<(), MetaError> {
        self.conn.execute("DELETE FROM changeset_trees", &[])?;
        self.conn.execute("DELETE FROM trees", &[])?;
        Ok(())
    }

    /// Removes manifests no longer referenced by any changeset.
    pub fn prune_orphan_trees(&self) -> Result<u64, MetaError> {
        self.conn.execute("DELETE FROM trees WHERE tree_id NOT IN (SELECT tree_id FROM changeset_trees)", &[])
    }
}
