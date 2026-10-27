//! The claim → check out → run → attest → complete loop.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ag_client::{Client, ClientError};
use ag_crypto::KeyPair;
use ag_policy::CheckResult;
use ag_protocol::{ErrorCode, v1};
use ag_sandbox::{Sandbox, SandboxError};
use ag_types::{Identity, TreeId};
use tokio::sync::{oneshot, watch};

use crate::attest;
use crate::checkout::{self, CheckoutError};
use crate::plan::{self, Unrunnable};
use crate::profile::Profile;

/// Job kind this worker understands.
const KIND_VALIDATE: &str = "validate";
/// How long the first retry of a failed claim waits; it doubles up to [`Options::backoff_max`].
const BACKOFF_START: Duration = Duration::from_secs(1);

/// Connection-independent settings of a worker.
#[derive(Clone, Debug)]
pub struct Options {
    /// The worker's own name for this loop; `[A-Za-z0-9._-]{1,64}`. With more than one lane, `-1`, `-2`, ...
    /// is appended.
    pub label: String,
    /// Only claim jobs of this repository (default: any the credential may run).
    pub repo: Option<String>,
    /// How long a claim lasts without a heartbeat.
    pub lease: Duration,
    /// How long one claim request waits for work (the server caps it at 30 s).
    pub wait: Duration,
    /// Number of jobs run at once.
    pub concurrency: usize,
    /// How long a job this worker could not run waits before the server offers it again.
    pub retry_after: Duration,
    /// The longest the wait between two failed claim requests grows to.
    pub backoff_max: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            label: "worker".into(),
            repo: None,
            lease: Duration::from_secs(120),
            wait: Duration::from_secs(20),
            concurrency: 1,
            retry_after: Duration::from_secs(10),
            backoff_max: Duration::from_secs(30),
        }
    }
}

/// What became of one job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// The check ran and a signed attestation was delivered with the completion.
    Attested {
        /// The verdict that was signed.
        result: CheckResult,
        /// The attestation's id.
        attestation: String,
    },
    /// The candidate was no longer the job's tree; the job was completed as stale, with no attestation.
    Stale,
    /// The job could not be run here and was failed on the server.
    Failed {
        /// Why.
        error: String,
        /// Whether it was returned to the queue for another attempt.
        retry: bool,
    },
    /// The result could not be delivered (the lease was lost, or the server was unreachable). The server
    /// will hand the job out again when the lease expires.
    Abandoned(String),
}

/// The record of one processed job.
#[derive(Clone, Debug)]
pub struct JobReport {
    /// The job's id.
    pub job_id: String,
    /// The check's name.
    pub check: String,
    /// What happened.
    pub disposition: Disposition,
}

/// A problem that ends a job without a verdict.
#[derive(Debug)]
struct Failure {
    message: String,
    retry: bool,
}

impl Failure {
    fn retry(m: impl Into<String>) -> Self {
        Failure { message: m.into(), retry: true }
    }
    fn permanent(m: impl Into<String>) -> Self {
        Failure { message: m.into(), retry: false }
    }
}

enum Finished {
    Verdict { attestation: Box<v1::Attestation>, result: CheckResult, result_json: serde_json::Value },
    Stale,
}

/// A worker: a connection, a sandbox profile, and a signing key.
pub struct Worker {
    client: Client,
    profile: Profile,
    key: KeyPair,
    issuer: Identity,
    opts: Options,
    /// Set by [`Worker::cancel`]: every running check is killed and every job handed back.
    cancel: Arc<AtomicBool>,
}

impl Worker {
    /// Creates a worker.
    ///
    /// `issuer` is the identity the server knows the credential as (what `Whoami` returns): the server
    /// refuses an attestation whose issuer is anyone else. `key`'s public half must be listed under that
    /// identity in the policy's `trusted_issuers` for the attestations to count.
    pub fn new(client: Client, profile: Profile, key: KeyPair, issuer: Identity, opts: Options) -> Self {
        Worker { client, profile, key, issuer, opts, cancel: Arc::new(AtomicBool::new(false)) }
    }

