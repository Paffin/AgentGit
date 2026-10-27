//! The load scenarios.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Barrier, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ag_types::{ChangesetId, Namespace};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::json;

use super::Plan;
use super::client::{CallError, Proto, Session, build_changeset};
use super::server::{ProcUse, Server, ServerSettings, proc_use};
use crate::stats::Summary;

/// What one scenario measured.
#[derive(Serialize)]
pub struct ScenarioResult {
    pub name: String,
    pub protocol: Proto,
    pub description: String,
    pub clients: usize,
    pub elapsed_secs: f64,
    /// Units of work that finished with a definite answer from the server (see `description`).
    pub completed: u64,
    pub per_second: f64,
    /// Latency per step, in the order the steps happen.
    pub steps: Vec<(String, Summary)>,
    pub outcomes: BTreeMap<String, u64>,
    pub errors: BTreeMap<String, u64>,
    pub sample_errors: Vec<String>,
    pub notes: BTreeMap<String, String>,
    /// Requests the rate limit configuration admits over the run (burst + rate x time), for the `ratelimit` scenario.
    pub allowed_requests: Option<f64>,
    /// CPU used by the server during the load, and its peak memory so far (Linux only).
    pub server: Option<ProcUse>,
    pub settings: ServerSettings,
}

/// Latencies, outcomes and failures collected by one thread (merged at the end).
#[derive(Default)]
struct Rec {
    steps: HashMap<&'static str, Vec<Duration>>,
    outcomes: BTreeMap<String, u64>,
    errors: BTreeMap<String, u64>,
    samples: Vec<String>,
    completed: u64,
    // Acknowledged writes, kept separately from timing samples so the final server state can be checked.
    integrated_per_stream: BTreeMap<String, u64>,
    expected_files: BTreeMap<(String, String), Vec<u8>>,
}

impl Rec {
    fn step(&mut self, name: &'static str, d: Duration) {
        self.steps.entry(name).or_default().push(d);
    }

    fn fail(&mut self, step: &str, e: &CallError) {
        let key = if e.http_status != 0 {
            format!("{step}: {} (HTTP {})", e.code, e.http_status)
        } else {
            format!("{step}: {}", e.code)
        };
        let n = self.errors.entry(key.clone()).or_default();
        *n += 1;
        if *n == 1 && self.samples.len() < 5 {
            self.samples.push(format!("{key}: {}", e.message.chars().take(160).collect::<String>()));
        }
    }

    fn merge(&mut self, o: Rec) {
        for (k, mut v) in o.steps {
            self.steps.entry(k).or_default().append(&mut v);
        }
        for (k, v) in o.outcomes {
            *self.outcomes.entry(k).or_default() += v;
        }
        for (k, v) in o.errors {
            *self.errors.entry(k).or_default() += v;
        }
        for s in o.samples {
            if self.samples.len() < 5 && !self.samples.contains(&s) {
                self.samples.push(s);
            }
        }
        self.completed += o.completed;
        for (stream, count) in o.integrated_per_stream {
            *self.integrated_per_stream.entry(stream).or_default() += count;
        }
        // Non-conflict workers own disjoint paths; within a worker this contains its last successful write.
        self.expected_files.extend(o.expected_files);
    }
}

const ORDER: [&str; 8] = ["head", "upload", "propose", "integrate", "cycle", "admitted", "rejected", "read"];

