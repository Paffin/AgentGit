use ag_types::{ChangesetId, Timestamp, TreeId};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::db::{Param, Row, params};
use super::{MetaError, Tx, escape_like, parse, stream_name, ts};

/// Parameters for creating a proposal.
pub struct NewProposal<'a> {
    /// Identifier, e.g. `prop_…`.
    pub id: &'a str,
    /// Target stream.
    pub stream: &'a str,
    /// Proposed head.
    pub head: &'a ChangesetId,
    /// Title.
    pub title: &'a str,
    /// Description.
    pub description: &'a str,
    /// Author identity.
    pub author: &'a str,
    /// Task the proposal belongs to.
    pub task_id: Option<&'a str>,
}

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

const PROPOSAL_COLS: &str =
    "id, stream, head, title, description, author, state, task_id, created_at, updated_at, integrated_as";
const ATTESTATION_COLS: &str = "id, tree_id, check_name, result, policy_version, environment_digest, issuer, issued_at, public_key, signature, payload";
const POLICY_COLS: &str = "stream, version, doc, doc_hash, created_at, created_by";
const WORKSPACE_COLS: &str = "name, stream, base, head, owner, created_at, updated_at";

fn proposal_row(r: &Row) -> Result<ProposalRow, MetaError> {
    Ok(ProposalRow {
        id: r.string(0)?,
        stream: r.string(1)?,
        head: parse(r.str(2)?, "proposal head")?,
        title: r.string(3)?,
        description: r.string(4)?,
        author: r.string(5)?,
        state: r.string(6)?,
        task_id: r.opt_string(7)?,
        created_at: ts(r.i64(8)?),
        updated_at: ts(r.i64(9)?),
        integrated_as: opt_cs(r.opt_string(10)?, "integrated_as")?,
    })
}

fn policy_row(r: &Row) -> Result<PolicyRow, MetaError> {
    Ok(PolicyRow {
        stream: r.string(0)?,
        version: r.i64(1)? as u64,
        doc: r.string(2)?,
        doc_hash: r.string(3)?,
        created_at: ts(r.i64(4)?),
        created_by: r.string(5)?,
    })
}

fn workspace_row(r: &Row) -> Result<WorkspaceRow, MetaError> {
    let stream = r.string(1)?;
    let _ = stream_name(&stream)?;
    Ok(WorkspaceRow {
        name: r.string(0)?,
        stream,
        base: opt_cs(r.opt_string(2)?, "workspace base")?,
        head: opt_cs(r.opt_string(3)?, "workspace head")?,
        owner: r.string(4)?,
        created_at: ts(r.i64(5)?),
        updated_at: ts(r.i64(6)?),
    })
}

/// Equality filters of [`Tx::proposals_page`]; `None` matches everything.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProposalFilter<'a> {
    /// Proposal state (`open`, `integrated`, `closed`).
    pub state: Option<&'a str>,
    /// Target stream.
    pub stream: Option<&'a str>,
    /// Author identity, exactly as stored.
    pub author: Option<&'a str>,
    /// Linked task, exactly as stored.
    pub task_id: Option<&'a str>,
}

/// The `WHERE … ORDER BY … LIMIT` of [`Tx::proposals_page`] and its arguments (separate so that a test can ask the
/// database how it would run exactly this statement).
pub(crate) fn proposals_page_clause<'a>(
    filter: &ProposalFilter<'a>,
    after: Option<(i64, &'a str)>,
    limit: usize,
) -> (String, Vec<Param<'a>>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<Param<'_>> = Vec::new();
    for (column, value) in
        [("state", filter.state), ("stream", filter.stream), ("author", filter.author), ("task_id", filter.task_id)]
    {
        if let Some(v) = value {
            args.push(Param::from(v));
            clauses.push(format!("{column} = ?{}", args.len()));
        }
    }
    if let Some((created, id)) = after {
        args.push(Param::from(created));
        args.push(Param::from(id));
        clauses.push(format!("(created_at, id) < (?{}, ?{})", args.len() - 1, args.len()));
    }
    args.push(Param::from(limit as i64));
    let where_ = if clauses.is_empty() { "1 = 1".to_string() } else { clauses.join(" AND ") };
    (format!("{where_} ORDER BY created_at DESC, id DESC LIMIT ?{}", args.len()), args)
}

