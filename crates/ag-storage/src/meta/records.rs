use ag_types::{ChangesetId, Timestamp, TreeId};
use rusqlite::params;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{MetaError, Tx, parse, stream_name, ts};

/// A proposal: a request to integrate a changeset into a stream.
#[derive(Clone, Debug, Serialize)]
pub struct ProposalRow {
    /// Identifier, e.g. `prop_…`.
    pub id: String,
    /// Target stream.
    pub stream: String,
    /// Proposed head changeset.
    pub head: ChangesetId,
    /// Title.
    pub title: String,
    /// Description.
    pub description: String,
    /// Author identity.
    pub author: String,
    /// `open`, `integrated` or `closed`.
    pub state: String,
    /// Task this proposal belongs to.
    pub task_id: Option<String>,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last update.
    pub updated_at: Timestamp,
    /// Changeset created by integration.
    pub integrated_as: Option<ChangesetId>,
}

/// A review decision bound to the exact head that was reviewed.
#[derive(Clone, Debug, Serialize)]
pub struct ReviewRow {
    /// Row id.
    pub id: u64,
    /// Proposal id.
    pub proposal: String,
    /// Reviewer identity.
    pub reviewer: String,
    /// `approve`, `request_changes` or `comment`.
    pub decision: String,
    /// Free text.
    pub body: String,
    /// Proposal head at the time of review; approvals of an older head are stale.
    pub head: ChangesetId,
    /// When.
    pub timestamp: Timestamp,
}

/// A stored, signed attestation.
#[derive(Clone, Debug, Serialize)]
pub struct AttestationRow {
    /// Identifier.
    pub id: String,
    /// The exact tree that was checked.
    pub tree_id: TreeId,
    /// Check name, e.g. `tests`.
    pub check_name: String,
    /// `pass` or `fail`.
    pub result: String,
    /// Policy version the check ran under.
    pub policy_version: u64,
    /// Digest of the execution environment.
    pub environment_digest: String,
    /// Issuer identity.
    pub issuer: String,
    /// When issued.
    pub issued_at: Timestamp,
    /// Issuer public key (hex).
    pub public_key: String,
    /// Signature (hex).
    pub signature: String,
    /// Canonical signed payload (JSON).
    pub payload: Value,
}

/// A stored policy version.
#[derive(Clone, Debug, Serialize)]
pub struct PolicyRow {
    /// Stream the policy governs.
    pub stream: String,
    /// Version, starting at 1.
    pub version: u64,
    /// Policy document (JSON text).
    pub doc: String,
    /// SHA-256 of the document.
    pub doc_hash: String,
    /// When stored.
    pub created_at: Timestamp,
    /// Who stored it.
    pub created_by: String,
}

/// A retention root.
#[derive(Clone, Debug, Serialize)]
pub struct PinRow {
    /// Pin name.
    pub name: String,
    /// Pinned changeset.
    pub changeset: ChangesetId,
    /// Why.
    pub reason: String,
    /// When.
    pub created_at: Timestamp,
}

/// A workspace: an isolated line of work based on a stream.
#[derive(Clone, Debug, Serialize)]
pub struct WorkspaceRow {
    /// Name.
    pub name: String,
    /// Stream it tracks.
    pub stream: String,
    /// Stream head the workspace started from (or was last synced to).
    pub base: Option<ChangesetId>,
    /// The workspace's own head (latest snapshot).
    pub head: Option<ChangesetId>,
    /// Owner identity.
    pub owner: String,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last update.
    pub updated_at: Timestamp,
}

fn opt_cs(s: Option<String>, what: &str) -> Result<Option<ChangesetId>, MetaError> {
    s.map(|s| parse::<ChangesetId>(&s, what)).transpose()
}

