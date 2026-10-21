//! Several real `ag-server` processes on one PostgreSQL and one S3-compatible store (ADR-0007).
//!
//! Every test starts 2–3 replicas of the actual binary (`serve --config …`) that share a PostgreSQL control
//! schema, PostgreSQL schemas for the repositories and an S3 bucket, and drives them over real sockets, the
//! way a load balancer's clients would. They run only when both `AG_TEST_POSTGRES_URL` and `AG_TEST_S3_ENDPOINT`
//! are set (see `scripts/ha-test.sh`, which wraps `scripts/pg-test.sh` and `scripts/s3-test.sh`); with
//! `AG_REQUIRE_COMPONENTS=postgres,s3` a missing variable fails the test instead of skipping it.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ag_crypto::KeyPair;
use ag_policy::{Attestation, CheckResult};
use ag_types::{ChangesetId, Identity, Timestamp, TreeId};
use common::{ADMIN_TOKEN, Client};
use serde_json::{Value, json};

// ---- the processes ---------------------------------------------------------------------------------------------

/// The tests start dozens of processes and measure timings: they run one at a time, whatever `--test-threads` says.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct Backends {
    pg_url: String,
    s3_endpoint: String,
    s3_bucket: String,
    s3_region: String,
}

fn backends() -> Option<Backends> {
    let pg_url = ag_testkit::require_here(
        "postgres",
        "AG_TEST_POSTGRES_URL is not set (use scripts/pg-test.sh)",
        ag_storage::testing::postgres_url(),
    )?;
    let s3_endpoint = ag_testkit::require_here(
        "s3",
        "AG_TEST_S3_ENDPOINT is not set (use scripts/s3-test.sh)",
        ag_storage::testing::s3_endpoint(),
    )?;
    let var = |k: &str, d: &str| std::env::var(k).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| d.to_string());
    Some(Backends {
        pg_url,
        s3_endpoint,
        s3_bucket: var("AG_TEST_S3_BUCKET", "ag-test"),
        s3_region: var("AG_TEST_S3_REGION", "us-east-1"),
    })
}

struct Replica {
    name: String,
    pg_url: String,
    port: u16,
    #[allow(dead_code)]
    dir: PathBuf,
    cfg: PathBuf,
    log: PathBuf,
    child: Option<Child>,
}

impl Replica {
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn spawn(&mut self) {
        let log = std::fs::OpenOptions::new().create(true).append(true).open(&self.log).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_ag-server"))
            .args(["serve", "--config"])
            .arg(&self.cfg)
            .env("AG_BOOTSTRAP_TOKEN", ADMIN_TOKEN)
            .env("AG_TEST_POSTGRES_URL", &self.pg_url)
            .env("AG_METRICS_TOKEN", "")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost")
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("start ag-server");
        self.child = Some(child);
    }

