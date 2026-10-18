//! Shared fixtures: an `ag-server` in this process, a repository with a policy that runs a check, and helpers
//! to propose changes, queue validations and run workers.
#![allow(dead_code, missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use ag_changeset::Op;
use ag_client::{Client, Options as ClientOptions};
use ag_crypto::KeyPair;
use ag_integrate::Repository;
use ag_protocol::v1;
use ag_sandbox::IsolationLevel;
use ag_server::app::App;
use ag_server::auth::mint_token;
use ag_server::authz::Role;
use ag_server::config::ServerConfig;
use ag_server::control::CredentialRow;
use ag_storage::SystemClock;
use ag_types::{ChangesetId, FileType, Identity, RepoPath};
use ag_worker::{Backend, JobReport, Options, Profile, Worker};
use serde_json::{Value, json};

pub const REPO: &str = "acme/web";
pub const BOOTSTRAP: &str = "bootstrap-token-0123456789abcdef";

// ---- a server in this process -------------------------------------------------------------------------

pub struct Server {
    pub url: String,
    pub app: App,
    _dir: tempfile::TempDir,
    _thread: std::thread::JoinHandle<()>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Server {
    pub fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = ServerConfig::default();
        cfg.server.data_dir = dir.path().to_path_buf();
        cfg.limits.requests_per_second = 0;
        let app = App::with_options(cfg, Arc::new(SystemClock), Some(BOOTSTRAP.into())).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let a = app.clone();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                ag_server::server::serve(a, listener, async {
                    let _ = stop_rx.await;
                })
                .await
                .unwrap();
            });
        });
        let addr = rx.recv().unwrap();
        app.repos.create(REPO, "main", "", &Identity::new("system:test").unwrap(), Default::default()).unwrap();
        Self { url: format!("http://{addr}"), app, _dir: dir, _thread: thread, stop: Some(stop_tx) }
    }

    /// A token for `subject` holding `role` on the repository.
    pub fn token(&self, subject: &str, role: Role) -> String {
        self.app.control.grant_role(subject, REPO, role, "system:test").unwrap();
        self.app.invalidate_bindings();
        let m = mint_token().unwrap();
        let now = self.app.clock.now().as_millis();
        self.app
            .control
            .insert_credential(&CredentialRow {
                id: m.credential_id,
                secret_hash: m.secret_hash,
                subject: subject.into(),
                kind: subject.split(':').next().unwrap().into(),
                description: String::new(),
                scope: Default::default(),
                created_at_ms: now,
                expires_at_ms: now + 3_600_000,
                revoked_at_ms: None,
                last_used_ms: 0,
                created_by: "system:test".into(),
            })
            .unwrap();
        m.token
    }

    pub async fn client(&self, subject: &str, role: Role) -> Client {
        Client::connect(ClientOptions {
            endpoint: self.url.clone(),
            token: self.token(subject, role),
            timeout: Some(Duration::from_secs(60)),
            ..Default::default()
        })
        .await
        .unwrap()
    }

    pub fn repo(&self) -> Arc<Repository> {
        self.app.repos.handle(REPO).unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

// ---- the scenario ------------------------------------------------------------------------------------

pub struct World {
    pub srv: Server,
    pub ci_key: KeyPair,
    pub alice: Client,
    pub carol: Client,
    pub scratch: PathBuf,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

pub fn id(s: &str) -> Identity {
    Identity::new(s).unwrap()
}

/// Commits `files` on top of `parent` (or as a root) and returns the changeset.
pub fn commit_files(repo: &Repository, parent: Option<ChangesetId>, files: &[(&str, &[u8])]) -> ChangesetId {
    let ops = files
        .iter()
        .map(|(path, content)| Op::Put {
            path: RepoPath::new(*path).unwrap(),
            content: repo.put_blob(content.to_vec()).unwrap(),
            file_type: FileType::Regular,
            executable: path.ends_with(".sh"),
        })
        .collect();
    repo.commit(parent.into_iter().collect(), &id("user:alice"), "change", ops).unwrap()
}

/// Opens a proposal to integrate a new root commit of `files` into `main`.
pub fn propose_files(repo: &Repository, files: &[(&str, &[u8])]) -> (String, ChangesetId) {
    let head = commit_files(repo, None, files);
    let p = repo.propose("main", &head, "a change", "", &id("user:alice"), None).unwrap();
    (p.id, head)
}

impl World {
    /// A repository whose policy requires the check `tests`, run as `run`, trusting `service:ci-1`'s key.
    pub async fn new(run: Value) -> World {
        Self::with_policy(run, None).await
    }

    pub async fn with_policy(run: Value, environment_digest: Option<&str>) -> World {
        let mut check = json!({"check": "tests", "run": run});
        if let Some(d) = environment_digest {
            check["environment_digest"] = json!(d);
        }
        Self::with_checks(vec![check], None).await
    }

    /// A repository whose policy requires every one of `checks` (each `{"check": name, "run": {...}}`), trusting
    /// the key in `trusted` or, by default, a fresh one for `service:ci-1`.
    pub async fn with_checks(checks: Vec<Value>, trusted: Option<&KeyPair>) -> World {
        let srv = Server::start();
        let ci_key = KeyPair::generate().unwrap();
        let trusted_hex = trusted.map_or_else(|| ci_key.public().to_hex(), |k| k.public().to_hex());
        let policy = json!({
            "trusted_issuers": [{"identity": "service:ci-1", "public_key": trusted_hex}],
            "required_validations": checks
        });
        // (while a policy names no admins, any user may set it)
        srv.repo().set_policy("main", &policy.to_string(), &id("user:root")).unwrap();
        let alice = srv.client("user:alice", Role::Writer).await;
        let carol = srv.client("user:carol", Role::Integrator).await;
        let scratch = std::env::temp_dir().join(format!(
            "ag-worker-e2e-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        World { srv, ci_key, alice, carol, scratch }
    }

    /// Commits `files` on top of `parent` (or as a root) and returns the changeset.
    pub fn commit(&self, parent: Option<ChangesetId>, files: &[(&str, &[u8])]) -> ChangesetId {
        commit_files(&self.srv.repo(), parent, files)
    }

    pub fn propose(&self, files: &[(&str, &[u8])]) -> (String, ChangesetId) {
        propose_files(&self.srv.repo(), files)
    }

    pub async fn queue_validation(&self, proposal: &str) -> Vec<v1::Job> {
        let req = v1::RequestValidationRequest { repo: REPO.into(), proposal: proposal.into(), ..Default::default() };
        self.alice.integration().request_validation(self.alice.req(req)).await.unwrap().into_inner().jobs
    }

    pub async fn integrate(&self, proposal: &str) -> String {
        let req = v1::IntegrateRequest { repo: REPO.into(), proposal: proposal.into(), ..Default::default() };
        let r = self.carol.integration().integrate(self.carol.req(req)).await.unwrap().into_inner();
        r.outcome.map(|o| o.status).unwrap_or_default()
    }

    pub async fn job(&self, id: &str) -> v1::Job {
        let req = v1::GetJobRequest { repo: REPO.into(), job_id: id.into() };
        self.alice.execution().get_job(self.alice.req(req)).await.unwrap().into_inner()
    }

    /// A worker for `service:ci-1`, one job and out.
    pub async fn run_worker(&self, profile: Profile, opts: Options) -> Vec<JobReport> {
        let ci = self.srv.client("service:ci-1", Role::Runner).await;
        let worker = Arc::new(Worker::new(ci, profile, self.ci_key_clone(), id("service:ci-1"), opts));
        let (_stop, rx) = tokio::sync::watch::channel(false);
        tokio::time::timeout(Duration::from_secs(120), worker.run(rx, true))
            .await
            .expect("the worker finished")
            .unwrap()
    }

    pub fn ci_key_clone(&self) -> KeyPair {
        // KeyPair is deliberately not Clone; rebuild it from its file form.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k");
        self.ci_key.save(&path).unwrap();
        KeyPair::load(&path).unwrap()
    }

    pub fn profile(&self) -> Profile {
        test_profile(&self.scratch)
    }
}

pub fn namespace_works() -> bool {
    let works = ag_sandbox::NamespaceSandbox::new().availability().is_available();
    // CI sets AG_REQUIRE_SANDBOX on Linux: a runner that silently lost its isolation must not go green.
    let required = std::env::var("AG_REQUIRE_SANDBOX").is_ok_and(|v| !matches!(v.trim(), "" | "0" | "false" | "no"));
    assert!(works || !required, "AG_REQUIRE_SANDBOX is set, but the namespace sandbox is unavailable on this host");
    works
}

/// The namespace sandbox if the machine can run it, else the process sandbox (opted into).
pub fn test_profile(scratch: &Path) -> Profile {
    let mut p = if namespace_works() {
        Profile::new(Backend::namespace((65534, 65534)))
    } else {
        eprintln!("NOTE: the namespace sandbox is unavailable here; the loop is tested with the process sandbox");
        let mut p = Profile::new(Backend::Process);
        p.min_isolation = IsolationLevel::None;
        p
    };
    p.scratch_dir = scratch.to_path_buf();
    p
}

pub fn opts() -> Options {
    Options { label: "e2e".into(), wait: Duration::from_secs(10), ..Options::default() }
}

pub fn only(mut reports: Vec<JobReport>) -> JobReport {
    assert_eq!(reports.len(), 1, "{reports:?}");
    reports.remove(0)
}

pub fn sh(script: &str, timeout_secs: u32) -> Value {
    json!({"argv": ["sh", "-c", script], "timeout_secs": timeout_secs, "memory_mb": 256})
}
