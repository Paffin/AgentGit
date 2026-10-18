//! The asynchronous job queue (schema version 2).
//!
//! A job is a unit of deferred work: `kind` names the handler, `payload` is
//! its JSON input. Workers pull jobs with [`Tx::job_claim`], which hands out a
//! *lease*: the job is theirs until `claimed_until`, and they extend the lease
//! with [`Tx::job_heartbeat`] while they work. A worker that dies simply stops
//! heartbeating; the next claimer reaps the expired claim, putting the job back
//! in the queue (or failing it for good when its attempts are used up).
//!
//! ```text
//! queued --claim--> claimed --complete--> succeeded
//!    ^                 | \--fail(retry)--> queued   (attempts left, run_after = now + delay)
//!    |                 |  \-fail(final)--> failed    (no attempts left, or no retry requested)
//!    +-lease expired---+
//! queued | claimed --cancel--> cancelled
//! ```
//!
//! All methods are meant to run inside [`super::Meta::write`]: that
//! transaction is the only writer of the repository, which is what makes a
//! claim safe against concurrent claimers on SQLite; on PostgreSQL the claim
//! additionally selects with `FOR UPDATE SKIP LOCKED`, so it stays correct
//! even if a claimer runs outside the advisory write lock.

use ag_types::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::db::{Dialect, Param, Row, params};
use super::{MetaError, Tx, parse, ts};

/// Attempts a job gets by default ([`Tx::job_enqueue`]).
pub const DEFAULT_JOB_ATTEMPTS: u32 = 5;

/// Lifecycle of a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Waiting for a worker (and for `run_after`).
    Queued,
    /// Leased to a worker.
    Claimed,
    /// Terminal: finished successfully.
    Succeeded,
    /// Terminal: failed for good.
    Failed,
    /// Terminal: cancelled.
    Cancelled,
}

impl JobState {
    /// Stored name.
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Claimed => "claimed",
            JobState::Succeeded => "succeeded",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
        }
    }

    /// True for states that never change again.
    pub fn is_terminal(self) -> bool {
        matches!(self, JobState::Succeeded | JobState::Failed | JobState::Cancelled)
    }
}

impl std::fmt::Display for JobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for JobState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "queued" => JobState::Queued,
            "claimed" => JobState::Claimed,
            "succeeded" => JobState::Succeeded,
            "failed" => JobState::Failed,
            "cancelled" => JobState::Cancelled,
            other => return Err(format!("unknown job state '{other}'")),
        })
    }
}

/// Parameters for [`Tx::job_enqueue_with`].
pub struct NewJob<'a> {
    /// Handler name, e.g. `reindex`.
    pub kind: &'a str,
    /// Input of the handler.
    pub payload: &'a Value,
    /// Enqueueing the same key again returns the existing job (whatever its
    /// state) instead of creating another. Keys are permanent: include a nonce
    /// to run the "same" work twice.
    pub dedupe_key: Option<&'a str>,
    /// The job is not handed out before this time.
    pub run_after: Timestamp,
    /// How many times the job may be claimed (at least 1).
    pub max_attempts: u32,
}

/// A stored job.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JobRow {
    /// Identifier, `job_…`.
    pub id: String,
    /// Handler name.
    pub kind: String,
    /// Current state.
    pub state: JobState,
    /// Input of the handler.
    pub payload: Value,
    /// Output, once succeeded.
    pub result: Option<Value>,
    /// Last error (a failed attempt, or an expired lease).
    pub error: Option<String>,
    /// Idempotency key given at enqueue time.
    pub dedupe_key: Option<String>,
    /// Times the job was claimed so far.
    pub attempts: u32,
    /// Times it may be claimed in total.
    pub max_attempts: u32,
    /// Earliest time it may be claimed.
    pub run_after: Timestamp,
    /// The worker holding (or that last held) the job.
    pub claimed_by: Option<String>,
    /// When the current lease expires (claimed jobs only).
    pub claimed_until: Option<Timestamp>,
    /// Enqueue time.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
}

const JOB_COLS: &str = "id, kind, state, payload, result, error, dedupe_key, attempts, max_attempts, run_after, claimed_by, claimed_until, created_at, updated_at";

