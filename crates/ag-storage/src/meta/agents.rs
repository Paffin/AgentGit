use ag_types::{ChangesetId, Timestamp};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

use super::{MetaError, Tx, parse, ts};

/// A registered agent.
#[derive(Clone, Debug, Serialize)]
pub struct AgentRow {
    /// Short name (unique).
    pub name: String,
    /// Identity string, `agent:<name>`.
    pub identity: String,
    /// Ed25519 public key (hex) used to verify provenance signatures.
    pub public_key: String,
    /// Role, e.g. `agent`, `reviewer`.
    pub role: String,
    /// Registration time.
    pub created_at: Timestamp,
    /// Disabled agents cannot start runs.
    pub disabled: bool,
}

/// One execution of an agent.
#[derive(Clone, Debug, Serialize)]
pub struct RunRow {
    /// Run id, e.g. `run_…`.
    pub run_id: String,
    /// Agent name.
    pub agent: String,
    /// Task the run works on.
    pub task_id: Option<String>,
    /// Start time.
    pub started_at: Timestamp,
    /// Finish time.
    pub finished_at: Option<Timestamp>,
    /// `running`, `completed`, `failed`, `abandoned`.
    pub status: String,
    /// Last heartbeat.
    pub last_heartbeat: Timestamp,
    /// Outcome summary.
    pub summary: Option<String>,
}

/// A unit of work.
#[derive(Clone, Debug, Serialize)]
pub struct TaskRow {
    /// Identifier, e.g. `task_…`.
    pub id: String,
    /// Title.
    pub title: String,
    /// Description.
    pub description: String,
    /// Owner (agent or user identity).
    pub owner: Option<String>,
    /// `open`, `in_progress`, `done`, `cancelled`.
    pub state: String,
    /// Path globs the task is expected to touch.
    pub scope: Vec<String>,
    /// Creator.
    pub created_by: String,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last update.
    pub updated_at: Timestamp,
}

/// A lease on a scope (path glob).
#[derive(Clone, Debug, Serialize)]
pub struct LeaseRow {
    /// Identifier, e.g. `lease_…`.
    pub id: String,
    /// Path glob.
    pub scope: String,
    /// Holder identity.
    pub holder: String,
    /// Exclusive leases are enforced with fencing tokens; soft leases are advisory.
    pub exclusive: bool,
    /// Fencing token (strictly increasing across all leases).
    pub token: u64,
    /// When acquired.
    pub acquired_at: Timestamp,
    /// When it expires.
    pub expires_at: Timestamp,
    /// When released early.
    pub released_at: Option<Timestamp>,
    /// Task the lease belongs to.
    pub task_id: Option<String>,
}

impl LeaseRow {
    /// True if the lease is neither released nor expired at `now`.
    pub fn is_active(&self, now: Timestamp) -> bool {
        self.released_at.is_none() && self.expires_at > now
    }
}

/// A signed provenance record.
#[derive(Clone, Debug, Serialize)]
pub struct ProvenanceRow {
    /// Row id.
    pub id: u64,
    /// Changeset the record describes.
    pub changeset: ChangesetId,
    /// Agent name.
    pub agent: String,
    /// Run id.
    pub run_id: Option<String>,
    /// The signed record (JSON).
    pub record: Value,
    /// Signer public key (hex).
    pub public_key: String,
    /// Signature (hex).
    pub signature: String,
    /// When stored.
    pub created_at: Timestamp,
}

/// An entry of the integration queue.
#[derive(Clone, Debug, Serialize)]
pub struct QueueRow {
    /// Queue position id.
    pub id: u64,
    /// Target stream.
    pub stream: String,
    /// Proposal to integrate.
    pub proposal: String,
    /// `queued`, `done`, `blocked`, `rejected`, `failed`.
    pub state: String,
    /// Who enqueued it.
    pub enqueued_by: String,
    /// When.
    pub enqueued_at: Timestamp,
    /// When processed.
    pub processed_at: Option<Timestamp>,
    /// Outcome text.
    pub outcome: Option<String>,
}