impl Tx<'_> {
    // ---- proposals -------------------------------------------------------

    /// Creates an open proposal.
    pub fn proposal_create(
        &self,
        id: &str,
        stream: &str,
        head: &ChangesetId,
        title: &str,
        description: &str,
        author: &str,
        task_id: Option<&str>,
    ) -> Result<ProposalRow, MetaError> {
        let now = self.now().as_millis();
        self.conn.execute(
            "INSERT INTO proposals (id, stream, head, title, description, author, state, task_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7, ?8, ?8)",
            params![id, stream, head.to_hex(), title, description, author, task_id, now],
        )?;
        self.proposal(id)?.ok_or_else(|| MetaError::Corrupt("proposal vanished".into()))
    }

    /// Looks up a proposal.
    pub fn proposal(&self, id: &str) -> Result<Option<ProposalRow>, MetaError> {
        self.proposals_where("id = ?1", params![id]).map(|mut v| v.pop())
    }

    /// Proposals whose id starts with `prefix`, at most `limit`.
    pub fn proposals_by_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<ProposalRow>, MetaError> {
        let like = format!("{}%", prefix.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        self.proposals_where("id LIKE ?1 ESCAPE '\\' ORDER BY id LIMIT ?2", params![like, limit as i64])
    }

    /// Proposals in `state` (or all), newest first.
    pub fn proposals_list(&self, state: Option<&str>, limit: usize) -> Result<Vec<ProposalRow>, MetaError> {
        match state {
            Some(s) => self.proposals_where("state = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2", params![s, limit as i64]),
            None => self.proposals_where("?1 = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2", params!["", limit as i64]),
        }
    }

    fn proposals_where(&self, clause: &str, p: impl rusqlite::Params) -> Result<Vec<ProposalRow>, MetaError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, stream, head, title, description, author, state, task_id, created_at, updated_at, integrated_as
             FROM proposals WHERE {clause}"
        ))?;
        let rows = stmt.query_map(p, |r| {
            Ok((
                r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?,
                r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, Option<String>>(7)?,
                r.get::<_, i64>(8)?, r.get::<_, i64>(9)?, r.get::<_, Option<String>>(10)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, stream, head, title, description, author, state, task_id, c, u, integ) = r?;
            out.push(ProposalRow {
                id,
                stream,
                head: parse(&head, "proposal head")?,
                title,
                description,
                author,
                state,
                task_id,
                created_at: ts(c),
                updated_at: ts(u),
                integrated_as: opt_cs(integ, "integrated_as")?,
            });
        }
        Ok(out)
    }

    /// Points an open proposal at a new head (earlier approvals become stale).
    pub fn proposal_set_head(&self, id: &str, head: &ChangesetId) -> Result<(), MetaError> {
        let n = self.conn.execute(
            "UPDATE proposals SET head = ?1, updated_at = ?2 WHERE id = ?3 AND state = 'open'",
            params![head.to_hex(), self.now().as_millis(), id],
        )?;
        if n == 0 {
            return Err(MetaError::NotFound(format!("open proposal '{id}'")));
        }
        Ok(())
    }

    /// Sets the proposal state (`open`, `integrated`, `closed`).
    pub fn proposal_set_state(&self, id: &str, state: &str, integrated_as: Option<&ChangesetId>) -> Result<(), MetaError> {
        let n = self.conn.execute(
            "UPDATE proposals SET state = ?1, integrated_as = COALESCE(?2, integrated_as), updated_at = ?3 WHERE id = ?4",
            params![state, integrated_as.map(|c| c.to_hex()), self.now().as_millis(), id],
        )?;
        if n == 0 {
            return Err(MetaError::NotFound(format!("proposal '{id}'")));
        }
        Ok(())
    }

    // ---- reviews ---------------------------------------------------------

    /// Records a review of a specific proposal head.
    pub fn review_add(&self, proposal: &str, reviewer: &str, decision: &str, body: &str, head: &ChangesetId) -> Result<u64, MetaError> {
        self.conn.execute(
            "INSERT INTO reviews (proposal, reviewer, decision, body, head, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![proposal, reviewer, decision, body, head.to_hex(), self.now().as_millis()],
        )?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    /// All reviews of a proposal, oldest first.
    pub fn reviews(&self, proposal: &str) -> Result<Vec<ReviewRow>, MetaError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, proposal, reviewer, decision, body, head, ts FROM reviews WHERE proposal = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([proposal], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, i64>(6)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, proposal, reviewer, decision, body, head, t) = r?;
            out.push(ReviewRow { id: id as u64, proposal, reviewer, decision, body, head: parse(&head, "review head")?, timestamp: ts(t) });
        }
        Ok(out)
    }

    // ---- attestations ----------------------------------------------------

    /// Stores a verified attestation.
    pub fn attestation_insert(&self, a: &AttestationRow) -> Result<(), MetaError> {
        self.conn.execute(
            "INSERT INTO attestations (id, tree_id, check_name, result, policy_version, environment_digest, issuer, issued_at, public_key, signature, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                a.id, a.tree_id.to_hex(), a.check_name, a.result, a.policy_version as i64, a.environment_digest,
                a.issuer, a.issued_at.as_millis(), a.public_key, a.signature, serde_json::to_string(&a.payload)?
            ],
        )?;
        Ok(())
    }

    /// Attestations about an exact tree, oldest first.
    pub fn attestations_for_tree(&self, tree: &TreeId) -> Result<Vec<AttestationRow>, MetaError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, tree_id, check_name, result, policy_version, environment_digest, issuer, issued_at, public_key, signature, payload
             FROM attestations WHERE tree_id = ?1 ORDER BY issued_at, id",
        )?;
        let rows = stmt.query_map([tree.to_hex()], |r| {
            Ok((
                r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, i64>(7)?,
                r.get::<_, String>(8)?, r.get::<_, String>(9)?, r.get::<_, String>(10)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, tree_id, check_name, result, pv, env, issuer, t, pk, sig, payload) = r?;
            out.push(AttestationRow {
                id,
                tree_id: parse(&tree_id, "tree id")?,
                check_name,
                result,
                policy_version: pv as u64,
                environment_digest: env,
                issuer,
                issued_at: ts(t),
                public_key: pk,
                signature: sig,
                payload: serde_json::from_str(&payload)?,
            });
        }
        Ok(out)
    }

    // ---- policies --------------------------------------------------------

    /// Appends a new policy version for a stream and returns its number.
    pub fn policy_put(&self, stream: &str, doc: &str, actor: &str) -> Result<PolicyRow, MetaError> {
        let version: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM policies WHERE stream = ?1",
            [stream],
            |r| r.get(0),
        )?;
        let hash = hex::encode(Sha256::digest(doc.as_bytes()));
        self.conn.execute(
            "INSERT INTO policies (stream, version, doc, doc_hash, created_at, created_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![stream, version, doc, hash, self.now().as_millis(), actor],
        )?;
        Ok(PolicyRow { stream: stream.into(), version: version as u64, doc: doc.into(), doc_hash: hash, created_at: self.now(), created_by: actor.into() })
    }

    /// The newest policy of a stream.
    pub fn policy_current(&self, stream: &str) -> Result<Option<PolicyRow>, MetaError> {
        self.policy_query("stream = ?1 ORDER BY version DESC LIMIT 1", params![stream]).map(|mut v| v.pop())
    }

    /// A specific policy version.
    pub fn policy_version(&self, stream: &str, version: u64) -> Result<Option<PolicyRow>, MetaError> {
        self.policy_query("stream = ?1 AND version = ?2", params![stream, version as i64]).map(|mut v| v.pop())
    }

    /// All versions of a stream's policy, oldest first.
    pub fn policy_history(&self, stream: &str) -> Result<Vec<PolicyRow>, MetaError> {
        self.policy_query("stream = ?1 ORDER BY version", params![stream])
    }

    fn policy_query(&self, clause: &str, p: impl rusqlite::Params) -> Result<Vec<PolicyRow>, MetaError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT stream, version, doc, doc_hash, created_at, created_by FROM policies WHERE {clause}"
        ))?;
        let rows = stmt.query_map(p, |r| {
            Ok(PolicyRow {
                stream: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                doc: r.get(2)?,
                doc_hash: r.get(3)?,
                created_at: ts(r.get(4)?),
                created_by: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // ---- pins ------------------------------------------------------------

    /// Creates or moves a retention root.
    pub fn pin_put(&self, name: &str, changeset: &ChangesetId, reason: &str) -> Result<(), MetaError> {
        self.conn.execute(
            "INSERT INTO pins (name, changeset, reason, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET changeset = excluded.changeset, reason = excluded.reason",
            params![name, changeset.to_hex(), reason, self.now().as_millis()],
        )?;
        Ok(())
    }

    /// Removes a retention root. Returns whether it existed.
    pub fn pin_delete(&self, name: &str) -> Result<bool, MetaError> {
        Ok(self.conn.execute("DELETE FROM pins WHERE name = ?1", [name])? > 0)
    }

    /// All retention roots.
    pub fn pins(&self) -> Result<Vec<PinRow>, MetaError> {
        let mut stmt = self.conn.prepare_cached("SELECT name, changeset, reason, created_at FROM pins ORDER BY name")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))?;
        let mut out = Vec::new();
        for r in rows {
            let (name, cs, reason, t) = r?;
            out.push(PinRow { name, changeset: parse(&cs, "pin changeset")?, reason, created_at: ts(t) });
        }
        Ok(out)
    }

    // ---- workspaces ------------------------------------------------------

    /// Creates a workspace.
    pub fn workspace_create(&self, name: &str, stream: &str, base: Option<&ChangesetId>, owner: &str) -> Result<WorkspaceRow, MetaError> {
        if self.workspace(name)?.is_some() {
            return Err(MetaError::AlreadyExists(format!("workspace '{name}'")));
        }
        let now = self.now().as_millis();
        self.conn.execute(
            "INSERT INTO workspaces (name, stream, base, head, owner, created_at, updated_at) VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?5)",
            params![name, stream, base.map(|b| b.to_hex()), owner, now],
        )?;
        self.workspace(name)?.ok_or_else(|| MetaError::Corrupt("workspace vanished".into()))
    }

    /// Looks up a workspace.
    pub fn workspace(&self, name: &str) -> Result<Option<WorkspaceRow>, MetaError> {
        self.workspaces_where("name = ?1", params![name]).map(|mut v| v.pop())
    }

    /// All workspaces.
    pub fn workspaces(&self) -> Result<Vec<WorkspaceRow>, MetaError> {
        self.workspaces_where("1 = 1 ORDER BY name", params![])
    }

    fn workspaces_where(&self, clause: &str, p: impl rusqlite::Params) -> Result<Vec<WorkspaceRow>, MetaError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT name, stream, base, head, owner, created_at, updated_at FROM workspaces WHERE {clause}"
        ))?;
        let rows = stmt.query_map(p, |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?, r.get::<_, i64>(5)?, r.get::<_, i64>(6)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (name, stream, base, head, owner, c, u) = r?;
            let _ = stream_name(&stream)?;
            out.push(WorkspaceRow { name, stream, base: opt_cs(base, "workspace base")?, head: opt_cs(head, "workspace head")?, owner, created_at: ts(c), updated_at: ts(u) });
        }
        Ok(out)
    }

    /// Updates a workspace's base and head.
    pub fn workspace_update(&self, name: &str, base: Option<&ChangesetId>, head: Option<&ChangesetId>) -> Result<(), MetaError> {
        let n = self.conn.execute(
            "UPDATE workspaces SET base = ?1, head = ?2, updated_at = ?3 WHERE name = ?4",
            params![base.map(|b| b.to_hex()), head.map(|h| h.to_hex()), self.now().as_millis(), name],
        )?;
        if n == 0 {
            return Err(MetaError::NotFound(format!("workspace '{name}'")));
        }
        Ok(())
    }

    /// Deletes a workspace.
    pub fn workspace_delete(&self, name: &str) -> Result<bool, MetaError> {
        Ok(self.conn.execute("DELETE FROM workspaces WHERE name = ?1", [name])? > 0)
    }

    /// Heads that must stay reachable: stream heads, pins, open proposals and workspaces.
    pub fn live_roots(&self) -> Result<Vec<ChangesetId>, MetaError> {
        let mut roots = Vec::new();
        for sql in [
            "SELECT head FROM streams WHERE head IS NOT NULL",
            "SELECT changeset FROM pins",
            "SELECT head FROM proposals WHERE state = 'open'",
            "SELECT head FROM workspaces WHERE head IS NOT NULL",
            "SELECT base FROM workspaces WHERE base IS NOT NULL",
        ] {
            let mut stmt = self.conn.prepare(sql)?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for r in rows {
                roots.push(parse::<ChangesetId>(&r?, "root")?);
            }
        }
        roots.sort();
        roots.dedup();
        Ok(roots)
    }

    /// Looks up the provenance of one stored value by id; helper for tests.
    #[doc(hidden)]
    pub fn count_rows(&self, table: &str) -> Result<u64, MetaError> {
        const TABLES: &[&str] = &["changesets", "streams", "reflog", "intents", "outbox", "audit", "proposals", "reviews", "attestations", "pins", "workspaces", "trees", "leases", "provenance", "queue", "agents", "tasks"];
        if !TABLES.contains(&table) {
            return Err(MetaError::Invalid(format!("unknown table '{table}'")));
        }
        Ok(self.conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get::<_, i64>(0))? as u64)
    }
}