    /// Stop now: the checks that are running are killed (a container is killed and removed through its runtime),
    /// the jobs they belong to are returned to the queue, and [`run`](Self::run) returns once that is done.
    /// This is the second signal of `ag-worker run`; the first one only stops claiming and lets the current
    /// checks finish.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Asks the server who the credential belongs to.
    pub async fn whoami(client: &Client) -> Result<Identity, ClientError> {
        let p = client.retrying(|| async { client.protocol().whoami(client.req(v1::WhoamiRequest {})).await }).await?;
        Identity::new(p.into_inner().identity)
            .map_err(|e| ClientError::Invalid(format!("the server returned an unusable identity: {e}")))
    }

    /// The label of lane `i` (0-based) of `n`.
    fn lane_label(&self, i: usize) -> String {
        if self.opts.concurrency <= 1 { self.opts.label.clone() } else { format!("{}-{}", self.opts.label, i + 1) }
    }

    /// Runs until `shutdown` turns true (or, with `once`, until every lane has processed one job). Returns
    /// the reports of the jobs processed. Fails only for conditions that retrying cannot fix (a rejected
    /// credential).
    pub async fn run(
        self: &Arc<Self>,
        shutdown: watch::Receiver<bool>,
        once: bool,
    ) -> Result<Vec<JobReport>, ClientError> {
        let lanes = self.opts.concurrency.max(1);
        let mut tasks = Vec::new();
        for i in 0..lanes {
            let me = self.clone();
            let sd = shutdown.clone();
            tasks.push(tokio::spawn(async move { me.lane(i, sd, once).await }));
        }
        let mut reports = Vec::new();
        let mut fatal = None;
        for t in tasks {
            match t.await {
                Ok(Ok(mut r)) => reports.append(&mut r),
                Ok(Err(e)) => fatal = Some(e),
                Err(e) => tracing::error!(error = %e, "a worker lane panicked"),
            }
        }
        match fatal {
            Some(e) => Err(e),
            None => Ok(reports),
        }
    }

