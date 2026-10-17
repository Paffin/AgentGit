//! Shared harness for the `ag` binary tests: a scratch repository, hermetic environment, and (for the
//! remote tests) an in-process `ag-server`.
#![allow(dead_code, missing_docs)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use ag_server::app::App;
use ag_server::auth::mint_token;
use ag_server::authz::Role;
use ag_server::config::ServerConfig;
use ag_server::control::CredentialRow;
use ag_storage::SystemClock;
use serde_json::Value;

/// The result of one `ag` invocation.
pub struct Run {
    pub out: Output,
}

impl Run {
    pub fn code(&self) -> i32 {
        self.out.status.code().unwrap_or(-1)
    }
    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.out.stdout).into_owned()
    }
    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.out.stderr).into_owned()
    }
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout()).unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.stdout()))
    }
    #[track_caller]
    pub fn ok(self) -> Self {
        assert_eq!(self.code(), 0, "stdout: {}\nstderr: {}", self.stdout(), self.stderr());
        self
    }
    #[track_caller]
    pub fn code_is(self, c: i32) -> Self {
        assert_eq!(self.code(), c, "stdout: {}\nstderr: {}", self.stdout(), self.stderr());
        self
    }
}

/// A scratch repository with a hermetic environment (no inherited colour, token or config settings).
pub struct Ag {
    pub dir: tempfile::TempDir,
    pub cfg: tempfile::TempDir,
}

/// Runs `ag` in `cwd` with a clean environment plus `env`.
pub fn run_in(cwd: &Path, cfg: &Path, who: &str, args: &[&str], env: &[(&str, &str)]) -> Run {
    let mut c = Command::new(env!("CARGO_BIN_EXE_ag"));
    c.current_dir(cwd)
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("AG_TOKEN")
        .env_remove("AG_RUN")
        .env_remove("AG_KEY_DIR")
        .env_remove("TERM")
        .env("AG_IDENTITY", who)
        .env("AG_CONFIG", cfg.join("config.json"))
        .args(args);
    for (k, v) in env {
        c.env(k, v);
    }
    Run { out: c.output().expect("run ag") }
}

impl Ag {
    /// A directory that is not yet a repository.
    pub fn bare() -> Self {
        Self { dir: tempfile::tempdir().unwrap(), cfg: tempfile::tempdir().unwrap() }
    }

    /// A repository `acme/demo` initialized by `user:alice`.
    pub fn new() -> Self {
        let a = Self::bare();
        a.ag(&["init", "--namespace", "acme/demo"]).ok();
        a
    }

    pub fn root(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    pub fn run_as(&self, who: &str, args: &[&str]) -> Run {
        run_in(self.dir.path(), self.cfg.path(), who, args, &[])
    }

    pub fn run_env(&self, who: &str, args: &[&str], env: &[(&str, &str)]) -> Run {
        run_in(self.dir.path(), self.cfg.path(), who, args, env)
    }

    pub fn ag(&self, args: &[&str]) -> Run {
        self.run_as("user:alice", args)
    }

    pub fn write(&self, rel: &str, data: &str) {
        let p = self.dir.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    }

    /// snapshot + propose as `who`, returns the proposal id
    pub fn propose_as(&self, who: &str, msg: &str) -> String {
        self.run_as(who, &["snapshot", "-m", msg]).ok();
        self.run_as(who, &["--json", "propose", "-t", msg]).ok().json()["id"].as_str().unwrap().to_string()
    }

    /// First commit landed on `main`: README.md and src/lib.rs.
    pub fn land_first_change(&self) -> String {
        self.write("README.md", "# demo\n");
        self.write("src/lib.rs", "pub fn one() -> u32 { 1 }\n");
        let p = self.propose_as("user:alice", "initial import");
        self.ag(&["integrate", &p]).ok();
        p
    }
}

/// An in-process `ag-server` on a free port.
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
        let app =
            App::with_options(cfg, Arc::new(SystemClock), Some("bootstrap-token-0123456789abcdef".into())).unwrap();
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
        Self { url: format!("http://{addr}"), app, _dir: dir, _thread: thread, stop: Some(stop_tx) }
    }

    /// Creates the repository `name` (owner/name) with default stream `main`.
    pub fn create_repo(&self, name: &str) {
        self.app
            .repos
            .create(name, "main", "", &ag_types::Identity::new("system:test").unwrap(), Default::default())
            .unwrap();
    }

    /// Grants `subject` a role on `repo` and mints a credential that lives for `ttl_ms`.
    pub fn user_ttl(&self, subject: &str, repo: &str, role: Role, ttl_ms: i64) -> String {
        self.app.control.grant_role(subject, repo, role, "system:test").unwrap();
        self.app.invalidate_bindings();
        let m = mint_token().unwrap();
        let now = self.app.clock.now().as_millis();
        self.app
            .control
            .insert_credential(&CredentialRow {
                id: m.credential_id,
                secret_hash: m.secret_hash,
                subject: subject.into(),
                kind: "user".into(),
                description: String::new(),
                scope: Default::default(),
                created_at_ms: now,
                expires_at_ms: now + ttl_ms,
                revoked_at_ms: None,
                last_used_ms: 0,
                created_by: "system:test".into(),
            })
            .unwrap();
        m.token
    }

    /// A credential valid for 30 days.
    pub fn user(&self, subject: &str, repo: &str, role: Role) -> String {
        self.user_ttl(subject, repo, role, 30 * 86_400_000)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}