    /// SIGKILL: no drain, no cleanup, connections just vanish.
    fn kill9(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// SIGTERM, then wait for the exit status.
    #[allow(dead_code)]
    fn term(&mut self, within: Duration) -> Option<std::process::ExitStatus> {
        let c = self.child.as_mut()?;
        let _ = Command::new("kill").args(["-TERM", &c.id().to_string()]).status();
        let t = Instant::now();
        while t.elapsed() < within {
            if let Ok(Some(st)) = c.try_wait() {
                self.child = None;
                return Some(st);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    fn log_tail(&self) -> String {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(25)..].join("\n")
    }
}

struct Options {
    drain_secs: u64,
    bindings_ttl_ms: u64,
    /// The PostgreSQL URL the replicas get (default: the test cluster's).
    pg_url: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self { drain_secs: 0, bindings_ttl_ms: 5000, pg_url: None }
    }
}

/// What differs between the configuration files of the processes of one test.
struct Shape<'a> {
    port: u16,
    data: &'a std::path::Path,
    name: &'a str,
    drain_secs: u64,
    /// `Some((schema, bindings_ttl_ms))`: the control plane is on PostgreSQL; `None`: the SQLite file in `data`.
    control: Option<(&'a str, u64)>,
    repo_schema: &'a str,
    s3_prefix: &'a str,
}

/// A server configuration: repositories in PostgreSQL + S3 (so any process can serve them), the control plane
/// per `shape.control`.
fn config_text(be: &Backends, shape: &Shape<'_>) -> String {
    let control = match shape.control {
        Some((schema, ttl)) => format!(
            "[control]\nurl_env = \"AG_TEST_POSTGRES_URL\"\nschema = \"{schema}\"\npool_size = 6\nbindings_ttl_ms = {ttl}\n"
        ),
        None => String::new(),
    };
    format!(
        r#"
[server]
listen = "127.0.0.1:{port}"
data_dir = "{data}"
replica_id = "{name}"
drain_secs = {drain}

{control}
[limits]
requests_per_second = 0

[repos]
pool_size = 2

[repos.storage.meta]
kind = "postgres"
url_env = "AG_TEST_POSTGRES_URL"
schema = "{repo_schema}"
pool_size = 4

[repos.storage.objects]
kind = "s3"
endpoint = "{endpoint}"
region = "{region}"
bucket = "{bucket}"
prefix = "{s3_prefix}"
path_style = true
access_key_id_env = "AG_TEST_S3_ACCESS_KEY_ID"
secret_access_key_env = "AG_TEST_S3_SECRET_ACCESS_KEY"
"#,
        port = shape.port,
        data = shape.data.display(),
        name = shape.name,
        drain = shape.drain_secs,
        repo_schema = shape.repo_schema,
        s3_prefix = shape.s3_prefix,
        endpoint = be.s3_endpoint,
        region = be.s3_region,
        bucket = be.s3_bucket,
    )
}

fn wait_replica_ready(r: &Replica, within: Duration) {
    let t = Instant::now();
    loop {
        if let Ok(rep) = send("GET", &format!("{}/readyz", r.url()), "", Body::None, &[])
            && rep.status == 200
        {
            return;
        }
        if t.elapsed() > within {
            panic!("{} did not become ready within {within:?}; log:\n{}", r.name, r.log_tail());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct Cluster {
    _root: tempfile::TempDir,
    be: Backends,
    control_schema: String,
    repo_schema: String,
    repos: Vec<String>,
    replicas: Vec<Replica>,
}

/// A port nobody listens on. The listener is closed before the replica binds, and the kernel may hand the same
/// number to the next caller while the first replica is still starting (two replicas of one cluster then fight for
/// it: "Address already in use"). So no number is given out twice by this process.
fn free_port() -> u16 {
    static GIVEN: std::sync::Mutex<Vec<u16>> = std::sync::Mutex::new(Vec::new());
    let mut given = GIVEN.lock().unwrap();
    loop {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        if !given.contains(&port) {
            given.push(port);
            return port;
        }
    }
}

impl Cluster {
    /// Starts `n` replicas at the same time and waits until all are ready.
    fn start(be: Backends, n: usize, opts: Options) -> Self {
        let root = tempfile::tempdir().unwrap();
        let control_schema = ag_storage::testing::unique("agha_ctl");
        let repo_schema = ag_storage::testing::unique("agha");
        let s3_prefix = ag_storage::testing::unique("ha");
        let mut replicas = Vec::new();
        for i in 0..n {
            let name = format!("replica-{i}");
            let dir = root.path().join(&name);
            std::fs::create_dir_all(&dir).unwrap();
            let port = free_port();
            let cfg = dir.join("ag-server.toml");
            std::fs::write(
                &cfg,
                config_text(
                    &be,
                    &Shape {
                        port,
                        data: &dir.join("data"),
                        name: &name,
                        drain_secs: opts.drain_secs,
                        control: Some((&control_schema, opts.bindings_ttl_ms)),
                        repo_schema: &repo_schema,
                        s3_prefix: &s3_prefix,
                    },
                ),
            )
            .unwrap();
            let pg_url = opts.pg_url.clone().unwrap_or_else(|| be.pg_url.clone());
            replicas.push(Replica { name, pg_url, port, log: dir.join("server.log"), dir, cfg, child: None });
        }
        let mut cluster = Self { _root: root, be, control_schema, repo_schema, repos: Vec::new(), replicas };
        for r in &mut cluster.replicas {
            r.spawn();
        }
        for i in 0..n {
            cluster.wait_ready(i, Duration::from_secs(60));
        }
        cluster
    }

    fn wait_ready(&self, i: usize, within: Duration) {
        wait_replica_ready(&self.replicas[i], within);
    }

    fn fleet(&self) -> Fleet {
        Fleet { urls: self.replicas.iter().map(Replica::url).collect(), token: ADMIN_TOKEN.to_string() }
    }

    fn sql(&self) -> ag_storage::sqldb::Database {
        ag_storage::sqldb::Database::open_postgres_now(
            &self.be.pg_url,
            &self.control_schema,
            ag_server::control::MIGRATIONS,
            &ag_storage::sqldb::PgOptions::default(),
        )
        .unwrap()
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        let failed = std::thread::panicking();
        for r in &mut self.replicas {
            if failed && let Some(c) = r.child.as_ref() {
                eprintln!("---- {} (pid {}) log tail ----\n{}", r.name, c.id(), r.log_tail());
            }
            r.kill9();
        }
        let _ = ag_storage::meta::drop_schema(&self.be.pg_url, &self.control_schema);
        for repo in &self.repos {
            let clean: String =
                repo.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
            let _ = ag_storage::meta::drop_schema(&self.be.pg_url, &format!("{}_{clean}", self.repo_schema));
        }
    }
}

// ---- HTTP ---------------------------------------------------------------------------------------------------

enum Body {
    None,
    Json(Value),
    Bytes(Vec<u8>),
}

struct Reply {
    status: u16,
    json: Value,
    body: Vec<u8>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .proxy(None)
        .timeout_global(Some(Duration::from_secs(40)))
        .build()
        .into()
}

fn send(method: &str, url: &str, token: &str, body: Body, headers: &[(&str, &str)]) -> Result<Reply, String> {
    let mut b = ureq::http::Request::builder().method(method).uri(url);
    if !token.is_empty() {
        b = b.header("authorization", format!("Bearer {token}"));
    }
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let a = agent();
    let resp = match body {
        Body::None => a.run(b.body(()).unwrap()),
        Body::Json(v) => a.run(b.header("content-type", "application/json").body(v.to_string()).unwrap()),
        Body::Bytes(v) => a.run(b.header("content-type", "application/octet-stream").body(v).unwrap()),
    }
    .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let mut resp = resp;
    let body = resp.body_mut().read_to_vec().map_err(|e| e.to_string())?;
    let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
    Ok(Reply { status, json, body })
}

/// Clients of the installation: they know every replica and, like a load balancer plus a retrying SDK, move to
/// another one when a replica refuses or vanishes.
#[derive(Clone)]
struct Fleet {
    urls: Vec<String>,
    token: String,
}

impl Fleet {
    fn with_token(&self, token: &str) -> Self {
        Self { urls: self.urls.clone(), token: token.to_string() }
    }

    /// One try on one replica, no retry.
    fn once(&self, replica: usize, method: &str, path: &str, body: Body) -> Result<Reply, String> {
        send(method, &format!("{}{path}", self.urls[replica]), &self.token, body, &[])
    }

    /// Starts at `prefer`; retries on transport errors, 5xx and retryable error codes, round-robin, for up to a minute.
    fn call(&self, prefer: usize, method: &str, path: &str, body: impl Fn() -> Body) -> Reply {
        let started = Instant::now();
        let mut i = prefer;
        loop {
            let r = send(method, &format!("{}{path}", self.urls[i % self.urls.len()]), &self.token, body(), &[]);
            match r {
                Ok(rep)
                    if rep.status < 500
                        && !matches!(rep.json["error"]["code"].as_str(), Some("aborted" | "rate_limited")) =>
                {
                    return rep;
                }
                Ok(rep) if started.elapsed() > Duration::from_secs(60) => {
                    panic!("{method} {path}: {} {}", rep.status, rep.json)
                }
                Err(e) if started.elapsed() > Duration::from_secs(60) => panic!("{method} {path}: {e}"),
                _ => {}
            }
            i += 1;
            std::thread::sleep(Duration::from_millis(80));
        }
    }

    fn get(&self, prefer: usize, path: &str) -> Reply {
        self.call(prefer, "GET", path, || Body::None)
    }

    fn post(&self, prefer: usize, path: &str, v: Value) -> Reply {
        self.call(prefer, "POST", path, move || Body::Json(v.clone()))
    }
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

// ---- fixtures ---------------------------------------------------------------------------------------------------

struct Users {
    alice: Fleet,
    carol: Fleet,
}

fn mint(admin: &Fleet, subject: &str, repo: &str, role: &str) -> Fleet {
    let r = admin.post(0, "/v1/roles", json!({"subject": subject, "repo": repo, "role": role}));
    assert_eq!(r.status, 200, "{}", r.json);
    let r = admin.post(0, "/v1/credentials", json!({"subject": subject, "ttl_secs": 3600}));
    assert_eq!(r.status, 200, "{}", r.json);
    admin.with_token(s(&r.json["token"]))
}

fn create_repo(c: &mut Cluster, repo: &str) -> Users {
    let admin = c.fleet();
    let r = admin.post(0, "/v1/repos", json!({"name": repo}));
    assert_eq!(r.status, 200, "create {repo}: {}", r.json);
    c.repos.push(repo.to_string());
    let users = Users {
        alice: mint(&admin, "user:alice", repo, "writer"),
        carol: mint(&admin, "user:carol", repo, "integrator"),
    };
    // Concurrent clients author their changes on top of the current head; on an empty stream each would write its
    // own root and the histories would (rightly) be refused as unrelated. One seed change makes them siblings.
    land_one(&users, 0, repo, "seed", "SEED.txt", b"seed\n");
    users
}

/// Entries every repository from `create_repo` starts with (the seed change).
const SEED: usize = 1;

/// Uploads `objects` through replica `via`.
fn upload(f: &Fleet, via: usize, repo: &str, objects: &[ag_object::RawObject]) {
    for o in objects {
        let bytes = o.encode_envelope();
        let r = f.call(via, "PUT", &format!("/v1/repos/{repo}/objects/{}", o.id()), || Body::Bytes(bytes.clone()));
        assert_eq!(r.status, 200, "upload: {}", r.json);
    }
}

static CLOCK: AtomicUsize = AtomicUsize::new(0);

fn next_ts() -> i64 {
    1_700_000_000_000 + CLOCK.fetch_add(1, Ordering::Relaxed) as i64
}

/// Author a new file on top of the stream's current head, propose it and integrate it; every step retried
/// across replicas with idempotency keys. Returns the new head the integration reported.
fn land_one(users: &Users, via: usize, repo: &str, tag: &str, path: &str, content: &[u8]) -> String {
    let head = s(&users.alice.get(via, &format!("/v1/repos/{repo}/streams/head?stream=main")).json["head"]).to_string();
    let parents: Vec<ChangesetId> = if head.is_empty() { vec![] } else { vec![head.parse().unwrap()] };
    let c = Client::new(repo);
    let (id, objs) = c.commit(parents, "user:alice", &format!("add {path}"), &[(path, content)], next_ts());
    upload(&users.alice, via, repo, &objs);
    let p = users.alice.post(
        via,
        &format!("/v1/repos/{repo}/proposals"),
        json!({"stream": "main", "head": id.to_string(), "title": format!("add {path}"), "idempotency_key": format!("p-{tag}")}),
    );
    assert_eq!(p.status, 200, "propose {tag}: {}", p.json);
    let pid = s(&p.json["id"]).to_string();
    let r = users.carol.post(
        via,
        &format!("/v1/repos/{repo}/proposals/{pid}/integrate"),
        json!({"idempotency_key": format!("int-{tag}")}),
    );
    assert_eq!(r.status, 200, "integrate {tag}: {}", r.json);
    assert_eq!(r.json["outcome"]["status"], "integrated", "integrate {tag}: {}", r.json);
    s(&r.json["outcome"]["new_head"]).to_string()
}

/// The reflog of `main` must be one contiguous chain of exactly `expected` entries, identical on every replica.
fn assert_one_chain(f: &Fleet, repo: &str, expected: usize) {
    let mut seen: Option<Vec<(u64, String, String)>> = None;
    for i in 0..f.urls.len() {
        let r = f.get(i, &format!("/v1/repos/{repo}/reflog?stream=main&limit=1000"));
        assert_eq!(r.status, 200, "reflog via {i}: {}", r.json);
        let entries: Vec<(u64, String, String)> = r.json["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e["version"].as_u64().unwrap(), s(&e["old_head"]).to_string(), s(&e["new_head"]).to_string()))
            .collect();
        assert_eq!(
            entries.len(),
            expected,
            "reflog via replica {i} has {} entries, expected {expected}",
            entries.len()
        );
        // Newest first: versions count down to 1, and every entry starts where the previous one ended.
        for (k, e) in entries.iter().enumerate() {
            assert_eq!(e.0, (expected - k) as u64, "versions are consecutive");
            match entries.get(k + 1) {
                Some(older) => assert_eq!(e.1, older.2, "entry {} does not continue entry {}", e.0, older.0),
                None => assert_eq!(e.1, "", "the first entry starts from an empty stream"),
            }
        }
        let heads: BTreeSet<&String> = entries.iter().map(|e| &e.2).collect();
        assert_eq!(heads.len(), expected, "no head appears twice (nothing was applied twice)");
        match &seen {
            None => seen = Some(entries),
            Some(prev) => assert_eq!(prev, &entries, "replica {i} disagrees about the reflog"),
        }
        let head = f.get(i, &format!("/v1/repos/{repo}/streams/head?stream=main"));
        assert_eq!(head.json["version"].as_u64().unwrap() as usize, expected);
        assert_eq!(s(&head.json["head"]), seen.as_ref().unwrap()[0].2);
    }
    // Verifying the audit chain is an operator's call.
    let admin = f.with_token(ADMIN_TOKEN);
    for i in 0..f.urls.len() {
        let a = admin.get(i, &format!("/v1/repos/{repo}/audit/verify"));
        assert_eq!(a.json["ok"], true, "the audit chain is intact (via {i}): {}", a.json);
    }
}

fn assert_all_files(f: &Fleet, repo: &str, files: &BTreeMap<String, Vec<u8>>) {
    for (k, (path, content)) in files.iter().enumerate() {
        let r = f.get(k, &format!("/v1/repos/{repo}/files/{path}?rev=main"));
        assert_eq!(r.status, 200, "{path}: {}", r.json);
        assert_eq!(&r.body, content, "{path} has the content that was integrated");
    }
}

/// Runs `clients` threads, thread `c` talking mainly to replica `c % replicas`, each landing `per_client` files.
fn run_clients(
    users: &Users,
    repo: &str,
    replicas: usize,
    clients: usize,
    per_client: usize,
    done: &AtomicUsize,
) -> BTreeMap<String, Vec<u8>> {
    let files = std::sync::Mutex::new(BTreeMap::new());
    std::thread::scope(|sc| {
        for c in 0..clients {
            let (files, done) = (&files, done);
            sc.spawn(move || {
                for k in 0..per_client {
                    let path = format!("client{c}/file{k}.txt");
                    let content = format!("client {c}, change {k}\n").into_bytes();
                    // Rotate the entry replica per request so a client is not pinned to one process.
                    land_one(users, repo_replica(c, k, replicas), repo, &format!("{c}-{k}"), &path, &content);
                    files.lock().unwrap().insert(path, content);
                    done.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
    });
    files.into_inner().unwrap()
}

fn repo_replica(client: usize, step: usize, replicas: usize) -> usize {
    (client + step) % replicas
}

// ---- tests ----------------------------------------------------------------------------------------------------------

#[test]
fn concurrent_integrations_through_three_replicas_never_lose_an_update() {
    let _serial = serial();
    let Some(be) = backends() else { return };
    let mut cluster = Cluster::start(be, 3, Options::default());
    let repo = "acme/ha";
    let users = create_repo(&mut cluster, repo);
    let done = AtomicUsize::new(0);
    let files = run_clients(&users, repo, 3, 6, 5, &done);
    assert_eq!(files.len(), 30);
    // Every acknowledged change is in the final tree, and the history is one chain of exactly 30 steps.
    assert_all_files(&users.alice, repo, &files);
    assert_one_chain(&users.alice, repo, 30 + SEED);
    let fsck = cluster.fleet().post(1, &format!("/v1/repos/{repo}/fsck"), json!({}));
    assert_eq!(fsck.json["ok"], true, "{}", fsck.json);
}

#[test]
fn killing_a_replica_mid_flight_loses_nothing_and_applies_nothing_twice() {
    let _serial = serial();
    let Some(be) = backends() else { return };
    let mut cluster = Cluster::start(be, 3, Options::default());
    let repo = "acme/chaos";
    let users = create_repo(&mut cluster, repo);
    let done = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let files = std::thread::scope(|sc| {
        let _stop_on_failure = StopOnDrop(&stop);
        let killer = sc.spawn(|| {
            // Wait until the clients are busy, then SIGKILL replica 1 while requests are in flight.
            while done.load(Ordering::SeqCst) < 6 && !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(20));
            }
            let landed = done.load(Ordering::SeqCst);
            cluster.replicas[1].kill9();
            landed
        });
        let files = run_clients(&users, repo, 3, 6, 5, &done);
        stop.store(true, Ordering::SeqCst);
        let landed_before_kill = killer.join().unwrap();
        assert!(
            (6..30).contains(&landed_before_kill),
            "the kill must hit while clients are busy ({landed_before_kill} of 30 had landed)"
        );
        files
    });
    assert!(cluster.replicas[1].child.is_none(), "replica 1 was killed");
    // The survivors served everything; clients that were talking to replica 1 retried elsewhere with the same
    // idempotency keys, so each change landed exactly once.
    assert_all_files(&users.alice, repo, &files);
    let survivors =
        Fleet { urls: vec![cluster.replicas[0].url(), cluster.replicas[2].url()], token: ADMIN_TOKEN.into() };
    let users =
        Users { alice: users.alice.with_token_urls(&survivors), carol: users.carol.with_token_urls(&survivors) };
    assert_one_chain(&users.alice, repo, 30 + SEED);

    // The dead replica comes back and serves the same history.
    cluster.replicas[1].spawn();
    cluster.wait_ready(1, Duration::from_secs(60));
    let all = cluster.fleet();
    let r = all
        .with_token(&users.alice.token)
        .once(1, "GET", &format!("/v1/repos/{repo}/streams/head?stream=main"), Body::None)
        .unwrap();
    assert_eq!(r.json["version"], 30 + SEED, "{}", r.json);
}

impl Fleet {
    fn with_token_urls(&self, other: &Fleet) -> Fleet {
        Fleet { urls: other.urls.clone(), token: self.token.clone() }
    }
}

#[test]
fn an_idempotency_key_used_on_one_replica_is_honoured_by_the_others() {
    let _serial = serial();
    let Some(be) = backends() else { return };
    let mut cluster = Cluster::start(be, 3, Options::default());
    let f = cluster.fleet();
    let body = json!({"name": "acme/once", "description": "d", "idempotency_key": "k-create"});
    let a = f.once(0, "POST", "/v1/repos", Body::Json(body.clone())).unwrap();
    assert_eq!(a.status, 200, "{}", a.json);
    cluster.repos.push("acme/once".into());
    // The client's retry lands on another replica: the same repository comes back, not "already exists".
    for via in [1, 2] {
        let b = f.once(via, "POST", "/v1/repos", Body::Json(body.clone())).unwrap();
        assert_eq!(b.status, 200, "via {via}: {}", b.json);
        assert_eq!(b.json["name"], "acme/once");
    }
    // The same key with another request is refused everywhere.
    let other = json!({"name": "acme/once", "description": "different", "idempotency_key": "k-create"});
    let c = f.once(2, "POST", "/v1/repos", Body::Json(other)).unwrap();
    assert_eq!(c.status, 422, "{}", c.json);
    assert_eq!(c.json["error"]["code"], "idempotency_key_reused");
    // Without the key it is a plain duplicate.
    let d = f.once(1, "POST", "/v1/repos", Body::Json(json!({"name": "acme/once"}))).unwrap();
    assert_eq!(d.status, 409, "{}", d.json);
    // The repository registered through replica 0 is readable and writable through all of them.
    let users = Users {
        alice: mint(&f, "user:alice", "acme/once", "writer"),
        carol: mint(&f, "user:carol", "acme/once", "integrator"),
    };
    for (i, via) in [2usize, 0, 1].into_iter().enumerate() {
        land_one(&users, via, "acme/once", &format!("x{i}"), &format!("f{i}.txt"), b"x\n");
    }
    assert_one_chain(&users.alice, "acme/once", 3);
}

#[test]
fn a_job_queued_through_one_replica_is_claimed_exactly_once_by_workers_on_the_others() {
    let _serial = serial();
    let Some(be) = backends() else { return };
    let mut cluster = Cluster::start(be, 3, Options::default());
    let repo = "acme/jobs";
    let admin = cluster.fleet();
    assert_eq!(admin.post(0, "/v1/repos", json!({"name": repo})).status, 200);
    cluster.repos.push(repo.into());
    let ci_key = KeyPair::generate().unwrap();
    let policy = json!({
        "trusted_issuers": [{"identity": "service:ci-1", "public_key": ci_key.public().to_hex()}],
        "required_validations": [{"check": "tests", "run": {"argv": ["cargo", "test"], "timeout_secs": 120, "memory_mb": 512}}]
    })
    .to_string();
    let r = admin.call(0, "PUT", &format!("/v1/repos/{repo}/policy"), || {
        Body::Json(json!({"stream": "main", "document": policy}))
    });
    assert_eq!(r.status, 200, "{}", r.json);
    let alice = mint(&admin, "user:alice", repo, "writer");
    let ci = mint(&admin, "service:ci-1", repo, "runner");

    // Workers connect to replicas 1 and 2 (two each) and wait for work with no repository in mind.
    let claimed: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
    let stop = AtomicBool::new(false);
    let jobs = 6usize;
    let first_claim_latency = std::sync::Mutex::new(None::<Duration>);
    let enqueued_at = std::sync::Mutex::new(None::<Instant>);
    std::thread::scope(|sc| {
        for w in 0..4usize {
            let (ci, claimed, stop, key, first, enq) =
                (&ci, &claimed, &stop, &ci_key, &first_claim_latency, &enqueued_at);
            sc.spawn(move || {
                let me = format!("w{w}");
                let via = 1 + (w % 2);
                while !stop.load(Ordering::SeqCst) {
                    let r = ci.once(via, "POST", "/v1/jobs/claim", Body::Json(json!({"worker": me, "lease_ms": 60000, "wait_ms": 3000})));
                    let Ok(r) = r else { continue };
                    let job = &r.json["job"];
                    if r.status != 200 || job.is_null() {
                        continue;
                    }
                    if let Some(t) = *enq.lock().unwrap() {
                        first.lock().unwrap().get_or_insert(t.elapsed());
                    }
                    let id = s(&job["id"]).to_string();
                    claimed.lock().unwrap().push((me.clone(), id.clone()));
                    // Finish it with a signed attestation for exactly this tree, so the job leaves the queue.
                    let v = &job["validation"];
                    let att = Attestation::sign(
                        key,
                        format!("att_{id}"),
                        s(&v["tree_id"]).parse::<TreeId>().unwrap(),
                        s(&v["check"]).to_string(),
                        CheckResult::Pass,
                        v["policy_version"].as_u64().unwrap(),
                        "env-test".into(),
                        Identity::new("service:ci-1").unwrap(),
                        Timestamp::from_millis(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64),
                    )
                    .unwrap();
                    let wire = json!({
                        "id": att.id, "tree_id": att.tree_id.to_string(), "check": att.check, "result": att.result.as_str(),
                        "policy_version": att.policy_version, "environment_digest": att.environment_digest, "issuer": att.issuer.to_string(),
                        "issued_at_ms": att.issued_at.as_millis(), "public_key": att.public_key.to_hex(), "signature": att.signature.to_hex()
                    });
                    let done = ci.call(via, "POST", &format!("/v1/repos/{repo}/jobs/{id}/complete"), || Body::Json(json!({"worker": me, "attestation": wire})));
                    assert_eq!(done.status, 200, "complete: {}", done.json);
                }
            });
        }
        std::thread::sleep(Duration::from_millis(700)); // let the workers start waiting
        // Six independent candidates, queued by asking replica 0 (the job rows are written there).
        *enqueued_at.lock().unwrap() = Some(Instant::now());
        let c = Client::new(repo);
        for n in 0..jobs {
            let (id, objs) = c.commit(
                vec![],
                "user:alice",
                &format!("c{n}"),
                &[("f.txt", format!("job {n}\n").as_bytes())],
                next_ts(),
            );
            upload(&alice, 0, repo, &objs);
            let p = alice.post(
                0,
                &format!("/v1/repos/{repo}/proposals"),
                json!({"stream": "main", "head": id.to_string(), "title": format!("c{n}")}),
            );
            let pid = s(&p.json["id"]).to_string();
            let prep = alice.post(
                0,
                &format!("/v1/repos/{repo}/proposals/{pid}/prepare"),
                json!({"enqueue_validations": true}),
            );
            assert_eq!(prep.status, 200, "{}", prep.json);
            assert_eq!(prep.json["jobs"].as_array().map(Vec::len), Some(1), "{}", prep.json);
        }
        // Wait for all of them to be done.
        let t = Instant::now();
        while claimed.lock().unwrap().len() < jobs && t.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(500)); // a double claim would show up now
        stop.store(true, Ordering::SeqCst);
    });
    let claimed = claimed.into_inner().unwrap();
    let ids: BTreeSet<&String> = claimed.iter().map(|(_, id)| id).collect();
    assert_eq!(claimed.len(), jobs, "{claimed:?}");
    assert_eq!(ids.len(), jobs, "every job was claimed exactly once: {claimed:?}");
    let workers: BTreeSet<&String> = claimed.iter().map(|(w, _)| w).collect();
    println!("claims by worker: {claimed:?}; first claim after {:?}", first_claim_latency.lock().unwrap());
    assert!(!workers.is_empty());
    // The authoritative record, read through the replica that took no part in claiming (0): all succeeded, one attempt each.
    let list = alice.get(0, &format!("/v1/repos/{repo}/jobs?limit=100"));
    let jobs_json = list.json["jobs"].as_array().unwrap();
    assert_eq!(jobs_json.len(), jobs);
    for j in jobs_json {
        assert_eq!(j["state"], "succeeded", "{j}");
        assert_eq!(j["attempts"], 1, "claimed once: {j}");
    }
    let lat = first_claim_latency.lock().unwrap().unwrap();
    assert!(lat < Duration::from_secs(10), "the first claim took {lat:?}");
}

/// A running SSE connection: events arrive on a channel as `(received_at, id, event, data)`.
struct Sse {
    rx: mpsc::Receiver<(Instant, String, String, Value)>,
}

fn sse(url: &str, token: &str, headers: &[(&str, &str)]) -> Sse {
    let (tx, rx) = mpsc::channel();
    let (url, token) = (url.to_string(), token.to_string());
    let headers: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    std::thread::spawn(move || {
        let a: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .proxy(None)
            .timeout_global(Some(Duration::from_secs(120)))
            .build()
            .into();
        let mut b =
            ureq::http::Request::builder().method("GET").uri(&url).header("authorization", format!("Bearer {token}"));
        for (k, v) in &headers {
            b = b.header(k.as_str(), v.as_str());
        }
        let resp = match a.run(b.body(()).unwrap()) {
            Ok(r) => r,
            Err(e) => return eprintln!("sse {url}: {e}"),
        };
        if resp.status().as_u16() != 200 {
            let status = resp.status();
            let body = resp.into_body().read_to_string().unwrap_or_default();
            return eprintln!("sse {url}: {status} {body}");
        }
        let reader = BufReader::new(resp.into_body().into_reader());
        let (mut id, mut event, mut data) = (String::new(), String::new(), String::new());
        for line in reader.lines() {
            let Ok(line) = line else { return };
            if let Some(v) = line.strip_prefix("id:") {
                id = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("event:") {
                event = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("data:") {
                data = v.trim().to_string();
            } else if line.is_empty() && !event.is_empty() {
                let v = serde_json::from_str(&data).unwrap_or(Value::Null);
                if tx.send((Instant::now(), std::mem::take(&mut id), std::mem::take(&mut event), v)).is_err() {
                    return;
                }
                data.clear();
            }
        }
    });
    Sse { rx }
}

impl Sse {
    fn next_matching(
        &self,
        within: Duration,
        pred: impl Fn(&str, &Value) -> bool,
    ) -> Option<(Instant, String, String, Value)> {
        let t = Instant::now();
        while let Some(left) = within.checked_sub(t.elapsed()) {
            match self.rx.recv_timeout(left) {
                Ok(ev) if pred(&ev.2, &ev.3) => return Some(ev),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        None
    }
}

#[test]
fn an_event_caused_through_one_replica_reaches_a_subscriber_on_another_and_replays_anywhere() {
    let _serial = serial();
    let Some(be) = backends() else { return };
    let mut cluster = Cluster::start(be, 3, Options::default());
    let repo = "acme/events";
    let users = create_repo(&mut cluster, repo);
    let f = cluster.fleet();

    // A subscriber on replica 0, following from "now"; a second one on replica 2.
    let url0 = format!("{}/v1/repos/{repo}/events", f.urls[0]);
    let url2 = format!("{}/v1/repos/{repo}/events", f.urls[2]);
    let on0 = sse(&url0, &users.alice.token, &[]);
    let on2 = sse(&url2, &users.alice.token, &[]);
    std::thread::sleep(Duration::from_millis(800)); // both are attached

    // Cause events through replica 1.
    let before = Instant::now();
    let head = land_one(&users, 1, repo, "ev1", "a.txt", b"one\n");
    let ev = on0
        .next_matching(Duration::from_secs(10), |topic, d| {
            topic == "stream.integrate" && s(&d["payload"]["details"]["new_head"]) == head
        })
        .expect("subscriber on replica 0 sees the integration made through replica 1");
    let latency = ev.0.duration_since(before);
    let ev2 = on2
        .next_matching(Duration::from_secs(10), |topic, _| topic == "stream.integrate")
        .expect("subscriber on replica 2 sees it too");
    println!("event seen on replica 0 after {latency:?} (includes authoring, upload, propose and integrate)");
    let seq: u64 = ev.1.parse().unwrap();
    assert_eq!(ev2.1, ev.1, "the same event id on every replica");

    // The delivery latency of a single event is bounded by the poll interval (500 ms) even without notices.
    let t = Instant::now();
    let r =
        users.alice.once(1, "POST", &format!("/v1/repos/{repo}/streams"), Body::Json(json!({"name": "side"}))).unwrap();
    assert_eq!(r.status, 200, "{}", r.json);
    let created =
        on0.next_matching(Duration::from_secs(5), |topic, _| topic == "stream.create").expect("stream.create arrives");
    let lag = created.0.duration_since(t);
    println!("stream.create made through replica 1 reached the subscriber on replica 0 in {lag:?}");
    assert!(lag < Duration::from_secs(2), "event delivery across replicas took {lag:?}");

    // Replay by event id works against any replica and returns the same events.
    let mut replays = Vec::new();
    for i in 0..3 {
        let r = users
            .alice
            .once(i, "GET", &format!("/v1/repos/{repo}/events/replay?after_seq={}&limit=100", seq - 1), Body::None)
            .unwrap();
        assert_eq!(r.status, 200, "{}", r.json);
        let evs: Vec<(u64, String)> = r.json["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e["seq"].as_u64().unwrap(), s(&e["topic"]).to_string()))
            .collect();
        assert_eq!(evs[0].0, seq);
        replays.push(evs);
    }
    assert_eq!(replays[0], replays[1]);
    assert_eq!(replays[1], replays[2]);
    // ...and an SSE reconnect with Last-Event-ID to a different replica resumes right after it.
    let resumed = sse(&url2, &users.alice.token, &[("last-event-id", &(seq - 1).to_string())]);
    let again = resumed.next_matching(Duration::from_secs(10), |_, _| true).expect("replayed event");
    assert_eq!(again.1, replays[0][0].0.to_string());

    // The delivery-lag histogram on the receiving replica saw live events.
    let m = users.alice.once(0, "GET", "/metrics", Body::None).unwrap();
    let text = String::from_utf8_lossy(&m.body);
    let count: f64 = text
        .lines()
        .find_map(|l| l.strip_prefix("agwp_event_delivery_lag_seconds_count "))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0.0);
    assert!(
        count >= 2.0,
        "replica 0 recorded {count} live deliveries:\n{}",
        text.lines().filter(|l| l.contains("lag") || l.contains("cluster")).collect::<Vec<_>>().join("\n")
    );
    assert!(text.contains("agwp_replica_info{") && text.contains("replica=\"replica-0\""), "replica id is exported");
    assert!(text.contains("agwp_cluster_listener_up 1"), "the notice listener is connected");
}

#[test]
fn a_revoked_credential_and_a_revoked_role_stop_working_on_every_replica() {
    let _serial = serial();
    let Some(be) = backends() else { return };
    let mut cluster = Cluster::start(be, 3, Options { bindings_ttl_ms: 3000, ..Options::default() });
    let repo = "acme/revoke";
    let _ = create_repo(&mut cluster, repo);
    let admin = cluster.fleet();

    // A credential minted on replica 0 works on replicas 1 and 2 at once.
    let bob = mint(&admin, "user:bob", repo, "reader");
    for i in 0..3 {
        assert_eq!(bob.once(i, "GET", &format!("/v1/repos/{repo}"), Body::None).unwrap().status, 200, "replica {i}");
    }
    // An open event stream on replica 2.
    let stream = sse(&format!("{}/v1/repos/{repo}/events", admin.urls[2]), &bob.token, &[]);
    std::thread::sleep(Duration::from_millis(600));

    // Revoke the credential through replica 0.
    let creds = admin.get(0, "/v1/credentials?subject=user:bob");
    let cid = s(&creds.json["credentials"][0]["id"]).to_string();
    let t = Instant::now();
    let r = admin.call(0, "DELETE", &format!("/v1/credentials/{cid}"), || Body::None);
    assert_eq!(r.status, 200, "{}", r.json);
    for i in 0..3 {
        let r = bob.once(i, "GET", &format!("/v1/repos/{repo}"), Body::None).unwrap();
        assert_eq!(r.status, 401, "replica {i} must reject a revoked credential at once: {}", r.json);
    }
    println!("credential revocation visible on all replicas {:?} after the call started", t.elapsed());
    // The open stream ends with an error event within the re-check interval (5 s) plus a poll.
    let end = stream.next_matching(Duration::from_secs(12), |topic, _| topic == "error").expect("the stream is cut");
    println!("an open event stream on replica 2 was cut {:?} after the revocation", end.0.duration_since(t));

    // Roles: carol writes through replica 1; revoke her role through replica 0; the bound is bindings_ttl_ms.
    let carol = mint(&admin, "user:carol", repo, "writer");
    let probe = |i: usize| {
        carol
            .once(
                i,
                "POST",
                &format!("/v1/repos/{repo}/streams"),
                Body::Json(json!({"name": format!("s{}", rand_suffix())})),
            )
            .unwrap()
            .status
    };
    assert_eq!(probe(1), 200);
    assert_eq!(probe(2), 200);
    let t = Instant::now();
    let r = admin.call(0, "DELETE", &format!("/v1/roles?subject=user:carol&repo={repo}&role=writer"), || Body::None);
    assert!(r.status == 200 || r.status == 204, "{}", r.json);
    let mut effective = [None; 3];
    while t.elapsed() < Duration::from_secs(8) && effective.iter().any(Option::is_none) {
        for (i, e) in effective.iter_mut().enumerate() {
            if e.is_none() && matches!(probe(i), 403 | 404) {
                *e = Some(t.elapsed());
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    println!("role revocation effective after {effective:?}");
    for (i, e) in effective.iter().enumerate() {
        let e = e.unwrap_or_else(|| panic!("replica {i} still honours a revoked role after 8 s"));
        assert!(
            e < Duration::from_millis(3000 + 1000),
            "replica {i}: {e:?} exceeds the documented bound (bindings_ttl_ms + 1 s)"
        );
    }
}

/// Sets the flag when dropped, also while unwinding: a failing test must stop its helper threads instead of
/// hanging in `thread::scope`.
struct StopOnDrop<'a>(&'a AtomicBool);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn rand_suffix() -> String {
    ag_storage::testing::unique("x")
}

#[test]
fn a_rolling_restart_never_makes_the_service_unavailable_and_loses_no_write() {
    let _serial = serial();
    let Some(be) = backends() else { return };
    let mut cluster = Cluster::start(be, 3, Options { drain_secs: 2, ..Options::default() });
    let repo = "acme/roll";
    let users = create_repo(&mut cluster, repo);
    let f = cluster.fleet();
    let stop = AtomicBool::new(false);
    let reads = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let mut max_gap = Duration::ZERO;
    let files = std::thread::scope(|sc| {
        let _stop_on_failure = StopOnDrop(&stop);
        // Readers: a steady stream of reads through a failing-over client; none may fail in the end.
        let reader = sc.spawn(|| {
            let mut last_ok = Instant::now();
            let mut gap = Duration::ZERO;
            let mut i = 0;
            while !stop.load(Ordering::SeqCst) {
                let r = users.alice.get(i, &format!("/v1/repos/{repo}"));
                assert_eq!(r.status, 200, "{}", r.json);
                reads.fetch_add(1, Ordering::SeqCst);
                gap = gap.max(last_ok.elapsed());
                last_ok = Instant::now();
                i += 1;
                std::thread::sleep(Duration::from_millis(15));
            }
            gap
        });
        // Writers: three clients landing files while the restart rolls through.
        let writers = sc.spawn(|| run_clients(&users, repo, 3, 3, 8, &done));
        std::thread::sleep(Duration::from_millis(800));
        for i in 0..3 {
            // Draining: SIGTERM, /readyz turns 503 at once while the replica still answers, then it exits cleanly.
            let r = &mut cluster.replicas[i];
            let url = r.url();
            let child_id = r.child.as_ref().unwrap().id();
            let _ = Command::new("kill").args(["-TERM", &child_id.to_string()]).status();
            let t = Instant::now();
            let mut saw_draining = false;
            while t.elapsed() < Duration::from_millis(1500) {
                if let Ok(rep) = send("GET", &format!("{url}/readyz"), "", Body::None, &[])
                    && rep.status == 503
                    && String::from_utf8_lossy(&rep.body).contains("draining")
                {
                    saw_draining = true;
                    // Still serving while it drains.
                    let h = send("GET", &format!("{url}/healthz"), "", Body::None, &[]).unwrap();
                    assert_eq!(h.status, 200);
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            assert!(saw_draining, "replica {i} should report draining right after SIGTERM");
            let st = loop {
                let c = r.child.as_mut().unwrap();
                if let Some(st) = c.try_wait().unwrap() {
                    break st;
                }
                assert!(t.elapsed() < Duration::from_secs(30), "replica {i} did not exit after SIGTERM");
                std::thread::sleep(Duration::from_millis(50));
            };
            r.child = None;
            assert!(st.success(), "replica {i} exited with {st:?}");
            r.spawn();
            let (name, log) = (r.name.clone(), r.log.clone());
            let _ = (name, log);
            // wait for it to be ready again before taking the next one down
            let t = Instant::now();
            loop {
                if let Ok(rep) = send("GET", &format!("{url}/readyz"), "", Body::None, &[])
                    && rep.status == 200
                {
                    break;
                }
                assert!(t.elapsed() < Duration::from_secs(60), "replica {i} did not come back");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let files = writers.join().unwrap();
        stop.store(true, Ordering::SeqCst);
        max_gap = reader.join().unwrap();
        files
    });
    println!(
        "{} reads served during the rolling restart; longest gap between successes {max_gap:?}",
        reads.load(Ordering::SeqCst)
    );
    assert!(reads.load(Ordering::SeqCst) > 50);
    assert!(max_gap < Duration::from_secs(10), "reads stalled for {max_gap:?}");
    assert_eq!(files.len(), 24);
    assert_all_files(&users.alice, repo, &files);
    assert_one_chain(&f, repo, 24 + SEED);
}

#[test]
fn replicas_started_together_migrate_once_and_refuse_a_newer_schema() {
    let _serial = serial();
    let Some(be) = backends() else { return };
    // Five replicas race to create and migrate the same control schema.
    let mut cluster = Cluster::start(be, 5, Options::default());
    let sql = cluster.sql();
    assert_eq!(sql.stored_version().unwrap(), sql.supported_version());
    let version = sql.supported_version();
    assert!(version >= 2, "the control schema has at least the HA migration");
    // Each one reports the same schema version and its own identity.
    for i in 0..5 {
        let m = send("GET", &format!("{}/metrics", cluster.replicas[i].url()), "", Body::None, &[]).unwrap();
        let text = String::from_utf8_lossy(&m.body).to_string();
        assert!(text.contains(&format!("agwp_control_schema_version {version}")), "{text}");
        assert!(text.contains(&format!("replica=\"replica-{i}\"")));
    }
    cluster.replicas[4].kill9();

    // A schema written by a newer release: a replica refuses to start instead of misreading it.
    sql.execute("UPDATE meta SET value = '99' WHERE key = 'schema'", &[]).unwrap();
    let r = &mut cluster.replicas[4];
    r.spawn();
    let t = Instant::now();
    let status = loop {
        if let Some(st) = r.child.as_mut().unwrap().try_wait().unwrap() {
            break st;
        }
        assert!(t.elapsed() < Duration::from_secs(30), "the replica should have refused to start");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(!status.success());
    let log = std::fs::read_to_string(&r.log).unwrap();
    assert!(log.contains("schema version 99") && log.contains("newer than this server supports"), "{log}");
    sql.execute(&format!("UPDATE meta SET value = '{version}' WHERE key = 'schema'"), &[]).unwrap();
    r.child = None;
    r.spawn();
    cluster.wait_ready(4, Duration::from_secs(60));
}

#[test]
fn readiness_follows_the_control_database_and_liveness_does_not() {
    let _serial = serial();
    let Some(be) = backends() else { return };
    // The replicas connect as their own role, so the test can lock exactly them out of PostgreSQL.
    let role = ag_storage::testing::unique("agha_role");
    let admin = ag_storage::sqldb::Database::open_postgres_now(
        &be.pg_url,
        &ag_storage::testing::unique("agha_adm"),
        ag_server::control::MIGRATIONS,
        &ag_storage::sqldb::PgOptions::default(),
    )
    .unwrap();
    admin.execute(&format!("CREATE ROLE {role} LOGIN SUPERUSER"), &[]).unwrap();
    let replica_url = be.pg_url.replacen("postgres@", &format!("{role}@"), 1);
    assert_ne!(replica_url, be.pg_url, "the test URL names the postgres user");
    let cluster = Cluster::start(be, 2, Options { pg_url: Some(replica_url), ..Options::default() });
    let url = cluster.replicas[0].url();
    let token = mint(&cluster.fleet(), "user:dana", "*", "reader");
    let get = |path: &str, t: &str| send("GET", &format!("{url}{path}"), t, Body::None, &[]).unwrap();
    assert_eq!(get("/readyz", "").status, 200);
    assert_eq!(get("/v1/whoami", &token.token).status, 200);

    // Lock the replicas out: no new connections, existing ones terminated.
    admin.execute(&format!("ALTER ROLE {role} NOLOGIN"), &[]).unwrap();
    admin
        .query(&format!("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename = '{role}'"), &[])
        .unwrap();
    let t = Instant::now();
    let reason = loop {
        let r = get("/readyz", "");
        if r.status == 503 {
            break String::from_utf8_lossy(&r.body).to_string();
        }
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "readiness should fail when the control database is unreachable"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    println!("not ready after {:?}: {reason}", t.elapsed());
    assert_eq!(reason, "control database unavailable");
    // Liveness is unaffected (a restart would not help), and requests that need the database fail as
    // retryable `unavailable`, not as an opaque internal error.
    assert_eq!(get("/healthz", "").status, 200);
    let r = get("/v1/whoami", &token.token);
    assert_eq!(r.status, 503, "{}", r.json);
    assert_eq!(r.json["error"]["code"], "unavailable");
    assert_eq!(r.json["error"]["retryable"], true);

    // The database returns: the replica recovers by itself, without a restart.
    admin.execute(&format!("ALTER ROLE {role} LOGIN"), &[]).unwrap();
    let t = Instant::now();
    loop {
        if get("/readyz", "").status == 200 && get("/v1/whoami", &token.token).status == 200 {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(30), "the replica did not recover");
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("ready again {:?} after the database came back", t.elapsed());
    drop(cluster);
    let _ = admin.execute(&format!("DROP OWNED BY {role}"), &[]);
    let _ = admin.execute(&format!("DROP ROLE {role}"), &[]);
}

/// Kills what a migration test started and drops the schemas it created, also when it fails.
struct Scratch {
    pg_url: String,
    schemas: Vec<String>,
    replicas: Vec<Replica>,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let failed = std::thread::panicking();
        for r in &mut self.replicas {
            if failed && r.child.is_some() {
                eprintln!("---- {} log tail ----\n{}", r.name, r.log_tail());
            }
            r.kill9();
        }
        for schema in &self.schemas {
            let _ = ag_storage::meta::drop_schema(&self.pg_url, schema);
        }
    }
}

/// The ids of every credential the administrator can list, subject by subject (the listing has no "all").
fn credential_ids(admin: &Fleet, via: usize) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for subject in ["user:alice", "user:carol", "agent:refactor-bot", "user:erin", "user:mallory"] {
        let r = admin.get(via, &format!("/v1/credentials?subject={subject}"));
        assert_eq!(r.status, 200, "{}", r.json);
        for c in r.json["credentials"].as_array().unwrap() {
            ids.insert(s(&c["id"]).to_string());
        }
    }
    ids
}

/// `ag-server admin --config <cfg> <args>`; returns (success, stdout + stderr).
fn admin_cli(cfg: &std::path::Path, pg_url: &str, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_ag-server"))
        .arg("admin")
        .arg("--config")
        .arg(cfg)
        .args(args)
        .env_remove("AG_SERVER_CONFIG")
        .env("AG_TEST_POSTGRES_URL", pg_url)
        .env("NO_PROXY", "127.0.0.1,localhost")
        .output()
        .expect("run ag-server admin");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// The upgrade path of docs/operations/high-availability.md, with real data: a single server whose control plane
/// is the SQLite file (credentials, roles, an agent with a registered key, a repository with history) is stopped,
/// `admin migrate-control --from sqlite --to postgres` moves the control plane, and two fresh replicas on empty
/// data directories serve everything the single server knew.
#[test]
fn migrate_control_turns_a_single_server_into_replicas_without_losing_credentials_roles_repos_or_agent_keys() {
    let _serial = serial();
    let Some(be) = backends() else { return };
    let root = tempfile::tempdir().unwrap();
    let control_schema = ag_storage::testing::unique("agha_ctl");
    let repo_schema = ag_storage::testing::unique("agha");
    let s3_prefix = ag_storage::testing::unique("ha");
    let repo = "acme/legacy";
    let mut scratch = Scratch { pg_url: be.pg_url.clone(), schemas: vec![control_schema.clone()], replicas: vec![] };
    scratch.schemas.push(format!("{repo_schema}_acme_legacy"));
    scratch.schemas.push(format!("{repo_schema}_acme_other"));

    let replica = |name: &str, control: Option<&str>| {
        let dir = root.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let port = free_port();
        let cfg = dir.join("ag-server.toml");
        let data = dir.join("data");
        let text = config_text(
            &be,
            &Shape {
                port,
                data: &data,
                name,
                drain_secs: 0,
                control: control.map(|c| (c, 5000)),
                repo_schema: &repo_schema,
                s3_prefix: &s3_prefix,
            },
        );
        std::fs::write(&cfg, text).unwrap();
        Replica {
            name: name.into(),
            pg_url: be.pg_url.clone(),
            port,
            log: dir.join("server.log"),
            dir,
            cfg,
            child: None,
        }
    };

    // ---- the single server, control plane in <data_dir>/control.db ----------------------------------------------
    let mut legacy = replica("legacy", None);
    legacy.spawn();
    wait_replica_ready(&legacy, Duration::from_secs(60));
    let admin = Fleet { urls: vec![legacy.url()], token: ADMIN_TOKEN.into() };
    assert_eq!(admin.post(0, "/v1/repos", json!({"name": repo})).status, 200);
    let alice = mint(&admin, "user:alice", repo, "writer");
    let carol = mint(&admin, "user:carol", repo, "integrator");
    let users = Users { alice, carol };
    land_one(&users, 0, repo, "seed", "SEED.txt", b"seed\n");
    // An agent with a registered key.
    let bot = mint(&admin, "agent:refactor-bot", repo, "writer");
    let bot_key = KeyPair::generate().unwrap();
    let r = bot.post(
        0,
        &format!("/v1/repos/{repo}/agents"),
        json!({"name": "refactor-bot", "public_key": bot_key.public().to_hex()}),
    );
    assert_eq!(r.status, 200, "{}", r.json);
    // A credential restricted to one repository, and one that was revoked before the move.
    let erin = mint(&admin, "user:erin", "*", "reader");
    let scoped =
        admin.post(0, "/v1/credentials", json!({"subject": "user:erin", "ttl_secs": 3600, "scope": {"repos": [repo]}}));
    assert_eq!(scoped.status, 200, "{}", scoped.json);
    let scoped = admin.with_token(s(&scoped.json["token"]));
    let mallory = mint(&admin, "user:mallory", "*", "reader");
    let listed = admin.get(0, "/v1/credentials?subject=user:mallory");
    let mallory_id = s(&listed.json["credentials"][0]["id"]).to_string();
    assert_eq!(admin.call(0, "DELETE", &format!("/v1/credentials/{mallory_id}"), || Body::None).status, 200);
    assert_eq!(mallory.once(0, "GET", "/v1/whoami", Body::None).unwrap().status, 401);

    let creds_before = credential_ids(&admin, 0);
    let roles_before = admin.get(0, "/v1/roles").json["bindings"].as_array().unwrap().len();
    assert!(creds_before.len() >= 5 && roles_before >= 5, "{} credentials, {roles_before} roles", creds_before.len());
    let head_before = admin.get(0, &format!("/v1/repos/{repo}/streams/head?stream=main")).json;
    assert_eq!(head_before["version"], 1, "{head_before}");
    let legacy_data = legacy.dir.join("data");
    assert!(legacy_data.join("control.db").is_file(), "the single server keeps its control plane in a SQLite file");
    // Stop it the way an operator would.
    assert!(legacy.term(Duration::from_secs(20)).is_some_and(|st| st.success()), "the server stops on SIGTERM");

    // ---- migrate-control ---------------------------------------------------------------------------------------
    // The administrator's configuration: the old data directory (where control.db is) plus the [control] section.
    let cfg = root.path().join("migrate.toml");
    std::fs::write(
        &cfg,
        config_text(
            &be,
            &Shape {
                port: 0,
                data: &legacy_data,
                name: "migrate",
                drain_secs: 0,
                control: Some((&control_schema, 5000)),
                repo_schema: &repo_schema,
                s3_prefix: &s3_prefix,
            },
        ),
    )
    .unwrap();
    let (ok, out) = admin_cli(&cfg, &be.pg_url, &["migrate-control", "--from", "sqlite", "--to", "sqlite"]);
    assert!(!ok && out.contains("same backend"), "{out}");
    let (ok, out) = admin_cli(&cfg, &be.pg_url, &["migrate-control", "--from", "sqlite", "--to", "postgres"]);
    assert!(ok, "{out}");
    // The listing hides the revoked credential; the move carries it (so it stays revoked).
    let creds_dumped = creds_before.len() + 1;
    assert!(
        out.contains(&format!("moved {creds_dumped} credential(s), {roles_before} role binding(s), 1 repositor(ies)")),
        "{out}"
    );
    // The target is no longer empty: a second run refuses, `--replace` overwrites.
    let (ok, out) = admin_cli(&cfg, &be.pg_url, &["migrate-control", "--from", "sqlite", "--to", "postgres"]);
    assert!(!ok, "a second migration must not silently overwrite:\n{out}");
    let (ok, out) =
        admin_cli(&cfg, &be.pg_url, &["migrate-control", "--from", "sqlite", "--to", "postgres", "--replace"]);
    assert!(ok, "{out}");
    // The source is untouched (rollback = start the old configuration again).
    assert!(legacy_data.join("control.db").is_file());

    // ---- two replicas on empty data directories ---------------------------------------------------------------------
    // Fresh machines: nothing of the single server's disk comes along. (A replica creates the small local marker of a
    // repository it opens, from the configuration stored in the registry, but no history.)
    for name in ["replica-0", "replica-1"] {
        assert!(!root.path().join(name).exists());
    }
    scratch.replicas.push(replica("replica-0", Some(&control_schema)));
    scratch.replicas.push(replica("replica-1", Some(&control_schema)));
    for r in &mut scratch.replicas {
        r.spawn();
    }
    for r in &scratch.replicas {
        wait_replica_ready(r, Duration::from_secs(60));
    }
    let fleet = Fleet { urls: scratch.replicas.iter().map(Replica::url).collect(), token: ADMIN_TOKEN.into() };
    // The same bearer tokens as before the move, now presented to the replicas.
    let users = Users { alice: users.alice.with_token_urls(&fleet), carol: users.carol.with_token_urls(&fleet) };
    let (bot, erin, scoped, mallory) = (
        bot.with_token_urls(&fleet),
        erin.with_token_urls(&fleet),
        scoped.with_token_urls(&fleet),
        mallory.with_token_urls(&fleet),
    );

    for via in 0..2 {
        // Every credential minted before the move still authenticates, as the same identity.
        for (who, f) in [
            ("user:alice", &users.alice),
            ("user:carol", &users.carol),
            ("agent:refactor-bot", &bot),
            ("user:erin", &erin),
        ] {
            let r = f.once(via, "GET", "/v1/whoami", Body::None).unwrap();
            assert_eq!(r.status, 200, "{who} via replica {via}: {}", r.json);
            assert_eq!(r.json["identity"], who);
        }
        // ...and the revoked one still does not.
        assert_eq!(
            mallory.once(via, "GET", "/v1/whoami", Body::None).unwrap().status,
            401,
            "revocation survived the move"
        );
    }
    let creds_after = credential_ids(&fleet, 1);
    assert_eq!(creds_after, creds_before, "the same credentials, by id");
    assert_eq!(fleet.get(0, "/v1/roles").json["bindings"].as_array().unwrap().len(), roles_before);

    // Roles still decide: alice (writer) may not administer; erin's credential is restricted to one repository.
    assert_eq!(
        users.alice.once(1, "GET", "/v1/credentials", Body::None).unwrap().status,
        200,
        "own credentials are listable"
    );
    assert_eq!(fleet.post(0, "/v1/repos", json!({"name": "acme/other"})).status, 200);
    assert_eq!(scoped.once(1, "GET", &format!("/v1/repos/{repo}"), Body::None).unwrap().status, 200);
    let denied = scoped.once(1, "GET", "/v1/repos/acme/other", Body::None).unwrap();
    assert!(matches!(denied.status, 403 | 404), "the scope survived the move: {} {}", denied.status, denied.json);
    assert_eq!(erin.once(0, "GET", "/v1/repos/acme/other", Body::None).unwrap().status, 200, "unscoped erin reads it");

    // The repository registry: the repository is served by the replicas from PostgreSQL + S3, history intact.
    let names: Vec<String> =
        fleet.get(0, "/v1/repos").json["repos"].as_array().unwrap().iter().map(|r| s(&r["name"]).to_string()).collect();
    assert!(names.iter().any(|n| n == repo), "{names:?}");
    let head_after = fleet.get(1, &format!("/v1/repos/{repo}/streams/head?stream=main")).json;
    assert_eq!(head_after["head"], head_before["head"]);
    let f = fleet.get(0, &format!("/v1/repos/{repo}/files/SEED.txt?rev=main"));
    assert_eq!(f.body, b"seed\n");

    // The agent's key is still registered (it lives with the repository, which the move did not touch).
    let agents = bot.get(1, &format!("/v1/repos/{repo}/agents"));
    let found = agents.json["agents"].as_array().unwrap().iter().find(|a| a["name"] == "refactor-bot").cloned();
    let found = found.unwrap_or_else(|| panic!("the agent is gone: {}", agents.json));
    assert_eq!(found["public_key"], bot_key.public().to_hex());

    // And the installation keeps working: new writes through the replicas continue the same history.
    land_one(&users, 1, repo, "after", "AFTER.txt", b"after the move\n");
    assert_one_chain(&users.alice, repo, 2);
}