    async fn lane(
        &self,
        index: usize,
        mut shutdown: watch::Receiver<bool>,
        once: bool,
    ) -> Result<Vec<JobReport>, ClientError> {
        let label = self.lane_label(index);
        let mut reports = Vec::new();
        let mut backoff = BACKOFF_START;
        while !*shutdown.borrow() {
            let claim = tokio::select! {
                r = self.claim(&label) => r,
                _ = shutdown.changed() => break,
            };
            match claim {
                Ok(Some(job)) => {
                    backoff = BACKOFF_START;
                    let report = self.process(&label, job).await;
                    reports.push(report);
                    if once {
                        break;
                    }
                }
                Ok(None) => backoff = BACKOFF_START,
                Err(e) => {
                    if matches!(e.code(), Some(ErrorCode::Unauthenticated | ErrorCode::PermissionDenied)) {
                        return Err(e);
                    }
                    tracing::warn!(error = %e, retry_in = ?backoff, "claiming a job failed");
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = shutdown.changed() => break,
                    }
                    backoff = (backoff * 2).min(self.opts.backoff_max);
                }
            }
        }
        Ok(reports)
    }

    async fn claim(&self, label: &str) -> Result<Option<v1::Job>, ClientError> {
        let req = v1::ClaimJobRequest {
            repo: self.opts.repo.clone().unwrap_or_default(),
            kinds: vec![KIND_VALIDATE.into()],
            worker: label.into(),
            lease_ms: self.opts.lease.as_millis() as i64,
            wait_ms: self.opts.wait.as_millis().min(30_000) as u32,
        };
        let resp = self.client.execution().claim_job(self.client.req(req)).await.map_err(ClientError::from)?;
        Ok(resp.into_inner().job)
    }

    /// Processes one claimed job completely and reports what became of it. Never panics on bad jobs.
    pub async fn process(&self, label: &str, job: v1::Job) -> JobReport {
        let job_id = job.id.clone();
        let check = job.validation.as_ref().map(|v| v.check.clone()).unwrap_or_default();
        let disposition = self.process_inner(label, &job).await;
        match &disposition {
            Disposition::Attested { result, attestation } => {
                tracing::info!(job = %job_id, check = %check, result = result.as_str(), attestation = %attestation, "job completed")
            }
            Disposition::Stale => {
                tracing::info!(job = %job_id, check = %check, "candidate is stale; job completed without an attestation")
            }
            Disposition::Failed { error, retry } => {
                tracing::warn!(job = %job_id, check = %check, retry, error = %error, "job failed")
            }
            Disposition::Abandoned(why) => tracing::warn!(job = %job_id, check = %check, why = %why, "job abandoned"),
        }
        JobReport { job_id, check, disposition }
    }

    async fn process_inner(&self, label: &str, job: &v1::Job) -> Disposition {
        let repo = job.repo.clone();
        if job.kind != KIND_VALIDATE {
            return self.fail(&repo, job, label, Failure::permanent(format!("unknown job kind '{}'", job.kind))).await;
        }
        let Some(task) = job.validation.clone() else {
            return self.fail(&repo, job, label, Failure::permanent("the job carries no validation task")).await;
        };

        // The directory the check's tree goes into: its name must not be guessable.
        let job_dir = match self.job_dir(&job.id, job.attempts) {
            Ok(d) => d,
            Err(f) => return self.fail(&repo, job, label, f).await,
        };

        // Keep the claim alive while the check runs; learn if it is taken away.
        let lost = Arc::new(AtomicBool::new(false));
        let (stop_tx, stop_rx) = oneshot::channel();
        let beat = tokio::spawn(keep_lease(
            self.client.clone(),
            repo.clone(),
            job.clone(),
            label.to_string(),
            self.opts.lease,
            stop_rx,
            lost.clone(),
        ));

        let outcome = self.run_job(job, &task, label, &job_dir, &lost).await;
        let _ = stop_tx.send(());
        let _ = beat.await;
        remove_job_dir(&job_dir);

        if lost.load(Ordering::SeqCst) {
            return Disposition::Abandoned(
                "the claim was lost while the check ran (lease expired or job reassigned)".into(),
            );
        }
        match outcome {
            Ok(Finished::Stale) => {
                let req = v1::CompleteJobRequest {
                    repo: repo.clone(),
                    job_id: job.id.clone(),
                    worker: label.into(),
                    result_json: serde_json::json!({"stale": true}).to_string(),
                    attestation: None,
                    stale: true,
                    fencing_token: u64::from(job.attempts),
                };
                match self.complete(job, label, req).await {
                    Ok(()) => Disposition::Stale,
                    Err(e) => Disposition::Abandoned(format!("could not complete the job: {e}")),
                }
            }
            Ok(Finished::Verdict { attestation, result, result_json }) => {
                let id = attestation.id.clone();
                let req = v1::CompleteJobRequest {
                    repo: repo.clone(),
                    job_id: job.id.clone(),
                    worker: label.into(),
                    result_json: result_json.to_string(),
                    attestation: Some(*attestation),
                    stale: false,
                    fencing_token: u64::from(job.attempts),
                };
                match self.complete(job, label, req).await {
                    Ok(()) => Disposition::Attested { result, attestation: id },
                    Err(e) => {
                        // The server refused the completion (for example a policy version it no longer
                        // accepts); say so on the job rather than leaving it to time out.
                        if matches!(e.code(), Some(ErrorCode::InvalidArgument | ErrorCode::PermissionDenied)) {
                            self.fail(
                                &repo,
                                job,
                                label,
                                Failure::retry(format!("the server refused the attestation: {e}")),
                            )
                            .await
                        } else {
                            Disposition::Abandoned(format!("could not complete the job: {e}"))
                        }
                    }
                }
            }
            Err(f) => self.fail(&repo, job, label, f).await,
        }
    }

    /// Everything between claiming and having something to deliver.
    async fn run_job(
        &self,
        job: &v1::Job,
        task: &v1::ValidationTask,
        label: &str,
        job_dir: &Path,
        lost: &Arc<AtomicBool>,
    ) -> Result<Finished, Failure> {
        let tree_id: TreeId = task
            .tree_id
            .parse()
            .map_err(|_| Failure::permanent(format!("the job names an unusable tree id '{}'", task.tree_id)))?;
        let image = task.run.as_ref().map(|r| r.image.as_str()).filter(|i| !i.is_empty());

        // Decide whether this worker can and will run it before spending bandwidth.
        let sandbox: Arc<dyn Sandbox> =
            Arc::from(self.profile.backend.sandbox_for(image).map_err(|r| Failure::retry(r.to_string()))?);
        if sandbox.isolation() < self.profile.min_isolation {
            return Err(Failure::retry(format!(
                "this worker's sandbox ({}) is weaker than the isolation it requires",
                sandbox.name()
            )));
        }
        // ... and that the sandbox can run it at all (a container image is in the local store: the sandbox
        // never pulls), before the tree is downloaded for nothing.
        let probe = sandbox.clone();
        tokio::task::spawn_blocking(move || probe.preflight())
            .await
            .map_err(|e| Failure::retry(format!("the sandbox thread panicked: {e}")))?
            .map_err(|e| Failure::retry(e.to_string()))?;
        let workdir = job_dir.join("tree");
        self.create_job_dirs(job_dir, &workdir).map_err(io_failure)?;
        let spec = plan::sandbox_spec(&self.profile, task, &workdir).map_err(|u| match u {
            Unrunnable::Invalid(m) => Failure::permanent(m),
            Unrunnable::Refused(r) => Failure::retry(r.to_string()),
        })?;

        // Fetch and verify the exact tree.
        let manifest = match checkout::fetch_manifest(&self.client, &job.repo, &task.proposal, &tree_id).await {
            Ok(t) => t,
            Err(CheckoutError::Stale(why)) => {
                tracing::info!(job = %job.id, why = %why, "stale candidate");
                return Ok(Finished::Stale);
            }
            Err(e) => return Err(checkout_failure(&e)),
        };
        let owner = self.profile.backend.tree_owner();
        let written =
            checkout::write_tree(&self.client, &job.repo, &manifest, &workdir, owner, self.profile.max_checkout_bytes)
                .await
                .map_err(|e| checkout_failure(&e))?;
        tracing::debug!(job = %job.id, files = written.files, bytes = written.bytes, "tree checked out");

        // Run it. The sandbox call blocks until nothing of the command is left.
        let name = sandbox.name();
        // The check is stopped (and its container killed and removed) as soon as the claim is lost, or this
        // worker is told to stop now: nobody wants the result of a check that nobody can deliver.
        let (lost, stop) = (lost.clone(), self.cancel.clone());
        let outcome = tokio::task::spawn_blocking(move || {
            sandbox.run_cancellable(&spec, &|| lost.load(Ordering::SeqCst) || stop.load(Ordering::SeqCst))
        })
        .await
        .map_err(|e| Failure::retry(format!("the sandbox thread panicked: {e}")))?
        .map_err(|e| match e {
            SandboxError::Exec { program, source } => Failure::retry(format!(
                "cannot execute '{program}' in the sandbox on this worker: {source} (is the toolchain mounted?)"
            )),
            SandboxError::Cancelled => {
                Failure::retry("the check was stopped before it finished: this worker is shutting down")
            }
            other => Failure::retry(format!("the sandbox could not run the check: {other}")),
        })?;

        if !task.required_environment_digest.is_empty()
            && outcome.environment_digest != task.required_environment_digest
        {
            return Err(Failure::retry(format!(
                "this worker's environment digest is {}, but the policy requires {}",
                outcome.environment_digest, task.required_environment_digest
            )));
        }

        let result = attest::verdict(&outcome.status);
        let att = attest::sign(
            &self.key,
            &self.issuer,
            attest::attestation_id(&job.id, job.attempts),
            tree_id,
            &task.check,
            task.policy_version,
            &outcome.environment_digest,
            result,
        )
        .map_err(|e| Failure::retry(format!("cannot sign the attestation: {e}")))?;
        Ok(Finished::Verdict {
            attestation: Box::new(attest::to_wire(&att)),
            result,
            result_json: attest::result_json(&outcome, name, label, self.profile.tail_bytes),
        })
    }

    /// Delivers a completion. If the response was lost and the retry finds the job already finished by this
    /// worker, that counts as delivered.
    async fn complete(&self, job: &v1::Job, label: &str, req: v1::CompleteJobRequest) -> Result<(), ClientError> {
        let first = self
            .client
            .retrying(|| async { self.client.execution().complete_job(self.client.req(req.clone())).await })
            .await;
        match first {
            Ok(_) => Ok(()),
            Err(e) if e.code() == Some(ErrorCode::FailedPrecondition) => {
                let get = v1::GetJobRequest { repo: job.repo.clone(), job_id: job.id.clone() };
                let now = self.client.execution().get_job(self.client.req(get)).await.map(|r| r.into_inner());
                match now {
                    Ok(j) if j.state == "succeeded" && j.claimed_by == self.claimed_by(label) => Ok(()),
                    _ => Err(e),
                }
            }
            Err(e) => Err(e),
        }
    }

    fn claimed_by(&self, label: &str) -> String {
        format!("{}|{}", self.issuer.principal(), label)
    }

    async fn fail(&self, repo: &str, job: &v1::Job, label: &str, f: Failure) -> Disposition {
        let req = v1::FailJobRequest {
            repo: repo.into(),
            job_id: job.id.clone(),
            worker: label.into(),
            error: f.message.clone(),
            retry_after_ms: if f.retry {
                i64::try_from(self.opts.retry_after.as_millis()).unwrap_or(i64::MAX)
            } else {
                0
            },
            fencing_token: u64::from(job.attempts),
        };
        match self
            .client
            .retrying(|| async { self.client.execution().fail_job(self.client.req(req.clone())).await })
            .await
        {
            Ok(_) => Disposition::Failed { error: f.message, retry: f.retry },
            Err(e) => Disposition::Abandoned(format!("could not report the failure '{}': {e}", f.message)),
        }
    }

    /// The directory of one attempt of one job. Its name carries a random nonce from the system's generator; if
    /// that fails the job is failed (and retried) instead of using a name that a local user could predict.
    fn job_dir(&self, job_id: &str, attempt: u32) -> Result<PathBuf, Failure> {
        let safe: String =
            job_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        let nonce = ag_crypto::random_id("d").map_err(|e| {
            Failure::retry(format!("cannot name the job directory: the system's random generator failed: {e}"))
        })?;
        Ok(self.profile.scratch_dir.join(format!("{safe}-{attempt}-{nonce}")))
    }

    /// Creates the scratch directory (if it is not there), the job directory and the working tree. The last two
    /// must not exist yet: a directory somebody prepared is never reused. A worker that runs checks as another
    /// user (it is root) needs them searchable; any other worker keeps the trees of the code it checks to
    /// itself.
    fn create_job_dirs(&self, job_dir: &Path, workdir: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let mode = if self.profile.backend.tree_owner().is_some() { 0o755 } else { 0o700 };
        if let Some(parent) = job_dir.parent() {
            create_scratch_root(parent, mode)?;
        }
        for dir in [job_dir, workdir] {
            std::fs::create_dir(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode))?;
        }
        Ok(())
    }
}

