//! Compatibility of the server's persisted state: a data directory written by an earlier `ag-server` (the control
//! database with its credentials, role bindings, repositories and idempotency records, plus the per-repository
//! stores) must be served unchanged by this build, and an older control database must be migrated, not rejected.
//!
//! `tests/compat/fixtures/control-<label>.tar.gz` comes from `tests/compat/generate_server.sh`; its `.expected.json`
//! holds the (throw-away) tokens minted by that build and what the API said then. The server under test runs on a
//! clock fixed shortly after the fixture was made, so the credentials are within their lifetime however old the
//! fixture becomes. Fixtures are append-only: when the control database schema changes, ADD a fixture and keep the
//! old ones passing (docs/architecture/compatibility.md). Every fixture is run through every test here.
#![cfg(unix)]
#![allow(missing_docs)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use ag_server::app::App;
use ag_server::config::ServerConfig;
use ag_storage::ManualClock;
use ag_types::Timestamp;
use common::{Client, Http, upload};
use serde_json::{Value, json};

struct Running {
    http: Http,
    expected: Value,
    label: String,
    _dir: tempfile::TempDir,
    shutdown: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
}

impl Running {
    fn as_token(&self, name: &str) -> Http {
        Http { base: self.http.base.clone(), token: self.expected["tokens"][name].as_str().unwrap().to_string() }
    }

    /// Stops the server and hands back the data directory, so a test can look at what the server left.
    async fn stop(self) -> tempfile::TempDir {
        let _ = self.shutdown.send(());
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), self.handle).await;
        self._dir
    }
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/fixtures")
}

fn fixture_tars() -> Vec<PathBuf> {
    let dir = fixtures_dir();
    let mut tars: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("tests/compat/fixtures exists")
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy();
            n.starts_with("control-") && n.ends_with(".tar.gz")
        })
        .collect();
    tars.sort();
    assert!(!tars.is_empty(), "no control-database fixtures in {}", dir.display());
    tars
}

/// The highest migration number the control database has in this build (`migrations/control/sqlite/NNNN_*.sql`).
fn latest_control_schema() -> i64 {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations/control/sqlite");
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().to_string_lossy().split('_').next()?.parse::<i64>().ok())
        .max()
        .expect("control migrations exist")
}

fn unpack(tar: &Path) -> (tempfile::TempDir, String, Value) {
    let name = tar.file_name().unwrap().to_string_lossy().into_owned();
    let label = name.trim_start_matches("control-").trim_end_matches(".tar.gz").to_string();
    let expected: Value =
        serde_json::from_slice(&std::fs::read(tar.with_file_name(format!("control-{label}.expected.json"))).unwrap())
            .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let status = Command::new("tar").arg("-xzf").arg(tar).arg("-C").arg(dir.path()).status().expect("tar");
    assert!(status.success(), "cannot unpack {name}");
    (dir, label, expected)
}

fn config_for(dir: &Path) -> ServerConfig {
    let mut cfg = ServerConfig::default();
    cfg.server.data_dir = dir.join("data");
    cfg.limits.requests_per_second = 0;
    cfg
}

fn control_clock(expected: &Value) -> Arc<ManualClock> {
    Arc::new(ManualClock::new(Timestamp::from_millis(expected["created_at_ms"].as_i64().unwrap() + 60_000)))
}

async fn start(tar: &Path) -> Running {
    let (dir, label, expected) = unpack(tar);
    let cfg = config_for(dir.path());
    let bootstrap = expected["bootstrap_token"].as_str().unwrap().to_string();
    let app = App::with_options(cfg, control_clock(&expected), Some(bootstrap.clone()))
        .unwrap_or_else(|e| panic!("fixture {label}: this server cannot open the data directory: {e}"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        ag_server::server::serve(app, listener, async {
            let _ = rx.await;
        })
        .await
        .unwrap();
    });
    Running { http: Http { base, token: bootstrap }, expected, label, _dir: dir, shutdown: tx, handle }
}

/// The scope a credential reports now against the one recorded: every recorded field is the same, and a field a
/// later build added is empty (a credential made before it existed is unrestricted in that respect).
fn assert_scope_unchanged(now: &Value, recorded: &Value, what: &str) {
    let (now, recorded) = (now.as_object().expect("a scope object"), recorded.as_object().unwrap());
    for (key, value) in recorded {
        assert_eq!(now.get(key), Some(value), "fixture {what}: scope.{key}");
    }
    for (key, value) in now.iter().filter(|(k, _)| !recorded.contains_key(*k)) {
        let empty = value.is_null() || value.as_str() == Some("") || value.as_array().is_some_and(|a| a.is_empty());
        assert!(empty, "fixture {what}: scope.{key} was added since the fixture and is not empty: {value}");
    }
}