impl Tx<'_> {
    // ---- proposals -------------------------------------------------------

    /// Creates an open proposal.
    pub fn proposal_create(&self, p: &NewProposal<'_>) -> Result<ProposalRow, MetaError> {
        let now = self.now().as_millis();
        self.conn.execute(
            "INSERT INTO proposals (id, stream, head, title, description, author, state, task_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7, ?8, ?8)",
            &params![p.id, p.stream, p.head.to_hex(), p.title, p.description, p.author, p.task_id, now],
        )?;
        self.proposal(p.id)?.ok_or_else(|| MetaError::Corrupt("proposal vanished".into()))
    }

    /// Looks up a proposal.
    pub fn proposal(&self, id: &str) -> Result<Option<ProposalRow>, MetaError> {
        self.proposals_where("id = ?1", &params![id]).map(|mut v| v.pop())
    }

    /// Proposals whose id starts with `prefix`, at most `limit`.
    pub fn proposals_by_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<ProposalRow>, MetaError> {
        let like = format!("{}%", escape_like(prefix));
        // lower() on both sides: SQLite's LIKE is ASCII case-insensitive, PostgreSQL's is not
        self.proposals_where("lower(id) LIKE lower(?1) ESCAPE '\\' ORDER BY id LIMIT ?2", &params![like, limit as i64])
    }

    /// Proposals in `state` (or all), newest first.
    pub fn proposals_list(&self, state: Option<&str>, limit: usize) -> Result<Vec<ProposalRow>, MetaError> {
        match state {
            Some(s) => {
                self.proposals_where("state = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2", &params![s, limit as i64])
            }
            None => self.proposals_where("1 = 1 ORDER BY created_at DESC, id DESC LIMIT ?1", &params![limit as i64]),
        }
    }

    /// One page of proposals, newest first, filtered in SQL. `after` is the `(created_at_ms, id)` of the last row of
    /// the previous page (keyset paging: the cost of a page does not grow with its position, and rows created or
    /// closed between two pages cannot shift it).
    pub fn proposals_page(
        &self,
        filter: &ProposalFilter<'_>,
        after: Option<(i64, &str)>,
        limit: usize,
    ) -> Result<Vec<ProposalRow>, MetaError> {
        let (clause, args) = proposals_page_clause(filter, after, limit);
        self.proposals_where(&clause, &args)
    }

    /// Every open proposal of `stream`, newest first. No cap: a prediction that silently ignored the oldest
    /// proposals would miss overlaps.
    pub fn proposals_open_for_stream(&self, stream: &str) -> Result<Vec<ProposalRow>, MetaError> {
        self.proposals_where("state = 'open' AND stream = ?1 ORDER BY created_at DESC, id DESC", &params![stream])
    }

    /// How many proposals are in `state` (a count, not a scan of the rows).
    pub fn proposals_count(&self, state: &str) -> Result<u64, MetaError> {
        let rows = self.conn.query("SELECT COUNT(*) FROM proposals WHERE state = ?1", &params![state])?;
        Ok(rows.first().map(|r| r.i64(0)).transpose()?.unwrap_or(0).max(0) as u64)
    }

    fn proposals_where(&self, clause: &str, p: &[Param<'_>]) -> Result<Vec<ProposalRow>, MetaError> {
        let rows = self.conn.query(&format!("SELECT {PROPOSAL_COLS} FROM proposals WHERE {clause}"), p)?;
        rows.iter().map(proposal_row).collect()
    }

    /// Points an open proposal at a new head (earlier approvals become stale).
    pub fn proposal_set_head(&self, id: &str, head: &ChangesetId) -> Result<(), MetaError> {
        let n = self.conn.execute(
            "UPDATE proposals SET head = ?1, updated_at = ?2 WHERE id = ?3 AND state = 'open'",
            &params![head.to_hex(), self.now().as_millis(), id],
        )?;
        if n == 0 {
            return Err(MetaError::NotFound(format!("open proposal '{id}'")));
        }
        Ok(())
    }

    /// Sets the proposal state (`open`, `integrated`, `closed`).
    pub fn proposal_set_state(
        &self,
        id: &str,
        state: &str,
        integrated_as: Option<&ChangesetId>,
    ) -> Result<(), MetaError> {
        let n = self.conn.execute(
            "UPDATE proposals SET state = ?1, integrated_as = COALESCE(?2, integrated_as), updated_at = ?3 WHERE id = ?4",
            &params![state, integrated_as.map(|c| c.to_hex()), self.now().as_millis(), id],
        )?;
        if n == 0 {
            return Err(MetaError::NotFound(format!("proposal '{id}'")));
        }
        Ok(())
    }

    // ---- reviews ---------------------------------------------------------

    /// Records a review of a specific proposal head.
    pub fn review_add(
        &self,
        proposal: &str,
        reviewer: &str,
        decision: &str,
        body: &str,
        head: &ChangesetId,
    ) -> Result<u64, MetaError> {
        let id = self
            .conn
            .query_one(
                "INSERT INTO reviews (proposal, reviewer, decision, body, head, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 RETURNING id",
                &params![proposal, reviewer, decision, body, head.to_hex(), self.now().as_millis()],
            )?
            .i64(0)?;
        Ok(id as u64)
    }

    /// A single stored review or discussion entry.
    pub fn review(&self, id: u64) -> Result<Option<ReviewRow>, MetaError> {
        Ok(self.reviews_where("id = ?1", &params![id as i64])?.pop())
    }

    /// All reviews of a proposal, oldest first.
    pub fn reviews(&self, proposal: &str) -> Result<Vec<ReviewRow>, MetaError> {
        self.reviews_where("proposal = ?1 ORDER BY id", &params![proposal])
    }

    fn reviews_where(&self, clause: &str, args: &[Param<'_>]) -> Result<Vec<ReviewRow>, MetaError> {
        let rows = self.conn.query(
            &format!("SELECT id, proposal, reviewer, decision, body, head, ts FROM reviews WHERE {clause}"),
            args,
        )?;
        rows.iter()
            .map(|r| {
                Ok(ReviewRow {
                    id: r.i64(0)? as u64,
                    proposal: r.string(1)?,
                    reviewer: r.string(2)?,
                    decision: r.string(3)?,
                    body: r.string(4)?,
                    head: parse(r.str(5)?, "review head")?,
                    timestamp: ts(r.i64(6)?),
                })
            })
            .collect()
    }

    // ---- attestations ----------------------------------------------------

    /// Stores a verified attestation.
    pub fn attestation_insert(&self, a: &AttestationRow) -> Result<(), MetaError> {
        self.conn.execute(
            &format!(
                "INSERT INTO attestations ({ATTESTATION_COLS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
            ),
            &params![
                &a.id,
                a.tree_id.to_hex(),
                &a.check_name,
                &a.result,
                a.policy_version as i64,
                &a.environment_digest,
                &a.issuer,
                a.issued_at.as_millis(),
                &a.public_key,
                &a.signature,
                serde_json::to_string(&a.payload)?
            ],
        )?;
        Ok(())
    }

    /// Attestations about an exact tree, oldest first.
    pub fn attestations_for_tree(&self, tree: &TreeId) -> Result<Vec<AttestationRow>, MetaError> {
        let rows = self.conn.query(
            &format!("SELECT {ATTESTATION_COLS} FROM attestations WHERE tree_id = ?1 ORDER BY issued_at, id"),
            &params![tree.to_hex()],
        )?;
        rows.iter()
            .map(|r| {
                Ok(AttestationRow {
                    id: r.string(0)?,
                    tree_id: parse(r.str(1)?, "tree id")?,
                    check_name: r.string(2)?,
                    result: r.string(3)?,
                    policy_version: r.i64(4)? as u64,
                    environment_digest: r.string(5)?,
                    issuer: r.string(6)?,
                    issued_at: ts(r.i64(7)?),
                    public_key: r.string(8)?,
                    signature: r.string(9)?,
                    payload: serde_json::from_str(r.str(10)?)?,
                })
            })
            .collect()
    }

    // ---- policies --------------------------------------------------------

    /// Appends a new policy version for a stream and returns its number.
    pub fn policy_put(&self, stream: &str, doc: &str, actor: &str) -> Result<PolicyRow, MetaError> {
        let version = self
            .conn
            .query_one("SELECT COALESCE(MAX(version), 0) + 1 FROM policies WHERE stream = ?1", &params![stream])?
            .i64(0)?;
        let hash = hex::encode(Sha256::digest(doc.as_bytes()));
        self.conn.execute(
            "INSERT INTO policies (stream, version, doc, doc_hash, created_at, created_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            &params![stream, version, doc, &hash, self.now().as_millis(), actor],
        )?;
        Ok(PolicyRow {
            stream: stream.into(),
            version: version as u64,
            doc: doc.into(),
            doc_hash: hash,
            created_at: self.now(),
            created_by: actor.into(),
        })
    }

    /// The newest policy of a stream.
    pub fn policy_current(&self, stream: &str) -> Result<Option<PolicyRow>, MetaError> {
        self.policy_query("stream = ?1 ORDER BY version DESC LIMIT 1", &params![stream]).map(|mut v| v.pop())
    }

    /// A specific policy version.
    pub fn policy_version(&self, stream: &str, version: u64) -> Result<Option<PolicyRow>, MetaError> {
        self.policy_query("stream = ?1 AND version = ?2", &params![stream, version as i64]).map(|mut v| v.pop())
    }

    /// All versions of a stream's policy, oldest first.
    pub fn policy_history(&self, stream: &str) -> Result<Vec<PolicyRow>, MetaError> {
        self.policy_query("stream = ?1 ORDER BY version", &params![stream])
    }

    fn policy_query(&self, clause: &str, p: &[Param<'_>]) -> Result<Vec<PolicyRow>, MetaError> {
        let rows = self.conn.query(&format!("SELECT {POLICY_COLS} FROM policies WHERE {clause}"), p)?;
        rows.iter().map(policy_row).collect()
    }

    // ---- pins ------------------------------------------------------------

    /// Creates or moves a retention root.
    pub fn pin_put(&self, name: &str, changeset: &ChangesetId, reason: &str) -> Result<(), MetaError> {
        self.conn.execute(
            "INSERT INTO pins (name, changeset, reason, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (name) DO UPDATE SET changeset = excluded.changeset, reason = excluded.reason",
            &params![name, changeset.to_hex(), reason, self.now().as_millis()],
        )?;
        Ok(())
    }

    /// Removes a retention root. Returns whether it existed.
    pub fn pin_delete(&self, name: &str) -> Result<bool, MetaError> {
        Ok(self.conn.execute("DELETE FROM pins WHERE name = ?1", &params![name])? > 0)
    }

    /// All retention roots.
    pub fn pins(&self) -> Result<Vec<PinRow>, MetaError> {
        let rows = self.conn.query("SELECT name, changeset, reason, created_at FROM pins ORDER BY name", &[])?;
        rows.iter()
            .map(|r| {
                Ok(PinRow {
                    name: r.string(0)?,
                    changeset: parse(r.str(1)?, "pin changeset")?,
                    reason: r.string(2)?,
                    created_at: ts(r.i64(3)?),
                })
            })
            .collect()
    }

    // ---- workspaces ------------------------------------------------------

    /// Creates a workspace.
    pub fn workspace_create(
        &self,
        name: &str,
        stream: &str,
        base: Option<&ChangesetId>,
        owner: &str,
    ) -> Result<WorkspaceRow, MetaError> {
        if self.workspace(name)?.is_some() {
            return Err(MetaError::AlreadyExists(format!("workspace '{name}'")));
        }
        let now = self.now().as_millis();
        self.conn.execute(
            "INSERT INTO workspaces (name, stream, base, head, owner, created_at, updated_at) VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?5)",
            &params![name, stream, base.map(|b| b.to_hex()), owner, now],
        )?;
        self.workspace(name)?.ok_or_else(|| MetaError::Corrupt("workspace vanished".into()))
    }

    /// Looks up a workspace.
    pub fn workspace(&self, name: &str) -> Result<Option<WorkspaceRow>, MetaError> {
        self.workspaces_where("name = ?1", &params![name]).map(|mut v| v.pop())
    }

    /// All workspaces.
    pub fn workspaces(&self) -> Result<Vec<WorkspaceRow>, MetaError> {
        self.workspaces_where("1 = 1 ORDER BY name", &[])
    }

    fn workspaces_where(&self, clause: &str, p: &[Param<'_>]) -> Result<Vec<WorkspaceRow>, MetaError> {
        let rows = self.conn.query(&format!("SELECT {WORKSPACE_COLS} FROM workspaces WHERE {clause}"), p)?;
        rows.iter().map(workspace_row).collect()
    }

    /// Updates a workspace's base and head.
    pub fn workspace_update(
        &self,
        name: &str,
        base: Option<&ChangesetId>,
        head: Option<&ChangesetId>,
    ) -> Result<(), MetaError> {
        let n = self.conn.execute(
            "UPDATE workspaces SET base = ?1, head = ?2, updated_at = ?3 WHERE name = ?4",
            &params![base.map(|b| b.to_hex()), head.map(|h| h.to_hex()), self.now().as_millis(), name],
        )?;
        if n == 0 {
            return Err(MetaError::NotFound(format!("workspace '{name}'")));
        }
        Ok(())
    }

    /// Deletes a workspace.
    pub fn workspace_delete(&self, name: &str) -> Result<bool, MetaError> {
        Ok(self.conn.execute("DELETE FROM workspaces WHERE name = ?1", &params![name])? > 0)
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
            for r in self.conn.query(sql, &[])? {
                roots.push(parse::<ChangesetId>(r.str(0)?, "root")?);
            }
        }
        roots.sort();
        roots.dedup();
        Ok(roots)
    }

    /// Row count of a table; helper for tests.
    #[doc(hidden)]
    pub fn count_rows(&self, table: &str) -> Result<u64, MetaError> {
        const TABLES: &[&str] = &[
            "changesets",
            "streams",
            "reflog",
            "intents",
            "outbox",
            "audit",
            "proposals",
            "reviews",
            "attestations",
            "pins",
            "workspaces",
            "trees",
            "leases",
            "provenance",
            "queue",
            "agents",
            "tasks",
            "changeset_trees",
            "jobs",
        ];
        if !TABLES.contains(&table) {
            return Err(MetaError::Invalid(format!("unknown table '{table}'")));
        }
        Ok(self.conn.query_one(&format!("SELECT COUNT(*) FROM {table}"), &[])?.i64(0)? as u64)
    }
}