#[allow(clippy::too_many_arguments)]
fn finish(
    name: &str,
    proto: Proto,
    description: &str,
    clients: usize,
    elapsed: Duration,
    rec: Rec,
    server: &Server,
    cpu_before: Option<ProcUse>,
) -> ScenarioResult {
    let mut steps: Vec<(String, Summary)> = Vec::new();
    for k in ORDER {
        if let Some(v) = rec.steps.get(k) {
            steps.push((k.to_string(), Summary::of(v)));
        }
    }
    for (k, v) in &rec.steps {
        if !ORDER.contains(k) {
            steps.push((k.to_string(), Summary::of(v)));
        }
    }
    let after = server.pid().and_then(proc_use);
    let used = after.map(|a| ProcUse {
        cpu_secs: a.cpu_secs - cpu_before.map_or(0.0, |b| b.cpu_secs),
        peak_rss_mib: a.peak_rss_mib,
    });
    ScenarioResult {
        name: name.into(),
        protocol: proto,
        description: description.into(),
        clients,
        elapsed_secs: elapsed.as_secs_f64(),
        completed: rec.completed,
        per_second: rec.completed as f64 / elapsed.as_secs_f64().max(0.001),
        steps,
        outcomes: rec.outcomes,
        errors: rec.errors,
        sample_errors: rec.samples,
        notes: BTreeMap::new(),
        allowed_requests: None,
        server: used,
        settings: server.settings.clone(),
    }
}

// ---- fixture ---------------------------------------------------------------------------------------------

struct Fixture {
    server: Server,
    admin: Session,
    repo: String,
    namespace: Namespace,
}

impl Fixture {
    fn start(bin: &Path, plan: &Plan, settings: ServerSettings) -> Result<Self> {
        let server = Server::start(bin, settings, plan.data_parent.as_deref())?;
        let admin = Session::http_only(&server.url, &server.admin_token);
        let repo = "bench/app".to_string();
        let r = admin
            .http("POST", "/v1/repos", Some(json!({"name": repo})))
            .map_err(|e| anyhow::anyhow!("{}", e.message))?;
        ensure!(r.status == 200, "creating the repository: HTTP {} {}", r.status, r.json);
        let namespace = Namespace::new(repo.as_str())?;
        Ok(Self { server, admin, repo, namespace })
    }

    /// A user with `role` on the repository and a token for it.
    fn user(&self, subject: &str, role: &str) -> Result<String> {
        let call = |path: &str, body: serde_json::Value| -> Result<serde_json::Value> {
            let r = self.admin.http("POST", path, Some(body)).map_err(|e| anyhow::anyhow!("{}", e.message))?;
            ensure!(r.status == 200, "{path}: HTTP {} {}", r.status, r.json);
            Ok(r.json)
        };
        call("/v1/roles", json!({"subject": subject, "repo": self.repo, "role": role}))?;
        let c = call("/v1/credentials", json!({"subject": subject, "ttl_secs": 3600}))?;
        Ok(c["token"].as_str().context("credential token")?.to_string())
    }

    fn stream(&self, name: &str) -> Result<()> {
        self.admin
            .create_stream(&self.repo, name)
            .map_err(|e| anyhow::anyhow!("create stream {name}: {} {}", e.code, e.message))
    }

    fn pair(&self, i: usize, proto: Proto, rt: &tokio::runtime::Handle) -> Result<(Session, Session)> {
        let w = Session::connect(proto, &self.server.url, &self.user(&format!("user:w{i}"), "writer")?, rt)?;
        let g = Session::connect(proto, &self.server.url, &self.user(&format!("user:i{i}"), "integrator")?, rt)?;
        Ok((w, g))
    }
}

// ---- the publish -> propose -> integrate cycle ---------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The worker owns its stream and builds on its own previous result (no other writer).
    Chain,
    /// Several workers on one stream, each changing its own files, building on the head it just read.
    Contend,
    /// Several workers on one stream, all changing the same file.
    Conflict,
}

struct Worker {
    id: usize,
    repo: String,
    stream: String,
    mode: Mode,
    writer: Session,
    integrator: Session,
    namespace: Namespace,
}

