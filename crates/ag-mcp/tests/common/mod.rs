#![allow(dead_code, missing_docs)]
//! A real `ag-server` on an ephemeral port and a real `ag-mcp` process, spoken to over stdio.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use ag_server::app::App;
use ag_server::config::ServerConfig;
use ag_storage::ManualClock;
use ag_types::Timestamp;
use base64::Engine as _;
use serde_json::{Value, json};

pub const ADMIN: &str = "test-bootstrap-token-0123456789abcdef";

pub struct TestServer {
    pub url: String,
    pub app: App,
    pub clock: Arc<ManualClock>,
    _dir: tempfile::TempDir,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    _thread: std::thread::JoinHandle<()>,
}

impl TestServer {
    pub fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = ServerConfig::default();
        cfg.server.data_dir = dir.path().to_path_buf();
        cfg.limits.requests_per_second = 0;
        let clock = Arc::new(ManualClock::new(Timestamp::now()));
        let app = App::with_options(cfg, clock.clone(), Some(ADMIN.to_string())).unwrap();
        let (tx, rx) = channel();
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
        Self { url: format!("http://{addr}"), app, clock, _dir: dir, stop: Some(stop_tx), _thread: thread }
    }

    pub fn http(&self, token: &str) -> Http {
        Http { base: self.url.clone(), token: token.to_string() }
    }

    pub fn admin(&self) -> Http {
        self.http(ADMIN)
    }

    /// Creates a repository.
    pub fn repo(&self, name: &str) {
        let r = self.admin().post("/v1/repos", json!({"name": name}));
        assert_eq!(r.0, 200, "{}", r.1);
    }

    /// Grants a role and mints a credential (optionally narrowed to repositories and operations).
    pub fn credential(&self, subject: &str, repo: &str, role: &str, ttl_secs: u64, operations: &[&str]) -> String {
        let a = self.admin();
        let r = a.post("/v1/roles", json!({"subject": subject, "repo": repo, "role": role}));
        assert_eq!(r.0, 200, "{}", r.1);
        let mut scope = json!({"repos": [repo]});
        if !operations.is_empty() {
            scope["operations"] = json!(operations);
        }
        let r = a.post("/v1/credentials", json!({"subject": subject, "ttl_secs": ttl_secs, "scope": scope}));
        assert_eq!(r.0, 200, "{}", r.1);
        r.1["token"].as_str().unwrap().to_string()
    }

    /// An extra role for an existing subject.
    pub fn grant(&self, subject: &str, repo: &str, role: &str) {
        let r = self.admin().post("/v1/roles", json!({"subject": subject, "repo": repo, "role": role}));
        assert_eq!(r.0, 200, "{}", r.1);
    }

    /// Registers an agent (with a throwaway key) using its own credential.
    pub fn register_agent(&self, token: &str, repo: &str, name: &str) {
        let key = ag_crypto::KeyPair::generate().unwrap();
        let r = self
            .http(token)
            .post(&format!("/v1/repos/{repo}/agents"), json!({"name": name, "public_key": key.public().to_hex()}));
        assert_eq!(r.0, 200, "{}", r.1);
    }

    /// Lands `files` on `main` as a human would: workspace, proposal, integrate.
    pub fn seed(&self, repo: &str, message: &str, files: &[(&str, &str)]) -> String {
        let writer = self.credential("user:seeder", repo, "writer", 3600, &[]);
        self.grant("user:seeder", repo, "integrator");
        let h = self.http(&writer);
        let ws = format!("seed-{}", short_id());
        let r = h.post(&format!("/v1/repos/{repo}/workspaces"), json!({"name": ws, "stream": "main"}));
        assert_eq!(r.0, 200, "{}", r.1);
        let changes: Vec<Value> = files
            .iter()
            .map(|(p, c)| json!({"path": p, "op": "put", "content": base64::engine::general_purpose::STANDARD.encode(c)}))
            .collect();
        let r = h.post(
            &format!("/v1/repos/{repo}/workspaces/{ws}/changes"),
            json!({"message": message, "changes": changes}),
        );
        assert_eq!(r.0, 200, "{}", r.1);
        let head = r.1["head"].as_str().unwrap().to_string();
        let p =
            h.post(&format!("/v1/repos/{repo}/proposals"), json!({"stream": "main", "head": head, "title": message}));
        assert_eq!(p.0, 200, "{}", p.1);
        let pid = p.1["id"].as_str().unwrap();
        let i = h.post(&format!("/v1/repos/{repo}/proposals/{pid}/integrate"), json!({}));
        assert_eq!(i.1["outcome"]["status"], "integrated", "{}", i.1);
        i.1["outcome"]["new_head"].as_str().unwrap().to_string()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

fn short_id() -> String {
    format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos())
}

/// A blocking HTTP client for the test's own set-up calls.
#[derive(Clone)]
pub struct Http {
    pub base: String,
    pub token: String,
}