/// The failure for a checkout that did not work: a tree that no worker could materialize is not retried.
fn checkout_failure(e: &CheckoutError) -> Failure {
    match e {
        CheckoutError::Unsafe(why) => {
            Failure::permanent(format!("the candidate cannot be checked by any worker: {why}"))
        }
        other => Failure::retry(other.to_string()),
    }
}

fn io_failure(e: std::io::Error) -> Failure {
    Failure::retry(format!("cannot prepare the job directory: {e}"))
}

/// Creates the scratch directory (and parents) if it does not exist, with `mode`. One that exists is left
/// exactly as it is (it may be somebody else's, such as `/tmp`); `Profile::scratch_problem` has already
/// refused one the worker cannot trust.
fn create_scratch_root(dir: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode))
}

fn remove_job_dir(dir: &Path) {
    if let Err(e) = std::fs::remove_dir_all(dir)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(dir = %dir.display(), error = %e, "cannot remove the job directory");
    }
}

/// Extends the lease every third of its length until told to stop; sets `lost` when the server says the
/// claim is gone.
async fn keep_lease(
    client: Client,
    repo: String,
    job: v1::Job,
    worker: String,
    lease: Duration,
    mut stop: oneshot::Receiver<()>,
    lost: Arc<AtomicBool>,
) {
    let every = (lease / 3).max(Duration::from_millis(250));
    loop {
        tokio::select! {
            _ = &mut stop => return,
            _ = tokio::time::sleep(every) => {}
        }
        let req = v1::HeartbeatJobRequest {
            repo: repo.clone(),
            job_id: job.id.clone(),
            worker: worker.clone(),
            lease_ms: lease.as_millis() as i64,
            fencing_token: u64::from(job.attempts),
        };
        match client.execution().heartbeat_job(client.req(req)).await {
            Ok(_) => {}
            Err(status) => {
                let e = ag_protocol::ProtocolError::from_status(&status);
                if matches!(e.code, ErrorCode::FailedPrecondition | ErrorCode::NotFound | ErrorCode::PermissionDenied) {
                    tracing::warn!(job = %job.id, error = %e.message, "the claim is gone");
                    lost.store(true, Ordering::SeqCst);
                    return;
                }
                tracing::warn!(job = %job.id, error = %e.message, "heartbeat failed; will try again");
            }
        }
    }
}
