#![allow(dead_code)]
//! A real server on an ephemeral port, spoken to over real sockets.

use std::sync::Arc;

use ag_changeset::{Changeset, Op};
use ag_object::RawObject;
use ag_server::app::App;
use ag_server::config::ServerConfig;
use ag_storage::SystemClock;
use ag_types::{ChangesetId, FileType, Identity, Limits, Namespace, RepoPath, Timestamp};
use serde_json::{Value, json};
use tempfile::TempDir;

pub const ADMIN_TOKEN: &str = "test-bootstrap-token-0123456789abcdef";

pub struct TestServer {
    pub base: String,
    pub app: App,
    pub dir: TempDir,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl TestServer {
    pub async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    pub async fn start_with(tweak: impl FnOnce(&mut ServerConfig)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = ServerConfig::default();
        cfg.server.data_dir = dir.path().to_path_buf();
        cfg.limits.requests_per_second = 0; // tests opt in to rate limiting explicitly
        tweak(&mut cfg);
        let app = App::with_options(cfg, Arc::new(SystemClock), Some(ADMIN_TOKEN.to_string())).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let a = app.clone();
        let handle = tokio::spawn(async move {
            ag_server::server::serve(a, listener, async {
                let _ = rx.await;
            })
            .await
            .unwrap();
        });
        Self { base, app, dir, shutdown: Some(tx), handle: Some(handle) }
    }

    pub async fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.handle.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), h).await;
        }
    }

    pub fn client(&self, token: &str) -> Http {
        Http { base: self.base.clone(), token: token.to_string() }
    }

    pub fn admin(&self) -> Http {
        self.client(ADMIN_TOKEN)
    }
}

/// A blocking HTTP client wrapped for async tests.
#[derive(Clone)]
pub struct Http {
    pub base: String,
    pub token: String,
}

pub struct Reply {
    pub status: u16,
    pub json: Value,
    pub body: Vec<u8>,
}

impl Http {
    pub async fn call(&self, method: &str, path: &str, body: Option<Value>, headers: &[(&str, &str)]) -> Reply {
        let (url, token, method) = (format!("{}{}", self.base, path), self.token.clone(), method.to_string());
        let headers: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        tokio::task::spawn_blocking(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
            let mut builder = ureq::http::Request::builder().method(method.as_str()).uri(&url);
            if !token.is_empty() {
                builder = builder.header("authorization", format!("Bearer {token}"));
            }
            for (k, v) in &headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            let resp = match body {
                Some(b) => agent.run(builder.header("content-type", "application/json").body(b.to_string()).unwrap()),
                None => agent.run(builder.body(()).unwrap()),
            }
            .unwrap();
            let status = resp.status().as_u16();
            let mut resp = resp;
            let body = resp.body_mut().read_to_vec().unwrap_or_default();
            let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
            Reply { status, json, body }
        })
        .await
        .unwrap()
    }

    pub async fn get(&self, path: &str) -> Reply {
        self.call("GET", path, None, &[]).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Reply {
        self.call("POST", path, Some(body), &[]).await
    }

    pub async fn put_bytes(&self, path: &str, bytes: Vec<u8>) -> Reply {
        let (url, token) = (format!("{}{}", self.base, path), self.token.clone());
        tokio::task::spawn_blocking(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
            let resp = agent
                .run(
                    ureq::http::Request::builder()
                        .method("PUT")
                        .uri(&url)
                        .header("authorization", format!("Bearer {token}"))
                        .header("content-type", "application/octet-stream")
                        .body(bytes)
                        .unwrap(),
                )
                .unwrap();
            let status = resp.status().as_u16();
            let mut resp = resp;
            let body = resp.body_mut().read_to_vec().unwrap_or_default();
            Reply { status, json: serde_json::from_slice(&body).unwrap_or(Value::Null), body }
        })
        .await
        .unwrap()
    }
}

/// Builds changesets locally the way a client would.
pub struct Client {
    pub namespace: Namespace,
    pub limits: Limits,
}

impl Client {
    pub fn new(repo: &str) -> Self {
        Self { namespace: Namespace::new(repo).unwrap(), limits: Limits::default() }
    }

    /// A root or child changeset that puts the given files; returns the changeset and every object to upload.
    pub fn commit(
        &self,
        parents: Vec<ChangesetId>,
        author: &str,
        message: &str,
        files: &[(&str, &[u8])],
        ts: i64,
    ) -> (ChangesetId, Vec<RawObject>) {
        let mut objects = Vec::new();
        let mut ops = Vec::new();
        for (path, content) in files {
            let blob = RawObject::blob(content.to_vec());
            let id = ag_types::BlobId::new(blob.id());
            objects.push(blob);
            ops.push(Op::Put {
                path: RepoPath::new(*path).unwrap(),
                content: id,
                file_type: FileType::Regular,
                executable: false,
            });
        }
        let cs = Changeset::build(
            self.namespace.clone(),
            parents,
            Identity::new(author).unwrap(),
            Timestamp::from_millis(ts),
            message,
            ops,
            &self.limits,
        )
        .unwrap();
        let id = cs.id();
        objects.push(cs.to_raw());
        (id, objects)
    }
}

pub async fn upload(http: &Http, repo: &str, objects: &[RawObject]) {
    for o in objects {
        let r = http.put_bytes(&format!("/v1/repos/{repo}/objects/{}", o.id()), o.encode_envelope()).await;
        assert_eq!(r.status, 200, "upload failed: {}", r.json);
    }
}

/// Grants `role` to `subject` on `repo` and mints a token for it.
pub async fn user_with_role(srv: &TestServer, subject: &str, repo: &str, role: &str) -> Http {
    let admin = srv.admin();
    let r = admin.post("/v1/roles", json!({"subject": subject, "repo": repo, "role": role})).await;
    assert_eq!(r.status, 200, "{}", r.json);
    let r = admin.post("/v1/credentials", json!({"subject": subject, "ttl_secs": 3600})).await;
    assert_eq!(r.status, 200, "{}", r.json);
    srv.client(r.json["token"].as_str().unwrap())
}