impl Http {
    pub fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        let b = ureq::http::Request::builder()
            .method(method)
            .uri(format!("{}{}", self.base, path))
            .header("authorization", format!("Bearer {}", self.token));
        let resp = match body {
            Some(v) => agent.run(b.header("content-type", "application/json").body(v.to_string()).unwrap()),
            None => agent.run(b.body(()).unwrap()),
        }
        .unwrap();
        let status = resp.status().as_u16();
        let mut resp = resp;
        let bytes = resp.body_mut().read_to_vec().unwrap_or_default();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    pub fn get(&self, path: &str) -> (u16, Value) {
        self.call("GET", path, None)
    }

    pub fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.call("POST", path, Some(body))
    }
}

// ---------------------------------------------------------------------------------------------------------
// the MCP client

pub const MODERN: &str = "2026-07-28";

/// A running `ag-mcp` process and a thread that reads its standard output line by line.
pub struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Value>,
    next_id: i64,
    /// Notifications and unmatched messages seen while waiting for a response.
    pub notifications: Vec<Value>,
    tools: Option<Vec<Value>>,
    pub modern: bool,
    /// Everything the process wrote to its standard error.
    pub stderr: Arc<std::sync::Mutex<Vec<String>>>,
    /// Every raw line the process wrote to its standard output.
    pub stdout_lines: Arc<std::sync::Mutex<Vec<String>>>,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub is_error: bool,
    pub text: String,
    pub structured: Value,
    pub raw: Value,
}

impl ToolResult {
    /// The machine-readable code of an error result.
    pub fn error_code(&self) -> &str {
        self.raw["_meta"]["agentgit/error"]["code"].as_str().unwrap_or("")
    }
}

