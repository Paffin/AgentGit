//! Compatibility of the server's persisted state: a data directory written by an earlier `ag-server` (the
//! control database with its credentials, role bindings, repositories and idempotency records, plus the
//! per-repository stores) must be served unchanged by this build.
//!
//! `tests/compat/fixtures/control-<label>.tar.gz` comes from `tests/compat/generate_server.sh`; its
//! `.expected.json` holds the (throw-away) tokens minted by that build and what the API said then. The server
//! under test runs on a clock fixed shortly after the fixture was made, so the credentials are within their
//! lifetime however old the fixture becomes. Fixtures are append-only: when the control database schema
//! changes, ADD a fixture and keep this one passing (docs/architecture/compatibility.md).
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

    async fn stop(self) {
        let _ = self.shutdown.send(());
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), self.handle).await;
    }
}

fn fixture_tars() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/fixtures");
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

async fn start(tar: &Path) -> Running {
    let name = tar.file_name().unwrap().to_string_lossy().into_owned();
    let label = name.trim_start_matches("control-").trim_end_matches(".tar.gz").to_string();
    let expected: Value =
        serde_json::from_slice(&std::fs::read(tar.with_file_name(format!("control-{label}.expected.json"))).unwrap())
            .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let status = Command::new("tar").arg("-xzf").arg(tar).arg("-C").arg(dir.path()).status().expect("tar");
    assert!(status.success(), "cannot unpack {name}");

    let mut cfg = ServerConfig::default();
    cfg.server.data_dir = dir.path().join("data");
    cfg.limits.requests_per_second = 0;
    let now = expected["created_at_ms"].as_i64().unwrap() + 60_000;
    let clock = Arc::new(ManualClock::new(Timestamp::from_millis(now)));
    let bootstrap = expected["bootstrap_token"].as_str().unwrap().to_string();
    let app = App::with_options(cfg, clock, Some(bootstrap.clone()))
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
            assert_eq!(r.json["scope"], s.expected["whoami"][who]["scope"]);
        }
        // A revoked credential stays revoked; garbage never authenticates.
        assert_eq!(s.as_token("revoked").get("/v1/whoami").await.status, 401, "fixture {label}: revoked token");
        let nobody = Http { base: s.http.base.clone(), token: "agt_cred_0000000000000000_bm9wZQ".into() };
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
        assert_eq!(bot.get("/v1/repos/acme/docs/streams").await.status, 403, "fixture {label}");

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