fn sorted(v: &Value) -> Vec<Value> {
    let mut items: Vec<Value> = v.as_array().unwrap().clone();
    items.sort_by_key(|i| i.to_string());
    items
}

fn list(doc: &Value) -> &Vec<Value> {
    match doc {
        Value::Array(a) => a,
        Value::Object(m) => m.values().find_map(Value::as_array).expect("a list response"),
        other => panic!("not a list: {other}"),
    }
}

#[test]
fn the_fixtures_say_which_server_made_them() {
    // A fixture without its provenance is just a blob: it names the build and the schema it was made with, and the
    // schema is never newer than this build's.
    for tar in fixture_tars() {
        let (dir, label, expected) = unpack(&tar);
        let schema =
            expected["control_schema_version"].as_i64().unwrap_or_else(|| panic!("{label}: no schema version"));
        assert!(schema >= 1 && schema <= latest_control_schema(), "fixture {label}: schema {schema}");
        assert!(expected["server"].as_str().unwrap().starts_with("ag-server "), "fixture {label}");
        // and the file really is at the schema it claims
        let db = rusqlite::Connection::open_with_flags(
            dir.path().join("data/control.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, schema, "fixture {label}: control.db is not at the schema its manifest names");
    }
}

#[tokio::test]
async fn old_credentials_roles_and_repositories_work_unchanged() {
    for tar in fixture_tars() {
        let s = start(&tar).await;
        let label = s.label.clone();

        // The bootstrap path and every minted token authenticate as they did.
        let r = s.http.get("/v1/whoami").await;
        assert_eq!(r.status, 200, "fixture {label}: {}", r.json);
        for who in ["alice", "bot"] {
            let r = s.as_token(who).get("/v1/whoami").await;
            assert_eq!(r.status, 200, "fixture {label}: {who}: {}", r.json);
            assert_eq!(r.json["identity"], s.expected["whoami"][who]["identity"], "fixture {label}: {who}");
            assert_eq!(r.json["credential_id"], s.expected["credential_ids"][who]);
            assert_eq!(r.json["roles"], s.expected["whoami"][who]["roles"]);
            assert_scope_unchanged(&r.json["scope"], &s.expected["whoami"][who]["scope"], &format!("{label}: {who}"));
        }
        // A revoked credential stays revoked; garbage never authenticates.
        assert_eq!(s.as_token("revoked").get("/v1/whoami").await.status, 401, "fixture {label}: revoked token");
        let nobody = Http { base: s.http.base.clone(), token: "agt_0000000000000000_bm9wZQ".into() };
        assert_eq!(nobody.get("/v1/whoami").await.status, 401);

        // The repositories and the role bindings are what was recorded.
        let repos = s.http.get("/v1/repos").await;
        let mut got: Vec<Value> =
            list(&repos.json).iter().map(|r| json!([r["name"], r["default_stream"], r["namespace"]])).collect();
        got.sort_by_key(|v| v.to_string());
        assert_eq!(got, sorted(&s.expected["repos"]), "fixture {label}: repositories");
        let roles = s.http.get("/v1/roles").await;
        let mut got: Vec<Value> =
            list(&roles.json).iter().map(|r| json!([r["subject"], r["repo"], r["role"]])).collect();
        got.sort_by_key(|v| v.to_string());
        assert_eq!(got, sorted(&s.expected["roles"]), "fixture {label}: role bindings");

        // Authorization is still enforced: the bot's credential is limited to one repository.
        let bot = s.as_token("bot");
        assert_eq!(bot.get("/v1/repos/acme/web/streams").await.status, 200, "fixture {label}");
        // (403 when the server says "not allowed", 404 when it does not even admit the repository exists)
        let outside = bot.get("/v1/repos/acme/docs/streams").await.status;
        assert!(
            matches!(outside, 403 | 404),
            "fixture {label}: a credential scoped to one repository read another: {outside}"
        );

        // The idempotency record written by the old build still answers: the same request is a replay.
        let again = s
            .http
            .post(
                "/v1/repos",
                json!({"name": "acme/web", "default_stream": "main", "description": "the web app", "idempotency_key": "compat-create-web"}),
            )
            .await;
        assert_eq!(again.status, 200, "fixture {label}: replaying the old repo.create: {}", again.json);
        assert_eq!(again.json["name"], "acme/web");
        let clash = s
            .http
            .post(
                "/v1/repos",
                json!({"name": "acme/web", "default_stream": "main", "description": "another", "idempotency_key": "compat-create-web"}),
            )
            .await;
        assert_eq!(clash.status, 422, "fixture {label}: the old key still refuses a different request");

        s.stop().await;
    }
}

#[tokio::test]
async fn an_old_run_bound_credential_still_acts_as_its_run_and_dies_with_it() {
    let mut seen = 0;
    for tar in fixture_tars() {
        let s = start(&tar).await;
        let Some(bound) = s.expected.get("run_bound").cloned() else {
            s.stop().await; // made before run-bound credentials existed (schema < 3)
            continue;
        };
        seen += 1;
        let label = s.label.clone();
        let (run, repo) = (bound["run_id"].as_str().unwrap().to_string(), bound["repo"].as_str().unwrap().to_string());
        let http = Http { base: s.http.base.clone(), token: bound["token"].as_str().unwrap().to_string() };

        let me = http.get("/v1/whoami").await;
        assert_eq!(me.status, 200, "fixture {label}: {}", me.json);
        assert_eq!(me.json["identity"], bound["whoami"]["identity"]);
        assert_eq!(me.json["credential_id"], bound["credential_id"]);
        assert_eq!(me.json["scope"]["run_id"], run.as_str());
        // it is the agent's own run, so it can heartbeat it, and no other run
        let beat = http.post(&format!("/v1/repos/{repo}/runs/{run}/heartbeat"), json!({})).await;
        assert_eq!(beat.status, 200, "fixture {label}: {}", beat.json);
        let other = http.post(&format!("/v1/repos/{repo}/runs/run_not_this_one/heartbeat"), json!({})).await;
        assert!(matches!(other.status, 403 | 404), "fixture {label}: {} {}", other.status, other.json);

        // ending the run ends the credential, exactly as for a credential minted by this build
        let done = http.post(&format!("/v1/repos/{repo}/runs/{run}/complete"), json!({"status": "completed"})).await;
        assert_eq!(done.status, 200, "fixture {label}: {}", done.json);
        assert_eq!(http.get("/v1/whoami").await.status, 401, "fixture {label}: the credential outlived its run");
        s.stop().await;
    }
    assert!(seen > 0, "no fixture has a run-bound credential: the schema-3 fixture is missing");
}

#[tokio::test]
async fn an_old_data_directory_accepts_new_work() {
    for tar in fixture_tars() {
        let s = start(&tar).await;
        let alice = s.as_token("alice");
        let c = Client::new("acme/web");
        let (head, objects) = c.commit(
            vec![],
            "user:alice",
            "first change on an old data directory",
            &[("README.md", b"# acme\n"), ("src/lib.rs", b"fn main() {}\n")],
            1_700_000_000_000,
        );
        upload(&alice, "acme/web", &objects).await;
        let r = alice
            .post(
                "/v1/repos/acme/web/proposals",
                json!({"stream": "main", "head": head.to_string(), "title": "Initial import", "idempotency_key": "p1"}),
            )
            .await;
        assert_eq!(r.status, 200, "fixture {}: {}", s.label, r.json);
        let listed = alice.get("/v1/repos/acme/web/proposals").await;
        assert_eq!(list(&listed.json).len(), 1);
        // and the audit chain of the repository is intact
        let admin = s.http.get("/v1/repos/acme/web/audit/verify").await;
        assert_eq!(admin.status, 200, "{}", admin.json);
        s.stop().await;
    }
}

#[tokio::test]
async fn an_old_control_database_is_migrated_to_this_builds_schema() {
    for tar in fixture_tars() {
        let s = start(&tar).await;
        let label = s.label.clone();
        // the server opened it and works ...
        assert_eq!(s.http.get("/v1/whoami").await.status, 200, "fixture {label}");
        let dir = s.stop().await;
        // ... and left it at the newest schema, not at the one it found
        let db = rusqlite::Connection::open_with_flags(
            dir.path().join("data/control.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, latest_control_schema(), "fixture {label}: not migrated to this build's schema");
    }
}

#[tokio::test]
async fn a_control_database_from_a_newer_build_is_refused_not_misread() {
    for tar in fixture_tars() {
        let (dir, label, expected) = unpack(&tar);
        {
            let db = rusqlite::Connection::open(dir.path().join("data/control.db")).unwrap();
            db.execute_batch(&format!("PRAGMA user_version = {}", latest_control_schema() + 1)).unwrap();
        }
        let refused = App::with_options(config_for(dir.path()), control_clock(&expected), Some("x".repeat(32)));
        let err = match refused {
            Ok(_) => panic!("fixture {label}: a server opened a control database that a newer build wrote"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("newer than this server supports"), "fixture {label}: {err}");
        // and it did not touch the file: the version is still the one we set
        let db = rusqlite::Connection::open(dir.path().join("data/control.db")).unwrap();
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, latest_control_schema() + 1, "fixture {label}");
    }
}