fn job_row(r: &Row) -> Result<JobRow, MetaError> {
    Ok(JobRow {
        id: r.string(0)?,
        kind: r.string(1)?,
        state: parse(r.str(2)?, "job state")?,
        payload: serde_json::from_str(r.str(3)?)?,
        result: r.opt_str(4)?.map(serde_json::from_str).transpose()?,
        error: r.opt_string(5)?,
        dedupe_key: r.opt_string(6)?,
        attempts: r.i64(7)? as u32,
        max_attempts: r.i64(8)? as u32,
        run_after: ts(r.i64(9)?),
        claimed_by: r.opt_string(10)?,
        claimed_until: r.opt_i64(11)?.map(ts),
        created_at: ts(r.i64(12)?),
        updated_at: ts(r.i64(13)?),
    })
}

/// Why a lease-holding call found nothing to act on.
fn not_owned(id: &str, worker: &str) -> MetaError {
    MetaError::NotFound(format!("job '{id}' claimed by '{worker}' (finished, cancelled, or its lease was taken over)"))
}

const LEASE_EXPIRED: &str = "the worker's lease expired before it finished or sent a heartbeat";

impl Tx<'_> {
    /// Enqueues a job with the default number of attempts. See [`NewJob`] for
    /// the idempotency contract of `dedupe_key`.
    pub fn job_enqueue(
        &self,
        kind: &str,
        payload: &Value,
        dedupe_key: Option<&str>,
        run_after: Timestamp,
    ) -> Result<JobRow, MetaError> {
        self.job_enqueue_with(&NewJob { kind, payload, dedupe_key, run_after, max_attempts: DEFAULT_JOB_ATTEMPTS })
    }

    /// Enqueues a job.
    pub fn job_enqueue_with(&self, n: &NewJob<'_>) -> Result<JobRow, MetaError> {
        if n.kind.is_empty() {
            return Err(MetaError::Invalid("a job needs a kind".into()));
        }
        if n.max_attempts == 0 {
            return Err(MetaError::Invalid("max_attempts must be at least 1".into()));
        }
        if let Some(key) = n.dedupe_key
            && let Some(existing) = self.job_by_key(key)?
        {
            return Ok(existing);
        }
        // The id starts with a per-repository counter, so ids sort in enqueue order:
        // jobs with the same `run_after` and `created_at` are still handed out first-in first-out.
        let seq = self.next_counter("jobs")?;
        let nonce = ag_crypto::random_bytes::<4>().map_err(|e| MetaError::Invalid(format!("no randomness: {e}")))?;
        let id = format!("job_{seq:012x}_{}", hex::encode(nonce));
        let now = self.now().as_millis();
        // ON CONFLICT: two enqueuers racing on one key (possible outside the write lock)
        // both end up with the single stored job.
        self.conn.execute(
            "INSERT INTO jobs (id, kind, state, payload, dedupe_key, attempts, max_attempts, run_after, created_at, updated_at)
             VALUES (?1, ?2, 'queued', ?3, ?4, 0, ?5, ?6, ?7, ?7)
             ON CONFLICT (dedupe_key) DO NOTHING",
            &params![&id, n.kind, serde_json::to_string(n.payload)?, n.dedupe_key, n.max_attempts as i64, n.run_after.as_millis(), now],
        )?;
        match n.dedupe_key {
            Some(key) => self.job_by_key(key)?,
            None => self.job_get(&id)?,
        }
        .ok_or_else(|| MetaError::Corrupt("job vanished".into()))
    }

    /// Looks up a job.
    pub fn job_get(&self, id: &str) -> Result<Option<JobRow>, MetaError> {
        self.jobs_where("id = ?1", &params![id]).map(|mut v| v.pop())
    }

    /// The job enqueued under `dedupe_key`, in whatever state it is in (an indexed lookup).
    pub fn job_by_key(&self, key: &str) -> Result<Option<JobRow>, MetaError> {
        self.jobs_where("dedupe_key = ?1", &params![key]).map(|mut v| v.pop())
    }

    /// How many jobs are in `state` (a count, not a scan of the rows).
    pub fn job_count(&self, state: JobState) -> Result<u64, MetaError> {
        let rows = self.conn.query("SELECT COUNT(*) FROM jobs WHERE state = ?1", &params![state.as_str()])?;
        Ok(rows.first().map(|r| r.i64(0)).transpose()?.unwrap_or(0).max(0) as u64)
    }

    /// Jobs in `state` (or all), newest first.
    pub fn job_list(&self, state: Option<JobState>, limit: usize) -> Result<Vec<JobRow>, MetaError> {
        match state {
            Some(s) => self.jobs_where(
                "state = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2",
                &params![s.as_str(), limit as i64],
            ),
            None => self.jobs_where("1 = 1 ORDER BY created_at DESC, id DESC LIMIT ?1", &params![limit as i64]),
        }
    }

    fn jobs_where(&self, clause: &str, p: &[Param<'_>]) -> Result<Vec<JobRow>, MetaError> {
        let rows = self.conn.query(&format!("SELECT {JOB_COLS} FROM jobs WHERE {clause}"), p)?;
        rows.iter().map(job_row).collect()
    }

    /// Takes back the jobs whose lease ran out: a job with attempts left goes back to the queue, one without
    /// fails for good. Returns the jobs reaped (in their new state), so a caller can record that they happened -
    /// they are nobody's request, and the worker that held them is gone. [`Tx::job_claim`] does this first.
    pub fn job_reap_expired(&self) -> Result<Vec<JobRow>, MetaError> {
        let now = self.now().as_millis();
        // RETURNING names exactly the rows this statement changed, so two claimers racing on one expired job
        // cannot both report it.
        let failed = self.conn.query(
            "UPDATE jobs SET state = 'failed', error = ?2, claimed_until = NULL, updated_at = ?1
             WHERE state = 'claimed' AND claimed_until < ?1 AND attempts >= max_attempts RETURNING id",
            &params![now, LEASE_EXPIRED],
        )?;
        let requeued = self.conn.query(
            "UPDATE jobs SET state = 'queued', error = ?2, claimed_by = NULL, claimed_until = NULL, updated_at = ?1
             WHERE state = 'claimed' AND claimed_until < ?1 RETURNING id",
            &params![now, LEASE_EXPIRED],
        )?;
        let mut out = Vec::with_capacity(failed.len() + requeued.len());
        for row in failed.iter().chain(requeued.iter()) {
            let id = row.string(0)?;
            out.extend(self.job_get(&id)?);
        }
        Ok(out)
    }

    /// Claims the next runnable job of one of `kinds` (any kind if the slice is
    /// empty) for `worker`, with a lease of `lease_ms`. Returns `None` when
    /// nothing is runnable.
    ///
    /// Expired claims are reaped first: a job whose lease ran out goes back to
    /// the queue, or fails permanently if it had no attempts left. The claim
    /// counts as an attempt.
    pub fn job_claim(&self, kinds: &[&str], worker: &str, lease_ms: i64) -> Result<Option<JobRow>, MetaError> {
        if worker.is_empty() {
            return Err(MetaError::Invalid("a worker needs a name".into()));
        }
        if lease_ms <= 0 {
            return Err(MetaError::Invalid("lease_ms must be positive".into()));
        }
        let now = self.now().as_millis();
        self.job_reap_expired()?;

        let mut args: Vec<Param<'_>> = vec![Param::Int(now)];
        let mut filter = String::new();
        if !kinds.is_empty() {
            let marks: Vec<String> = (0..kinds.len()).map(|i| format!("?{}", i + 2)).collect();
            filter = format!(" AND kind IN ({})", marks.join(", "));
            args.extend(kinds.iter().map(|k| Param::from(*k)));
        }
        // Another claimer holding a row lock is skipped, not waited for.
        let skip_locked = if self.conn.dialect() == Dialect::Postgres { " FOR UPDATE SKIP LOCKED" } else { "" };
        let candidate = self.conn.query_opt(
            &format!(
                "SELECT id FROM jobs WHERE state = 'queued' AND run_after <= ?1{filter}
                 ORDER BY run_after, created_at, id LIMIT 1{skip_locked}"
            ),
            &args,
        )?;
        let Some(candidate) = candidate else { return Ok(None) };
        let id = candidate.string(0)?;
        let n = self.conn.execute(
            "UPDATE jobs SET state = 'claimed', claimed_by = ?1, claimed_until = ?2, attempts = attempts + 1, updated_at = ?3
             WHERE id = ?4 AND state = 'queued'",
            &params![worker, now.saturating_add(lease_ms), now, &id],
        )?;
        if n != 1 {
            return Ok(None);
        }
        self.job_get(&id)
    }

    /// Extends the lease of a job `worker` holds. Fails with
    /// [`MetaError::NotFound`] if the worker no longer owns it (cancelled, or
    /// reaped and claimed by someone else): the worker should stop.
    pub fn job_heartbeat(&self, id: &str, worker: &str, lease_ms: i64) -> Result<JobRow, MetaError> {
        if lease_ms <= 0 {
            return Err(MetaError::Invalid("lease_ms must be positive".into()));
        }
        let now = self.now().as_millis();
        let n = self.conn.execute(
            "UPDATE jobs SET claimed_until = ?1, updated_at = ?2 WHERE id = ?3 AND state = 'claimed' AND claimed_by = ?4",
            &params![now.saturating_add(lease_ms), now, id, worker],
        )?;
        if n == 0 {
            return Err(not_owned(id, worker));
        }
        self.job_get(id)?.ok_or_else(|| MetaError::Corrupt("job vanished".into()))
    }

    /// Marks a job `worker` holds as succeeded and stores its result.
    pub fn job_complete(&self, id: &str, worker: &str, result: &Value) -> Result<JobRow, MetaError> {
        let n = self.conn.execute(
            "UPDATE jobs SET state = 'succeeded', result = ?1, claimed_until = NULL, updated_at = ?2
             WHERE id = ?3 AND state = 'claimed' AND claimed_by = ?4",
            &params![serde_json::to_string(result)?, self.now().as_millis(), id, worker],
        )?;
        if n == 0 {
            return Err(not_owned(id, worker));
        }
        self.job_get(id)?.ok_or_else(|| MetaError::Corrupt("job vanished".into()))
    }

    /// Records a failed attempt of a job `worker` holds.
    ///
    /// With `retry_after = Some(ms)` and attempts left, the job is queued again
    /// and may not run before `now + ms`. With `None`, or when the attempts are
    /// used up, it fails for good (`None` is how a worker says "retrying is
    /// pointless").
    pub fn job_fail(&self, id: &str, worker: &str, error: &str, retry_after: Option<i64>) -> Result<JobRow, MetaError> {
        let job = self.job_get(id)?.ok_or_else(|| MetaError::NotFound(format!("job '{id}'")))?;
        if job.state != JobState::Claimed || job.claimed_by.as_deref() != Some(worker) {
            return Err(not_owned(id, worker));
        }
        let now = self.now().as_millis();
        let n = match retry_after {
            Some(delay) if job.attempts < job.max_attempts => self.conn.execute(
                "UPDATE jobs SET state = 'queued', error = ?1, run_after = ?2, claimed_by = NULL, claimed_until = NULL, updated_at = ?3
                 WHERE id = ?4 AND state = 'claimed' AND claimed_by = ?5",
                &params![error, now.saturating_add(delay.max(0)), now, id, worker],
            )?,
            _ => self.conn.execute(
                "UPDATE jobs SET state = 'failed', error = ?1, claimed_until = NULL, updated_at = ?2
                 WHERE id = ?3 AND state = 'claimed' AND claimed_by = ?4",
                &params![error, now, id, worker],
            )?,
        };
        if n == 0 {
            return Err(not_owned(id, worker));
        }
        self.job_get(id)?.ok_or_else(|| MetaError::Corrupt("job vanished".into()))
    }

    /// Cancels a queued or claimed job. A worker still holding it finds out
    /// when its next heartbeat or completion fails.
    pub fn job_cancel(&self, id: &str) -> Result<JobRow, MetaError> {
        let job = self.job_get(id)?.ok_or_else(|| MetaError::NotFound(format!("job '{id}'")))?;
        if job.state.is_terminal() {
            return Err(MetaError::InvalidTransition {
                from: job.state.to_string(),
                to: JobState::Cancelled.to_string(),
            });
        }
        self.conn.execute(
            "UPDATE jobs SET state = 'cancelled', claimed_until = NULL, updated_at = ?1 WHERE id = ?2",
            &params![self.now().as_millis(), id],
        )?;
        self.job_get(id)?.ok_or_else(|| MetaError::Corrupt("job vanished".into()))
    }
}