fn payload(worker: usize, iter: u64, size: usize) -> Vec<u8> {
    let line = format!("worker {worker} change {iter}\n");
    line.repeat(size / line.len() + 1).into_bytes()
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn backoff(e: &CallError) {
    let ms = if e.retry_after_ms > 0 { e.retry_after_ms.min(500) as u64 } else { 20 };
    std::thread::sleep(Duration::from_millis(ms));
}

impl Worker {
    /// Runs cycles until `deadline`. Every failed call is recorded and retried as a new cycle after the server's hint.
    fn run(&self, deadline: Instant) -> Rec {
        let mut rec = Rec::default();
        let mut last: Option<String> = None;
        let mut iter = 0u64;
        while Instant::now() < deadline {
            iter += 1;
            let t_cycle = Instant::now();
            // Where to build: the head just read (shared streams) or our own last result.
            let parent = if self.mode == Mode::Chain && last.is_some() {
                last.clone()
            } else {
                let t = Instant::now();
                match self.writer.head(&self.repo, &self.stream) {
                    Ok((h, _)) => {
                        rec.step("head", t.elapsed());
                        (!h.is_empty()).then_some(h)
                    }
                    Err(e) => {
                        rec.fail("head", &e);
                        backoff(&e);
                        continue;
                    }
                }
            };
            let files = if self.mode == Mode::Conflict {
                vec![("shared.txt".to_string(), payload(self.id, iter, 2048))]
            } else {
                vec![
                    (format!("w{}/src/f{}.txt", self.id, iter % 8), payload(self.id, iter, 2048)),
                    (format!("w{}/notes.txt", self.id), payload(self.id, iter, 512)),
                ]
            };
            let parents: Vec<ChangesetId> = parent.iter().filter_map(|h| h.parse().ok()).collect();
            let (cs, objects) = match build_changeset(
                &self.namespace,
                parents,
                &format!("user:w{}", self.id),
                &format!("change {iter}"),
                &files,
            ) {
                Ok(x) => x,
                Err(e) => {
                    rec.errors.entry(format!("build: {e}")).and_modify(|n| *n += 1).or_insert(1);
                    break;
                }
            };
            let t = Instant::now();
            if let Err(e) = self.writer.upload(&self.repo, &objects) {
                rec.fail("upload", &e);
                backoff(&e);
                last = None;
                continue;
            }
            rec.step("upload", t.elapsed());
            let t = Instant::now();
            let key = format!("w{}-{iter}", self.id);
            let proposal =
                match self.writer.propose(&self.repo, &self.stream, &cs.to_string(), &format!("change {iter}"), &key) {
                    Ok(p) => p,
                    Err(e) => {
                        rec.fail("propose", &e);
                        backoff(&e);
                        last = None;
                        continue;
                    }
                };
            rec.step("propose", t.elapsed());
            let t = Instant::now();
            match self.integrator.integrate(&self.repo, &proposal, &key) {
                Ok(o) => {
                    rec.step("integrate", t.elapsed());
                    rec.step("cycle", t_cycle.elapsed());
                    rec.completed += 1;
                    let label =
                        if o.strategy.is_empty() { o.status.clone() } else { format!("{} ({})", o.status, o.strategy) };
                    *rec.outcomes.entry(label).or_default() += 1;
                    if o.status == "integrated" {
                        *rec.integrated_per_stream.entry(self.stream.clone()).or_default() += 1;
                        if self.mode != Mode::Conflict {
                            for (path, bytes) in &files {
                                rec.expected_files.insert((self.stream.clone(), path.clone()), bytes.clone());
                            }
                        }
                    }
                    last = (o.status == "integrated").then(|| o.new_head.clone()).filter(|h| !h.is_empty());
                }
                Err(e) => {
                    rec.fail("integrate", &e);
                    backoff(&e);
                    last = None;
                }
            }
        }
        rec
    }
}

/// Runs `workers` for `duration` and returns the merged record and the elapsed time of the load phase.
fn drive(workers: Vec<Worker>, duration: Duration) -> (Rec, Duration) {
    let barrier = Barrier::new(workers.len() + 1);
    let mut all = Rec::default();
    let started = std::thread::scope(|s| {
        let handles: Vec<_> = workers
            .iter()
            .map(|w| {
                let barrier = &barrier;
                s.spawn(move || {
                    // All workers start together; the clock starts when the last one is ready.
                    barrier.wait();
                    w.run(Instant::now() + duration)
                })
            })
            .collect();
        barrier.wait();
        let started = Instant::now();
        for h in handles {
            all.merge(h.join().expect("worker thread"));
        }
        started
    });
    (all, started.elapsed())
}

/// One cycle on an empty stream that gives the contention scenarios a common base.
fn seed(fx: &Fixture, w: &Worker) -> Result<()> {
    let (cs, objects) =
        build_changeset(&fx.namespace, vec![], "user:w0", "base", &[("shared.txt".into(), b"base\n".to_vec())])?;
    w.writer.upload(&fx.repo, &objects).map_err(|e| anyhow::anyhow!("seed upload: {} {}", e.code, e.message))?;
    let p = w
        .writer
        .propose(&fx.repo, &w.stream, &cs.to_string(), "base", "seed")
        .map_err(|e| anyhow::anyhow!("seed propose: {}", e.code))?;
    let o = w.integrator.integrate(&fx.repo, &p, "seed").map_err(|e| anyhow::anyhow!("seed integrate: {}", e.code))?;
    ensure!(o.status == "integrated", "seed was {}", o.status);
    Ok(())
}

fn workers(fx: &Fixture, plan: &Plan, proto: Proto, mode: Mode, rt: &tokio::runtime::Handle) -> Result<Vec<Worker>> {
    (0..plan.workers)
        .map(|i| {
            let stream = if mode == Mode::Chain {
                let s = format!("s{i}");
                fx.stream(&s)?;
                s
            } else {
                "main".to_string()
            };
            let (writer, integrator) = fx.pair(i, proto, rt)?;
            Ok(Worker {
                id: i,
                repo: fx.repo.clone(),
                stream,
                mode,
                writer,
                integrator,
                namespace: fx.namespace.clone(),
            })
        })
        .collect()
}

// ---- scenarios -------------------------------------------------------------------------------------------

pub fn run_one(
    name: &str,
    proto: Proto,
    bin: &Path,
    plan: &Plan,
    rt: &tokio::runtime::Handle,
) -> Result<ScenarioResult> {
    eprintln!("\n== {name} over {proto:?}");
    match name {
        "baseline" => baseline(proto, bin, plan, rt),
        "streams" => cycles(
            "streams",
            "Concurrent integrations on different streams of one repository: every client owns a stream and repeats upload (2 files + changeset), propose, integrate. A completed unit is one integrated change.",
            Mode::Chain,
            proto,
            bin,
            plan,
            rt,
        ),
        "contention" => cycles(
            "contention",
            "Same-stream contention: every client reads the head, builds on it, uploads, proposes and integrates on one stream, each changing its own files. A change built on a head that moved meanwhile is merged by the server (three-way). A completed unit is one answered integration.",
            Mode::Contend,
            proto,
            bin,
            plan,
            rt,
        ),
        "conflict" => cycles(
            "conflict",
            "Same-stream contention on one file: every client changes `shared.txt` on the head it read. The first to integrate wins; changes built on a stale head conflict and are reported as such. A completed unit is one answered integration, whatever the outcome.",
            Mode::Conflict,
            proto,
            bin,
            plan,
            rt,
        ),
        "sse" => sse(bin, plan, rt),
        "ratelimit" => ratelimit(proto, bin, plan, rt),
        "shedding" => shedding(proto, bin, plan, rt),
        other => bail!("unknown scenario {other}"),
    }
}

fn cycles(
    name: &str,
    description: &str,
    mode: Mode,
    proto: Proto,
    bin: &Path,
    plan: &Plan,
    rt: &tokio::runtime::Handle,
) -> Result<ScenarioResult> {
    let fx = Fixture::start(bin, plan, ServerSettings::unlimited())?;
    let ws = workers(&fx, plan, proto, mode, rt)?;
    let streams: HashSet<String> = ws.iter().map(|w| w.stream.clone()).collect();
    if mode != Mode::Chain {
        seed(&fx, &ws[0])?;
    }
    let before = fx.server.pid().and_then(proc_use);
    let (mut rec, elapsed) = drive(ws, plan.duration);
    ensure!(rec.completed > 0, "{name}: no integration completed; errors: {:?}", rec.errors);
    ensure!(
        rec.integrated_per_stream.values().sum::<u64>() > 0,
        "{name}: no change was integrated; outcomes: {:?}",
        rec.outcomes
    );
    let expected_files = std::mem::take(&mut rec.expected_files);
    let integrated_per_stream = std::mem::take(&mut rec.integrated_per_stream);
    let mut r = finish(name, proto, description, plan.workers, elapsed, rec, &fx.server, before);
    let seeded = u64::from(mode != Mode::Chain);
    for stream in &streams {
        let (_, version) = fx.admin.head(&fx.repo, stream).map_err(|e| anyhow::anyhow!("{}", e.message))?;
        let expected = integrated_per_stream.get(stream).copied().unwrap_or(0) + seeded;
        ensure!(version == expected, "stream {stream}: version {version}, expected {expected} acknowledged updates");
    }
    for ((stream, path), expected) in &expected_files {
        let bytes = fx
            .admin
            .file_bytes(&fx.repo, stream, path)
            .map_err(|e| anyhow::anyhow!("reading {stream}/{path}: {}", e.message))?;
        ensure!(&bytes == expected, "stream {stream}: acknowledged content lost at {path}");
    }
    let verified_files = expected_files.len();
    r.notes.insert("acknowledged updates verified".into(), "true".into());
    r.notes.insert("acknowledged files verified".into(), verified_files.to_string());
    let (head, version) = fx
        .admin
        .head(&fx.repo, if mode == Mode::Chain { "s0" } else { "main" })
        .map_err(|e| anyhow::anyhow!("{}", e.code))?;
    r.notes
        .insert("stream after the run".into(), format!("head `{}…`, version {version}", &head[..head.len().min(12)]));
    let mut srv = fx.server;
    r.notes.insert("server alive at the end".into(), srv.alive().to_string());
    Ok(r)
}

fn baseline(proto: Proto, bin: &Path, plan: &Plan, rt: &tokio::runtime::Handle) -> Result<ScenarioResult> {
    let fx = Fixture::start(bin, plan, ServerSettings::unlimited())?;
    let clients = plan.workers * 2;
    let sessions: Vec<Session> = (0..clients)
        .map(|i| Session::connect(proto, &fx.server.url, &fx.user(&format!("user:r{i}"), "reader")?, rt))
        .collect::<Result<_>>()?;
    let before = fx.server.pid().and_then(proc_use);
    let (rec, elapsed) = hammer(&sessions, plan.duration, |s| s.head(&fx.repo, "main").map(|_| ()), "read");
    let mut r = finish(
        "baseline",
        proto,
        "Authenticated reads of a stream head (credential check, authorization, one metadata read) with no rate limit: the ceiling that the write scenarios sit under. A completed unit is one answered request.",
        clients,
        elapsed,
        rec,
        &fx.server,
        before,
    );
    let mut srv = fx.server;
    r.notes.insert("server alive at the end".into(), srv.alive().to_string());
    Ok(r)
}

/// Closed-loop calls of `op` from one thread per session until the duration ends. Answers count as `ok_step`; failures are recorded by code.
fn hammer(
    sessions: &[Session],
    duration: Duration,
    op: impl Fn(&Session) -> Result<(), CallError> + Sync,
    ok_step: &'static str,
) -> (Rec, Duration) {
    let barrier = Barrier::new(sessions.len() + 1);
    let mut all = Rec::default();
    let started = std::thread::scope(|sc| {
        let handles: Vec<_> = sessions
            .iter()
            .map(|s| {
                let (barrier, op) = (&barrier, &op);
                sc.spawn(move || {
                    let mut rec = Rec::default();
                    barrier.wait();
                    let end = Instant::now() + duration;
                    while Instant::now() < end {
                        let t = Instant::now();
                        match op(s) {
                            Ok(()) => {
                                rec.step(ok_step, t.elapsed());
                                rec.completed += 1;
                            }
                            Err(e) => {
                                rec.step("rejected", t.elapsed());
                                rec.fail("request", &e);
                                rec.completed += 1;
                                if e.code == "transport" {
                                    std::thread::sleep(Duration::from_millis(20));
                                }
                            }
                        }
                    }
                    rec
                })
            })
            .collect();
        barrier.wait();
        let started = Instant::now();
        for h in handles {
            all.merge(h.join().expect("client thread"));
        }
        started
    });
    (all, started.elapsed())
}

fn ready(server: &Server) -> bool {
    Session::http_only(&server.url, "").http("GET", "/readyz", None).is_ok_and(|r| r.status == 200)
}

fn ratelimit(proto: Proto, bin: &Path, plan: &Plan, rt: &tokio::runtime::Handle) -> Result<ScenarioResult> {
    let settings =
        ServerSettings { requests_per_second: plan.rate_limit, burst: plan.rate_burst, max_concurrent_requests: 512 };
    let fx = Fixture::start(bin, plan, settings)?;
    let hammer_token = fx.user("user:hammer", "reader")?;
    let calm_token = fx.user("user:calm", "reader")?;
    let clients: Vec<Session> =
        (0..8).map(|_| Session::connect(proto, &fx.server.url, &hammer_token, rt)).collect::<Result<_>>()?;
    let calm = Session::connect(proto, &fx.server.url, &calm_token, rt)?;
    // A budget of the first calls of a fresh bucket is part of what is measured: `burst` tokens, then `rate` per second.
    let stop = AtomicBool::new(false);
    let calm_stats = Mutex::new((0u64, 0u64));
    let hints = Mutex::new(Vec::<(i64, bool, u16)>::new());
    let before = fx.server.pid().and_then(proc_use);
    let (rec, elapsed) = std::thread::scope(|sc| {
        let c = sc.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                let ok = calm.ping().is_ok();
                let mut g = calm_stats.lock().unwrap();
                if ok {
                    g.0 += 1
                } else {
                    g.1 += 1
                }
                drop(g);
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        let r = hammer(
            &clients,
            plan.duration,
            |s| {
                s.ping().inspect_err(|e| {
                    if e.code == "rate_limited" {
                        hints.lock().unwrap().push((e.retry_after_ms, e.retry_after_header.is_some(), e.http_status));
                    }
                })
            },
            "admitted",
        );
        stop.store(true, Ordering::Relaxed);
        c.join().expect("calm thread");
        r
    });
    let admitted = rec.steps.get("admitted").map_or(0, Vec::len);
    let hints = hints.into_inner().unwrap();
    let (calm_ok, calm_failed) = *calm_stats.lock().unwrap();
    let mut r = finish(
        "ratelimit",
        proto,
        &format!(
            "One credential hammered by 8 closed-loop clients against `limits.requests_per_second = {}`, `burst = {}`; a second credential makes one call every 200 ms meanwhile. A completed unit is one answered request, admitted or rejected.",
            plan.rate_limit, plan.rate_burst
        ),
        clients.len() + 1,
        elapsed,
        rec,
        &fx.server,
        before,
    );
    let secs = elapsed.as_secs_f64();
    r.notes
        .insert("admitted requests".into(), format!("{admitted} = {:.1}/s over {secs:.1} s", admitted as f64 / secs));
    let allowed = f64::from(plan.rate_burst) + f64::from(plan.rate_limit) * secs;
    r.allowed_requests = Some(allowed);
    r.notes.insert(
        "what the configuration allows".into(),
        format!(
            "about burst + rate × time = {} + {} × {secs:.1} = {allowed:.0} requests",
            plan.rate_burst, plan.rate_limit
        ),
    );
    if !hints.is_empty() {
        let hint_min = hints.iter().map(|h| h.0).min().unwrap_or(0);
        let hint_max = hints.iter().map(|h| h.0).max().unwrap_or(0);
        let header = hints.iter().filter(|h| h.1).count();
        r.notes.insert("retry hint on rejections".into(), if proto == Proto::Http {
            format!("`retry_after_ms` {hint_min}–{hint_max} ms in the body; `Retry-After` header (whole seconds, rounded up) on {header} of {} rejections", hints.len())
        } else {
            format!("`retry_after_ms` {hint_min}–{hint_max} ms in the error details ({} rejections)", hints.len())
        });
    }
    r.notes.insert("second credential meanwhile".into(), format!("{calm_ok} calls answered, {calm_failed} rejected"));
    // After the hint, the same credential is served again.
    std::thread::sleep(Duration::from_millis(1100));
    r.notes.insert(
        "hammered credential after 1.1 s of quiet".into(),
        if clients[0].ping().is_ok() { "served".into() } else { "still rejected".into() },
    );
    let mut srv = fx.server;
    r.notes.insert("server alive at the end".into(), srv.alive().to_string());
    Ok(r)
}

fn shedding(proto: Proto, bin: &Path, plan: &Plan, rt: &tokio::runtime::Handle) -> Result<ScenarioResult> {
    let settings = ServerSettings { requests_per_second: 0, burst: 0, max_concurrent_requests: plan.shed_limit };
    let fx = Fixture::start(bin, plan, settings)?;
    let n = plan.workers * 4;
    let token = fx.user("user:reader", "reader")?;
    let sessions: Vec<Session> =
        (0..n).map(|_| Session::connect(proto, &fx.server.url, &token, rt)).collect::<Result<_>>()?;
    let before = fx.server.pid().and_then(proc_use);
    let (rec, elapsed) = hammer(&sessions, plan.duration, |s| s.head(&fx.repo, "main").map(|_| ()), "read");
    let mut r = finish(
        "shedding",
        proto,
        &format!(
            "{n} closed-loop clients reading a stream head against `limits.max_concurrent_requests = {}`: requests beyond the cap are refused at once (`unavailable`) instead of queuing. A completed unit is one answered request, served or refused.",
            plan.shed_limit
        ),
        n,
        elapsed,
        rec,
        &fx.server,
        before,
    );
    let mut srv = fx.server;
    r.notes.insert("server alive at the end".into(), srv.alive().to_string());
    r.notes.insert("ready after the run".into(), ready(&srv).to_string());
    Ok(r)
}

fn sse(bin: &Path, plan: &Plan, rt: &tokio::runtime::Handle) -> Result<ScenarioResult> {
    let fx = Fixture::start(bin, plan, ServerSettings::unlimited())?;
    let ws = workers(&fx, plan, Proto::Http, Mode::Chain, rt)?;
    let token = fx.user("user:watcher", "reader")?;
    let subs: Vec<Session> = (0..plan.subscribers)
        .map(|i| Session::connect(if i % 2 == 0 { Proto::Http } else { Proto::Grpc }, &fx.server.url, &token, rt))
        .collect::<Result<_>>()?;
    let before = fx.server.pid().and_then(proc_use);
    let attached = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    struct Seen {
        proto: Proto,
        lags: Vec<Duration>,
        seqs: HashSet<u64>,
        duplicates: u64,
        failure: Option<String>,
    }
    let seen = Mutex::new(Vec::<Seen>::new());
    let (rec, elapsed) = std::thread::scope(|sc| -> Result<(Rec, Duration)> {
        let handles: Vec<_> = subs
            .iter()
            .map(|s| {
                let (attached, stop, seen, repo) = (&attached, &stop, &seen, &fx.repo);
                sc.spawn(move || {
                    let mut mine =
                        Seen { proto: s.proto, lags: Vec::new(), seqs: HashSet::new(), duplicates: 0, failure: None };
                    let r = s.follow(
                        repo,
                        &|| {
                            attached.fetch_add(1, Ordering::SeqCst);
                        },
                        &|| stop.load(Ordering::Relaxed),
                        &mut |seq, topic, ts, _| {
                            if topic == "stream.integrate" {
                                if !mine.seqs.insert(seq) {
                                    mine.duplicates += 1;
                                }
                                mine.lags.push(Duration::from_millis((now_ms() - ts).max(0) as u64));
                            }
                        },
                    );
                    mine.failure = r.err();
                    seen.lock().unwrap().push(mine);
                })
            })
            .collect();
        let give_up = Instant::now() + Duration::from_secs(30);
        while attached.load(Ordering::SeqCst) < subs.len() && Instant::now() < give_up {
            if seen.lock().unwrap().iter().any(|s| s.failure.is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if attached.load(Ordering::SeqCst) < subs.len() {
            stop.store(true, Ordering::Relaxed);
            let why = seen.lock().unwrap().iter().find_map(|s| s.failure.clone()).unwrap_or_else(|| "timed out".into());
            bail!("only {} of {} subscribers attached: {why}", attached.load(Ordering::SeqCst), subs.len());
        }
        let out = drive(ws, plan.duration);
        // Let the last events travel, then end the feeds.
        std::thread::sleep(Duration::from_secs(2));
        stop.store(true, Ordering::Relaxed);
        for h in handles {
            h.join().expect("subscriber thread");
        }
        Ok(out)
    })?;
    let integrated: u64 = rec.outcomes.iter().filter(|(k, _)| k.starts_with("integrated")).map(|(_, v)| v).sum();
    let seen = seen.into_inner().unwrap();
    let mut r = finish(
        "sse",
        Proto::Http,
        &format!(
            "Event fan-out: {} subscribers (half SSE over HTTP/JSON, half gRPC server streams) follow one repository while {} writers integrate on their own streams (HTTP/JSON). Delivery lag is the client's receive time minus the event's server timestamp (same host, 1 ms resolution). A completed unit is one integrated change.",
            plan.subscribers, plan.workers
        ),
        plan.workers + plan.subscribers,
        elapsed,
        rec,
        &fx.server,
        before,
    );
    for proto in [Proto::Http, Proto::Grpc] {
        let lags: Vec<Duration> =
            seen.iter().filter(|s| s.proto == proto).flat_map(|s| s.lags.iter().copied()).collect();
        if !lags.is_empty() {
            let label = if proto == Proto::Http { "event lag, SSE" } else { "event lag, gRPC" };
            r.steps.push((label.into(), Summary::of(&lags)));
        }
    }
    let counts: Vec<u64> = seen.iter().map(|s| s.seqs.len() as u64).collect();
    let complete = counts.iter().filter(|c| **c == integrated).count();
    r.notes.insert(
        "stream.integrate events per subscriber".into(),
        format!(
            "{integrated} integrations happened; subscribers received min {} / max {}; {complete} of {} received all of them; {} duplicate deliveries",
            counts.iter().min().unwrap_or(&0),
            counts.iter().max().unwrap_or(&0),
            seen.len(),
            seen.iter().map(|s| s.duplicates).sum::<u64>()
        ),
    );
    let failed: Vec<String> = seen.iter().filter_map(|s| s.failure.clone()).collect();
    r.notes.insert(
        "feeds that failed".into(),
        if failed.is_empty() { "none".into() } else { format!("{}: {}", failed.len(), failed[0]) },
    );
    let mut srv = fx.server;
    r.notes.insert("server alive at the end".into(), srv.alive().to_string());
    Ok(r)
}