impl Mcp {
    pub fn spawn(args: &[&str], envs: &[(&str, &str)]) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ag-mcp"));
        cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for k in
            ["AG_MCP_SERVER", "AG_MCP_TOKEN", "AG_MCP_TOKEN_FILE", "AG_MCP_REPO", "AG_MCP_ALLOW", "AG_MCP_WRITE_PATHS"]
        {
            cmd.env_remove(k);
        }
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn ag-mcp");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = channel();
        let raw_lines = Arc::new(std::sync::Mutex::new(Vec::new()));
        let raw_sink = raw_lines.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                raw_sink.lock().unwrap().push(line.clone());
                let v: Value = serde_json::from_str(&line)
                    .unwrap_or_else(|e| panic!("stdout carried a non-JSON line {line:?}: {e}"));
                if tx.send(v).is_err() {
                    break;
                }
            }
        });
        // keep stderr drained (and visible when a test fails)
        let err_lines = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = err_lines.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                eprintln!("[ag-mcp stderr] {line}");
                sink.lock().unwrap().push(line);
            }
        });
        Self {
            child,
            stdin,
            rx,
            next_id: 1,
            notifications: Vec::new(),
            tools: None,
            modern: true,
            stderr: err_lines,
            stdout_lines: raw_lines,
        }
    }

    /// Starts `ag-mcp` for a server with the given credential in `AG_MCP_TOKEN`, then performs the stateless discovery.
    pub fn start(srv: &TestServer, token: &str, extra: &[&str]) -> Self {
        let mut args = vec!["--server", &srv.url];
        args.extend_from_slice(extra);
        let mut m = Self::spawn(&args, &[("AG_MCP_TOKEN", token)]);
        let d = m.request("server/discover", json!({}));
        assert_eq!(d["result"]["supportedVersions"][0], MODERN, "{d}");
        m
    }

    /// Fails if `secret` appears anywhere the process has written so far (standard output or error).
    pub fn assert_never_saw(&self, secret: &str) {
        let out = self.stdout_lines.lock().unwrap().join("\n");
        let err = self.stderr_text();
        assert!(!out.contains(secret), "the credential leaked to standard output");
        assert!(!err.contains(secret), "the credential leaked to standard error");
        let body = secret.rsplit('_').next().unwrap_or(secret);
        if body.len() > 16 {
            assert!(!out.contains(body) && !err.contains(body), "the credential's secret part leaked");
        }
    }

    /// All raw standard output so far.
    pub fn all_stdout(&self) -> String {
        self.stdout_lines.lock().unwrap().join("\n")
    }

    /// The standard error written so far, as one string.
    pub fn stderr_text(&self) -> String {
        self.stderr.lock().unwrap().join("\n")
    }

    /// The standard error once it contains `needle`, or as it is when five seconds have passed.
    ///
    /// A line the process wrote before it answered is already in the pipe when the answer arrives, but the
    /// thread that drains standard error runs independently of the one that drains standard output, so
    /// reading [`Mcp::stderr_text`] right after a response can miss it. Waiting for the line is the
    /// deterministic way to assert that the process logged something; the bound only keeps a missing line from
    /// hanging the suite.
    pub fn stderr_with(&self, needle: &str) -> String {
        let end = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let text = self.stderr_text();
            if text.contains(needle) || std::time::Instant::now() >= end {
                return text;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn write_raw(&mut self, line: &str) {
        let w = self.stdin.as_mut().expect("stdin open");
        w.write_all(line.as_bytes()).unwrap();
        w.write_all(b"\n").unwrap();
        w.flush().unwrap();
    }

    pub fn send(&mut self, v: &Value) {
        self.write_raw(&v.to_string());
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn with_meta(&self, mut params: Value) -> Value {
        if self.modern {
            let o = params.as_object_mut().unwrap();
            let meta = o.entry("_meta").or_insert_with(|| json!({}));
            meta["io.modelcontextprotocol/protocolVersion"] = json!(MODERN);
            meta["io.modelcontextprotocol/clientCapabilities"] = json!({});
            meta["io.modelcontextprotocol/clientInfo"] = json!({"name": "ag-mcp-tests", "version": "0"});
        }
        params
    }

    /// Sends a request without waiting; returns its id.
    pub fn send_request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        let params = self.with_meta(params);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    pub fn recv(&mut self) -> Option<Value> {
        self.rx.recv_timeout(Duration::from_secs(60)).ok()
    }

    /// Waits for the response with `id`; notifications are collected.
    pub fn wait_for(&mut self, id: i64) -> Value {
        loop {
            let msg = self.recv().unwrap_or_else(|| panic!("no response to request {id} within 60 s"));
            if msg.get("id") == Some(&json!(id)) {
                return msg;
            }
            self.notifications.push(msg);
        }
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send_request(method, params);
        self.wait_for(id)
    }

    /// Legacy handshake (`initialize` + `notifications/initialized`).
    pub fn initialize_legacy(&mut self, version: &str) -> Value {
        self.modern = false;
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": {
            "protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "legacy-test", "version": "1"}}}));
        let r = self.wait_for(id);
        self.notify("notifications/initialized", json!({}));
        r
    }

    pub fn tools(&mut self) -> Vec<Value> {
        if self.tools.is_none() {
            let r = self.request("tools/list", json!({}));
            self.tools = Some(r["result"]["tools"].as_array().unwrap_or_else(|| panic!("{r}")).clone());
        }
        self.tools.clone().unwrap()
    }

    pub fn tool_names(&mut self) -> Vec<String> {
        self.tools().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
    }

    /// Calls a tool; checks the structured result against the tool's published output schema.
    pub fn call(&mut self, name: &str, args: Value) -> ToolResult {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}));
        let res = self.tool_result(name, &r);
        if self.modern && !res.is_error {
            let tools = self.tools();
            let schema = tools.iter().find(|t| t["name"] == name).map(|t| t["outputSchema"].clone());
            if let Some(s) = schema {
                let errs = ag_mcp::schema::validate(&s, &res.structured);
                assert!(
                    errs.is_empty(),
                    "{name}: structuredContent violates its outputSchema: {errs:?}\n{}",
                    res.structured
                );
            }
        }
        res
    }

    pub fn tool_result(&self, name: &str, r: &Value) -> ToolResult {
        let result = r.get("result").unwrap_or_else(|| panic!("{name}: protocol error: {r}"));
        ToolResult {
            is_error: result["isError"].as_bool().unwrap_or(false),
            text: result["content"][0]["text"].as_str().unwrap_or("").to_string(),
            structured: result.get("structuredContent").cloned().unwrap_or(Value::Null),
            raw: result.clone(),
        }
    }

    /// Calls a tool and requires success.
    pub fn ok(&mut self, name: &str, args: Value) -> ToolResult {
        let r = self.call(name, args);
        assert!(!r.is_error, "{name} failed: {}", r.text);
        r
    }

    /// Calls a tool and requires a refusal with this code.
    pub fn refused(&mut self, name: &str, args: Value, code: &str) -> ToolResult {
        let r = self.call(name, args);
        assert!(r.is_error, "{name} should have been refused but returned: {}", r.text);
        assert_eq!(r.error_code(), code, "{name}: {}", r.text);
        r
    }

    /// Closes stdin and waits for the process; returns its exit status.
    pub fn finish(mut self) -> std::process::ExitStatus {
        drop(self.stdin.take());
        self.child.wait().unwrap()
    }

    pub fn close_stdin(&mut self) {
        drop(self.stdin.take());
    }

    pub fn wait_exit(&mut self) -> std::process::ExitStatus {
        self.child.wait().unwrap()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Runs `ag-mcp` once to completion with the given arguments (for flag handling).
pub fn run_once(args: &[&str], envs: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ag-mcp"));
    cmd.args(args).stdin(Stdio::null());
    for k in ["AG_MCP_SERVER", "AG_MCP_TOKEN", "AG_MCP_TOKEN_FILE", "AG_MCP_REPO", "AG_MCP_ALLOW", "AG_MCP_WRITE_PATHS"]
    {
        cmd.env_remove(k);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let o = cmd.output().unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

pub fn write_file(p: &Path, content: &str) {
    std::fs::write(p, content).unwrap();
}