impl Tx<'_> {
    // ---- agents ----------------------------------------------------------

    /// Registers an agent.
    pub fn agent_register(&self, name: &str, identity: &str, public_key: &str, role: &str) -> Result<AgentRow, MetaError> {
        if self.agent(name)?.is_some() {
            return Err(MetaError::AlreadyExists(format!("agent '{name}'")));
        }
        self.conn.execute(
            "INSERT INTO agents (name, identity, public_key, role, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![name, identity, public_key, role, self.now().as_millis()],
        )?;
        self.agent(name)?.ok_or_else(|| MetaError::Corrupt("agent vanished".into()))
    }

    /// Looks up an agent by name.
    pub fn agent(&self, name: &str) -> Result<Option<AgentRow>, MetaError> {
        Ok(self.agents_where("name = ?1", params![name])?.pop())
    }

    /// Looks up an agent by identity string (a `#run` suffix is ignored).
    pub fn agent_by_identity(&self, identity: &str) -> Result<Option<AgentRow>, MetaError> {
        let principal = identity.split('#').next().unwrap_or(identity);
        Ok(self.agents_where("identity = ?1", params![principal])?.pop())
    }

    /// All agents.
    pub fn agents(&self) -> Result<Vec<AgentRow>, MetaError> {
        self.agents_where("1 = 1 ORDER BY name", params![])
    }

    fn agents_where(&self, clause: &str, p: impl rusqlite::Params) -> Result<Vec<AgentRow>, MetaError> {
        let mut stmt = self.conn.prepare(&format!("SELECT name, identity, public_key, role, created_at, disabled FROM agents WHERE {clause}"))?;
        let rows = stmt.query_map(p, |r| {
            Ok(AgentRow {
                name: r.get(0)?,
                identity: r.get(1)?,
                public_key: r.get(2)?,
                role: r.get(3)?,
                created_at: ts(r.get(4)?),
                disabled: r.get::<_, i64>(5)? != 0,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Disables or re-enables an agent.
    pub fn agent_set_disabled(&self, name: &str, disabled: bool) -> Result<(), MetaError> {
        let n = self.conn.execute("UPDATE agents SET disabled = ?1 WHERE name = ?2", params![i64::from(disabled), name])?;
        if n == 0 {
            return Err(MetaError::NotFound(format!("agent '{name}'")));
        }
        Ok(())
    }

    // ---- runs ------------------------------------------------------------

    /// Starts a run.
    pub fn run_start(&self, run_id: &str, agent: &str, task_id: Option<&str>) -> Result<RunRow, MetaError> {
        let now = self.now().as_millis();
        self.conn.execute(
            "INSERT INTO agent_runs (run_id, agent, task_id, started_at, status, last_heartbeat) VALUES (?1, ?2, ?3, ?4, 'running', ?4)",
            params![run_id, agent, task_id, now],
        )?;
        self.run(run_id)?.ok_or_else(|| MetaError::Corrupt("run vanished".into()))
    }

    /// Looks up a run.
    pub fn run(&self, run_id: &str) -> Result<Option<RunRow>, MetaError> {
        Ok(self.runs_where("run_id = ?1", params![run_id])?.pop())
    }

    /// Runs of an agent (or all), newest first.
    pub fn runs(&self, agent: Option<&str>, limit: usize) -> Result<Vec<RunRow>, MetaError> {
        match agent {
            Some(a) => self.runs_where("agent = ?1 ORDER BY started_at DESC, run_id DESC LIMIT ?2", params![a, limit as i64]),
            None => self.runs_where("?1 = ?1 ORDER BY started_at DESC, run_id DESC LIMIT ?2", params!["", limit as i64]),
        }
    }

    /// Running runs whose last heartbeat is older than `before`.
    pub fn runs_stale(&self, before: Timestamp) -> Result<Vec<RunRow>, MetaError> {
        self.runs_where("status = 'running' AND last_heartbeat < ?1 ORDER BY started_at", params![before.as_millis()])
    }

    fn runs_where(&self, clause: &str, p: impl rusqlite::Params) -> Result<Vec<RunRow>, MetaError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT run_id, agent, task_id, started_at, finished_at, status, last_heartbeat, summary FROM agent_runs WHERE {clause}"
        ))?;
        let rows = stmt.query_map(p, |r| {
            Ok(RunRow {
                run_id: r.get(0)?,
                agent: r.get(1)?,
                task_id: r.get(2)?,
                started_at: ts(r.get(3)?),
                finished_at: r.get::<_, Option<i64>>(4)?.map(ts),
                status: r.get(5)?,
                last_heartbeat: ts(r.get(6)?),
                summary: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Records a heartbeat for a running run.
    pub fn run_heartbeat(&self, run_id: &str) -> Result<(), MetaError> {
        let n = self.conn.execute(
            "UPDATE agent_runs SET last_heartbeat = ?1 WHERE run_id = ?2 AND status = 'running'",
            params![self.now().as_millis(), run_id],
        )?;
        if n == 0 {
            return Err(MetaError::NotFound(format!("running run '{run_id}'")));
        }
        Ok(())
    }

    /// Finishes a run with a terminal status.
    pub fn run_finish(&self, run_id: &str, status: &str, summary: Option<&str>) -> Result<(), MetaError> {
        let n = self.conn.execute(
            "UPDATE agent_runs SET status = ?1, summary = ?2, finished_at = ?3 WHERE run_id = ?4 AND status = 'running'",
            params![status, summary, self.now().as_millis(), run_id],
        )?;
        if n == 0 {
            return Err(MetaError::NotFound(format!("running run '{run_id}'")));
        }
        Ok(())
    }

    // ---- tasks -----------------------------------------------------------

    /// Creates a task.
    pub fn task_create(&self, id: &str, title: &str, description: &str, scope: &[String], created_by: &str) -> Result<TaskRow, MetaError> {
        let now = self.now().as_millis();
        self.conn.execute(
            "INSERT INTO tasks (id, title, description, state, scope, created_by, created_at, updated_at) VALUES (?1, ?2, ?3, 'open', ?4, ?5, ?6, ?6)",
            params![id, title, description, serde_json::to_string(scope)?, created_by, now],
        )?;
        self.task(id)?.ok_or_else(|| MetaError::Corrupt("task vanished".into()))
    }

    /// Looks up a task.
    pub fn task(&self, id: &str) -> Result<Option<TaskRow>, MetaError> {
        Ok(self.tasks_where("id = ?1", params![id])?.pop())
    }

    /// Tasks in `state` (or all), oldest first.
    pub fn tasks(&self, state: Option<&str>) -> Result<Vec<TaskRow>, MetaError> {
        match state {
            Some(s) => self.tasks_where("state = ?1 ORDER BY created_at, id", params![s]),
            None => self.tasks_where("?1 = ?1 ORDER BY created_at, id", params![""]),
        }
    }

    fn tasks_where(&self, clause: &str, p: impl rusqlite::Params) -> Result<Vec<TaskRow>, MetaError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, title, description, owner, state, scope, created_by, created_at, updated_at FROM tasks WHERE {clause}"
        ))?;
        let rows = stmt.query_map(p, |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, i64>(7)?, r.get::<_, i64>(8)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, title, description, owner, state, scope, created_by, c, u) = r?;
            out.push(TaskRow { id, title, description, owner, state, scope: serde_json::from_str(&scope)?, created_by, created_at: ts(c), updated_at: ts(u) });
        }
        Ok(out)
    }

    /// Sets the owner and/or state of a task.
    pub fn task_update(&self, id: &str, owner: Option<&str>, state: Option<&str>) -> Result<(), MetaError> {
        let n = self.conn.execute(
            "UPDATE tasks SET owner = COALESCE(?1, owner), state = COALESCE(?2, state), updated_at = ?3 WHERE id = ?4",
            params![owner, state, self.now().as_millis(), id],
        )?;
        if n == 0 {
            return Err(MetaError::NotFound(format!("task '{id}'")));
        }
        Ok(())
    }

    /// Adds a dependency edge `task -> depends_on` (cycle checking is the caller's job).
    pub fn task_dep_add(&self, task: &str, depends_on: &str) -> Result<(), MetaError> {
        self.conn.execute("INSERT OR IGNORE INTO task_deps (task, depends_on) VALUES (?1, ?2)", params![task, depends_on])?;
        Ok(())
    }

    /// Direct dependencies of a task.
    pub fn task_deps(&self, task: &str) -> Result<Vec<String>, MetaError> {
        let mut stmt = self.conn.prepare_cached("SELECT depends_on FROM task_deps WHERE task = ?1 ORDER BY depends_on")?;
        let rows = stmt.query_map([task], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // ---- leases & counters ----------------------------------------------

    /// Returns the next value of a named monotonic counter (starting at 1).
    pub fn next_counter(&self, name: &str) -> Result<u64, MetaError> {
        self.conn.execute(
            "INSERT INTO counters (name, value) VALUES (?1, 1) ON CONFLICT(name) DO UPDATE SET value = value + 1",
            [name],
        )?;
        Ok(self.conn.query_row("SELECT value FROM counters WHERE name = ?1", [name], |r| r.get::<_, i64>(0))? as u64)
    }

    /// Inserts a lease.
    pub fn lease_insert(&self, l: &LeaseRow) -> Result<(), MetaError> {
        self.conn.execute(
            "INSERT INTO leases (id, scope, holder, exclusive, token, acquired_at, expires_at, released_at, task_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                l.id, l.scope, l.holder, i64::from(l.exclusive), l.token as i64, l.acquired_at.as_millis(),
                l.expires_at.as_millis(), l.released_at.map(|t| t.as_millis()), l.task_id
            ],
        )?;
        Ok(())
    }

    /// Looks up a lease.
    pub fn lease(&self, id: &str) -> Result<Option<LeaseRow>, MetaError> {
        Ok(self.leases_where("id = ?1", params![id])?.pop())
    }

    /// Leases that are not released and not expired at the transaction time.
    pub fn leases_active(&self) -> Result<Vec<LeaseRow>, MetaError> {
        self.leases_where("released_at IS NULL AND expires_at > ?1 ORDER BY token", params![self.now().as_millis()])
    }

    /// Every lease ever granted, newest first.
    pub fn leases_all(&self, limit: usize) -> Result<Vec<LeaseRow>, MetaError> {
        self.leases_where("?1 = ?1 ORDER BY token DESC LIMIT ?2", params!["", limit as i64])
    }

    fn leases_where(&self, clause: &str, p: impl rusqlite::Params) -> Result<Vec<LeaseRow>, MetaError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, scope, holder, exclusive, token, acquired_at, expires_at, released_at, task_id FROM leases WHERE {clause}"
        ))?;
        let rows = stmt.query_map(p, |r| {
            Ok(LeaseRow {
                id: r.get(0)?,
                scope: r.get(1)?,
                holder: r.get(2)?,
                exclusive: r.get::<_, i64>(3)? != 0,
                token: r.get::<_, i64>(4)? as u64,
                acquired_at: ts(r.get(5)?),
                expires_at: ts(r.get(6)?),
                released_at: r.get::<_, Option<i64>>(7)?.map(ts),
                task_id: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Releases a lease early.
    pub fn lease_release(&self, id: &str) -> Result<(), MetaError> {
        let n = self.conn.execute(
            "UPDATE leases SET released_at = ?1 WHERE id = ?2 AND released_at IS NULL",
            params![self.now().as_millis(), id],
        )?;
        if n == 0 {
            return Err(MetaError::NotFound(format!("active lease '{id}'")));
        }
        Ok(())
    }

    /// Extends an active lease's expiry.
    pub fn lease_extend(&self, id: &str, expires_at: Timestamp) -> Result<(), MetaError> {
        let n = self.conn.execute(
            "UPDATE leases SET expires_at = ?1 WHERE id = ?2 AND released_at IS NULL AND expires_at > ?3",
            params![expires_at.as_millis(), id, self.now().as_millis()],
        )?;
        if n == 0 {
            return Err(MetaError::NotFound(format!("active lease '{id}'")));
        }
        Ok(())
    }

    // ---- provenance ------------------------------------------------------

    /// Stores a signed provenance record.
    pub fn provenance_insert(&self, changeset: &ChangesetId, agent: &str, run_id: Option<&str>, record: &Value, public_key: &str, signature: &str) -> Result<u64, MetaError> {
        self.conn.execute(
            "INSERT INTO provenance (changeset, agent, run_id, record, public_key, signature, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![changeset.to_hex(), agent, run_id, serde_json::to_string(record)?, public_key, signature, self.now().as_millis()],
        )?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    /// Provenance records of a changeset, oldest first.
    pub fn provenance_for(&self, changeset: &ChangesetId) -> Result<Vec<ProvenanceRow>, MetaError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, changeset, agent, run_id, record, public_key, signature, created_at FROM provenance WHERE changeset = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([changeset.to_hex()], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, i64>(7)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, cs, agent, run_id, record, pk, sig, t) = r?;
            out.push(ProvenanceRow { id: id as u64, changeset: parse(&cs, "provenance changeset")?, agent, run_id, record: serde_json::from_str(&record)?, public_key: pk, signature: sig, created_at: ts(t) });
        }
        Ok(out)
    }

    // ---- integration queue ----------------------------------------------

    /// Appends a proposal to the stream's integration queue.
    pub fn queue_push(&self, stream: &str, proposal: &str, by: &str) -> Result<QueueRow, MetaError> {
        let dup: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM queue WHERE stream = ?1 AND proposal = ?2 AND state = 'queued'",
            params![stream, proposal],
            |r| r.get(0),
        )?;
        if dup > 0 {
            return Err(MetaError::AlreadyExists(format!("queued proposal '{proposal}'")));
        }
        self.conn.execute(
            "INSERT INTO queue (stream, proposal, state, enqueued_by, enqueued_at) VALUES (?1, ?2, 'queued', ?3, ?4)",
            params![stream, proposal, by, self.now().as_millis()],
        )?;
        let id = self.conn.last_insert_rowid() as u64;
        self.queue_entry(id)?.ok_or_else(|| MetaError::Corrupt("queue entry vanished".into()))
    }

    /// One queue entry.
    pub fn queue_entry(&self, id: u64) -> Result<Option<QueueRow>, MetaError> {
        Ok(self.queue_where("id = ?1", params![id as i64])?.pop())
    }

    /// Entries of a stream in order, optionally only those in `state`.
    pub fn queue_list(&self, stream: &str, state: Option<&str>) -> Result<Vec<QueueRow>, MetaError> {
        match state {
            Some(s) => self.queue_where("stream = ?1 AND state = ?2 ORDER BY id", params![stream, s]),
            None => self.queue_where("stream = ?1 AND ?2 = ?2 ORDER BY id", params![stream, ""]),
        }
    }

    fn queue_where(&self, clause: &str, p: impl rusqlite::Params) -> Result<Vec<QueueRow>, MetaError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, stream, proposal, state, enqueued_by, enqueued_at, processed_at, outcome FROM queue WHERE {clause}"
        ))?;
        let rows = stmt.query_map(p, |r| {
            Ok(QueueRow {
                id: r.get::<_, i64>(0)? as u64,
                stream: r.get(1)?,
                proposal: r.get(2)?,
                state: r.get(3)?,
                enqueued_by: r.get(4)?,
                enqueued_at: ts(r.get(5)?),
                processed_at: r.get::<_, Option<i64>>(6)?.map(ts),
                outcome: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Marks a queue entry processed.
    pub fn queue_finish(&self, id: u64, state: &str, outcome: &str) -> Result<(), MetaError> {
        let n = self.conn.execute(
            "UPDATE queue SET state = ?1, outcome = ?2, processed_at = ?3 WHERE id = ?4",
            params![state, outcome, self.now().as_millis(), id as i64],
        )?;
        if n == 0 {
            return Err(MetaError::NotFound(format!("queue entry {id}")));
        }
        Ok(())
    }

    /// Reads a single integer from `agent_runs`/`tasks`; kept for symmetry in tests.
    #[doc(hidden)]
    pub fn exists(&self, sql: &str) -> Result<bool, MetaError> {
        Ok(self.conn.query_row(sql, [], |_| Ok(())).optional()?.is_some())
    }
}
