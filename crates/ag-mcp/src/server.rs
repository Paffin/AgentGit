//! The MCP server proper: version handling, method dispatch, tool calls with cancellation and progress.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::api::Api;
use crate::config::{Config, Group};
use crate::jsonrpc::{self, INTERNAL_ERROR, Line, METHOD_NOT_FOUND, Message, RpcError, UNSUPPORTED_PROTOCOL_VERSION};
use crate::log;
use crate::output::{ToolError, render_err, render_ok};
use crate::protocol::{
    INSTRUCTIONS, META_CAPABILITIES, META_SERVER_INFO, META_VERSION, SUPPORTED, Version, server_info, supported_strings,
};
use crate::tools::{CallCtx, Registry};
use crate::{prompts, resources, schema};

/// How long a graceful shutdown (stdin closed) waits for calls still running.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(20);

/// The standard output, shared by all threads; one message per line.
pub struct Out {
    w: Mutex<Box<dyn Write + Send>>,
    closed: AtomicBool,
}

impl Out {
    /// Wraps a writer.
    pub fn new(w: Box<dyn Write + Send>) -> Self {
        Self { w: Mutex::new(w), closed: AtomicBool::new(false) }
    }

    /// Writes one message and flushes. A failed write marks the output closed.
    pub fn send(&self, v: &Value) {
        let mut line = match serde_json::to_string(v) {
            Ok(l) => l,
            Err(e) => {
                log::error(&format!("cannot serialize a message: {e}"));
                return;
            }
        };
        line.push('\n');
        let Ok(mut w) = self.w.lock() else { return };
        if w.write_all(line.as_bytes()).and_then(|()| w.flush()).is_err() {
            self.closed.store(true, Ordering::Relaxed);
        }
    }

    /// True once the peer is gone.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}

/// One request that is being worked on.
struct InFlight {
    cancelled: AtomicBool,
}

/// The server.
pub struct Server {
    cfg: Arc<Config>,
    api: Arc<Api>,
    registry: Arc<Registry>,
    out: Arc<Out>,
    legacy: Mutex<Option<Version>>,
    inflight: Mutex<HashMap<String, Arc<InFlight>>>,
    active: AtomicUsize,
}

fn id_key(id: &Value) -> String {
    match id {
        Value::String(s) => format!("s:{s}"),
        other => format!("n:{other}"),
    }
}

/// Cache hints of a result (modern revision only).
#[derive(Clone, Copy)]
enum Cache {
    /// Identical for every caller of this process.
    Public(u64),
    /// Depends on the repository's state.
    Private(u64),
}

impl Server {
    /// Builds a server that talks to the configured AgentGit server.
    pub fn new(cfg: Config, registry: Registry, out: Box<dyn Write + Send>) -> Result<Arc<Self>, String> {
        cfg.validate()?;
        let api = Api::new(&cfg)?;
        Ok(Arc::new(Self {
            cfg: Arc::new(cfg),
            api: Arc::new(api),
            registry: Arc::new(registry),
            out: Arc::new(Out::new(out)),
            legacy: Mutex::new(None),
            inflight: Mutex::new(HashMap::new()),
            active: AtomicUsize::new(0),
        }))
    }

    /// The configuration.
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// The AGWP client (for `--check`).
    pub fn api(&self) -> &Api {
        &self.api
    }

    /// The tool registry.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Reads messages until the input ends, then waits (bounded) for running calls.
    pub fn serve<R: BufRead>(self: &Arc<Self>, mut input: R) -> io::Result<()> {
        loop {
            if self.out.is_closed() {
                break;
            }
            match jsonrpc::read_line_bounded(&mut input, self.cfg.max_message_bytes)? {
                Line::Eof => break,
                Line::TooLong => {
                    self.out.send(&jsonrpc::error_response(
                        &Value::Null,
                        &RpcError::new(
                            jsonrpc::INVALID_REQUEST,
                            format!(
                                "message larger than the limit of {} bytes was discarded",
                                self.cfg.max_message_bytes
                            ),
                        ),
                    ));
                }
                Line::Data(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => {}
                Line::Data(bytes) => self.handle_line(&bytes),
            }
        }
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while self.active.load(Ordering::SeqCst) > 0 && Instant::now() < deadline && !self.out.is_closed() {
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    /// Parses and dispatches one line.
    pub fn handle_line(self: &Arc<Self>, line: &[u8]) {
        match jsonrpc::parse_message(line) {
            Ok(Message::Request { id, method, params }) => self.on_request(id, method, params),
            Ok(Message::Notification { method, params }) => self.on_notification(&method, &params),
            Ok(Message::Response) => {}
            Err((id, e)) => self.out.send(&jsonrpc::error_response(&id, &e)),
        }
    }

    fn on_notification(&self, method: &str, params: &Value) {
        match method {
            "notifications/cancelled" => {
                let Some(rid) = params.get("requestId") else { return };
                let reason = params.get("reason").and_then(Value::as_str).unwrap_or("");
                if let Some(call) = self.inflight.lock().ok().and_then(|m| m.get(&id_key(rid)).cloned()) {
                    call.cancelled.store(true, Ordering::SeqCst);
                    log::debug(&format!(
                        "request {rid} cancelled by the client ({})",
                        reason.chars().take(80).collect::<String>()
                    ));
                }
            }
            // notifications/initialized and everything else needs no action
            other => log::debug(&format!("notification {other} ignored")),
        }
    }

    // -------------------------------------------------------------------------------------------------
    // versions

    /// Works out which revision a request speaks.
    fn request_version(&self, method: &str, params: &Value) -> Result<Version, RpcError> {
        let meta = params.get("_meta");
        if let Some(v) = meta.and_then(|m| m.get(META_VERSION)) {
            let Some(s) = v.as_str() else {
                return Err(RpcError::invalid_params(format!("`_meta.{META_VERSION}` must be a string")));
            };
            let Some(ver) = Version::parse(s) else {
                return Err(RpcError::new(UNSUPPORTED_PROTOCOL_VERSION, "Unsupported protocol version")
                    .with_data(json!({"supported": supported_strings(), "requested": s})));
            };
            if ver.is_modern() && !meta.and_then(|m| m.get(META_CAPABILITIES)).is_some_and(Value::is_object) {
                return Err(RpcError::invalid_params(format!(
                    "`_meta.{META_CAPABILITIES}` is required on every request of protocol {}",
                    ver.as_str()
                )));
            }
            return Ok(ver);
        }
        if let Some(v) = self.legacy.lock().ok().and_then(|g| *g) {
            return Ok(v);
        }
        if method == "ping" {
            return Ok(Version::V2025_11_25);
        }
        Err(RpcError::invalid_params(format!(
            "the request carries no `_meta.{META_VERSION}` and no `initialize` handshake was performed: send \
             `server/discover` (protocol 2026-07-28) or `initialize` ({})",
            supported_strings()[1..].join(", ")
        )))
    }

    fn capabilities(&self) -> Value {
        let mut c = Map::new();
        c.insert("tools".into(), json!({}));
        if self.cfg.allows(Group::Read) {
            c.insert("resources".into(), json!({}));
        }
        if !prompts::list(&self.cfg).is_empty() {
            c.insert("prompts".into(), json!({}));
        }
        Value::Object(c)
    }

    /// Adds what the revision requires to a result.
    fn finish(&self, ver: Version, mut result: Value, cache: Option<Cache>) -> Value {
        if ver.is_modern()
            && let Some(o) = result.as_object_mut()
        {
            o.insert("resultType".into(), json!("complete"));
            let meta = o.entry("_meta").or_insert_with(|| json!({}));
            if let Some(m) = meta.as_object_mut() {
                m.insert(META_SERVER_INFO.into(), server_info());
            }
            match cache {
                Some(Cache::Public(ttl)) => {
                    o.insert("ttlMs".into(), json!(ttl));
                    o.insert("cacheScope".into(), json!("public"));
                }
                Some(Cache::Private(ttl)) => {
                    o.insert("ttlMs".into(), json!(ttl));
                    o.insert("cacheScope".into(), json!("private"));
                }
                None => {}
            }
        }
        result
    }

    // -------------------------------------------------------------------------------------------------
    // requests

    fn reply(&self, id: &Value, r: Result<Value, RpcError>) {
        self.out.send(&match r {
            Ok(v) => jsonrpc::response(id, v),
            Err(e) => jsonrpc::error_response(id, &e),
        });
    }

    fn on_request(self: &Arc<Self>, id: Value, method: String, params: Value) {
        if method == "initialize" {
            let r = self.initialize(&params);
            self.reply(&id, r);
            return;
        }
        let ver = match self.request_version(&method, &params) {
            Ok(v) => v,
            Err(e) => return self.reply(&id, Err(e)),
        };
        match method.as_str() {
            "ping" => self.reply(&id, Ok(json!({}))),
            "server/discover" => {
                let r = if ver.is_modern() {
                    Ok(self.finish(
                        ver,
                        json!({
                            "supportedVersions": supported_strings(),
                            "capabilities": self.capabilities(),
                            "instructions": INSTRUCTIONS,
                        }),
                        Some(Cache::Public(300_000)),
                    ))
                } else {
                    Err(RpcError::new(
                        METHOD_NOT_FOUND,
                        "server/discover belongs to protocol 2026-07-28; this session speaks an earlier revision",
                    ))
                };
                self.reply(&id, r);
            }
            "tools/list" => {
                let r = self.tools_list(ver, &params);
                self.reply(&id, r);
            }
            "prompts/list" => {
                let r = no_cursor(&params).map(|()| {
                    self.finish(ver, json!({"prompts": prompts::list(&self.cfg)}), Some(Cache::Public(300_000)))
                });
                self.reply(&id, r);
            }
            "prompts/get" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let r = prompts::get(&self.cfg, name, &args).map(|v| self.finish(ver, v, None));
                self.reply(&id, r);
            }
            "resources/templates/list" => {
                let r = if self.cfg.allows(Group::Read) {
                    no_cursor(&params).map(|()| {
                        self.finish(
                            ver,
                            json!({"resourceTemplates": resources::templates()}),
                            Some(Cache::Public(300_000)),
                        )
                    })
                } else {
                    Err(RpcError::new(METHOD_NOT_FOUND, "resources are not available: the read group is off"))
                };
                self.reply(&id, r);
            }
            "resources/list" | "resources/read" if !self.cfg.allows(Group::Read) => {
                self.reply(
                    &id,
                    Err(RpcError::new(METHOD_NOT_FOUND, "resources are not available: the read group is off")),
                );
            }
            "tools/call" | "resources/list" | "resources/read" => self.spawn(id, method, params, ver),
            other => self.reply(&id, Err(RpcError::new(METHOD_NOT_FOUND, format!("Method not found: {other}")))),
        }
    }

    fn initialize(&self, params: &Value) -> Result<Value, RpcError> {
        let requested = params.get("protocolVersion").and_then(Value::as_str);
        if requested.is_none() {
            return Err(RpcError::invalid_params("`protocolVersion` is required"));
        }
        // The newest handshake revision at or below what the client asked for; for anything else, the newest we have.
        let ver = requested.and_then(Version::parse).filter(|v| !v.is_modern()).unwrap_or(Version::V2025_11_25);
        if let Ok(mut g) = self.legacy.lock() {
            *g = Some(ver);
        }
        let client = params.get("clientInfo").and_then(|c| c.get("name")).and_then(Value::as_str).unwrap_or("?");
        log::info(&format!(
            "client `{}` initialized protocol {}",
            client.chars().take(60).collect::<String>(),
            ver.as_str()
        ));
        Ok(json!({
            "protocolVersion": ver.as_str(),
            "capabilities": self.capabilities(),
            "serverInfo": server_info(),
            "instructions": INSTRUCTIONS,
        }))
    }

    fn tools_list(&self, ver: Version, params: &Value) -> Result<Value, RpcError> {
        no_cursor(params)?;
        let tools: Vec<Value> =
            self.registry.enabled(&self.cfg).map(|t| t.to_json(ver.has_structured_output())).collect();
        Ok(self.finish(ver, json!({"tools": tools}), Some(Cache::Public(300_000))))
    }

    /// Runs a slow request on its own thread so that others (and cancellations) are served meanwhile.
    fn spawn(self: &Arc<Self>, id: Value, method: String, params: Value, ver: Version) {
        let busy = self.active.fetch_add(1, Ordering::SeqCst) >= self.cfg.max_concurrent;
        if busy {
            self.active.fetch_sub(1, Ordering::SeqCst);
            let msg = format!(
                "too many calls are running at once (limit {}); wait for one to finish",
                self.cfg.max_concurrent
            );
            if method == "tools/call" {
                let e = ToolError::local("busy", msg).hint("Retry in a moment.");
                self.out.send(&jsonrpc::response(&id, self.finish(ver, render_err(&e, ""), None)));
            } else {
                self.reply(&id, Err(RpcError::new(INTERNAL_ERROR, msg)));
            }
            return;
        }
        let call = Arc::new(InFlight { cancelled: AtomicBool::new(false) });
        if let Ok(mut m) = self.inflight.lock() {
            m.insert(id_key(&id), call.clone());
        }
        let me = self.clone();
        let key = id_key(&id);
        let id_in_thread = id.clone();
        let spawned = std::thread::Builder::new().name("ag-mcp-call".into()).spawn(move || {
            let (id, key) = (id_in_thread, key);
            let started = Instant::now();
            let result = catch_unwind(AssertUnwindSafe(|| match method.as_str() {
                "tools/call" => me.run_tool(&params, ver, &call),
                "resources/list" => me.run_resources_list(ver, &call),
                _ => me.run_resources_read(&params, ver, &call),
            }));
            let result = result.unwrap_or_else(|_| {
                log::error(&format!("{method} panicked"));
                Err(RpcError::new(INTERNAL_ERROR, "internal error"))
            });
            if let Ok(mut m) = me.inflight.lock() {
                m.remove(&key);
            }
            if call.cancelled.load(Ordering::SeqCst) {
                log::debug(&format!(
                    "{method} {id}: cancelled; no response sent ({} ms)",
                    started.elapsed().as_millis()
                ));
            } else {
                me.reply(&id, result);
            }
            me.active.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            self.active.fetch_sub(1, Ordering::SeqCst);
            if let Ok(mut m) = self.inflight.lock() {
                m.remove(&id_key(&id));
            }
            self.reply(&id, Err(RpcError::new(INTERNAL_ERROR, "cannot start a worker thread")));
        }
    }

    fn progress_fn(&self, token: Option<Value>) -> Box<crate::tools::Progress> {
        let out = self.out.clone();
        let last = AtomicI64::new(-1);
        Box::new(move |progress, total, message| {
            let Some(t) = &token else { return };
            let p = (progress * 1000.0) as i64;
            if p <= last.load(Ordering::SeqCst) {
                return;
            }
            last.store(p, Ordering::SeqCst);
            let mut params = json!({"progressToken": t, "progress": progress, "message": message});
            if let Some(total) = total {
                params["total"] = json!(total);
            }
            out.send(&jsonrpc::notification("notifications/progress", params));
        })
    }

    fn run_tool(&self, params: &Value, ver: Version, call: &InFlight) -> Result<Value, RpcError> {
        let started = Instant::now();
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return Err(RpcError::invalid_params("`name` (the tool to call) is required"));
        };
        let args = match params.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(a) if a.is_object() => a.clone(),
            Some(_) => return Err(RpcError::invalid_params("`arguments` must be an object")),
        };
        let Some(tool) = self.registry.get(name) else {
            return Err(RpcError::invalid_params(format!("Unknown tool: {}", crate::sanitize::attr_value(name))));
        };
        let finish_err = |e: ToolError, started: Instant| {
            log::info(&format!("tool={name} result=error:{} ms={}", e.code, started.elapsed().as_millis()));
            Ok(self.finish(ver, render_err(&e, ""), None))
        };
        if !self.cfg.allows(tool.group) {
            let why = match tool.group {
                Group::Integrate => {
                    "Landing a proposal is deliberately not available to this agent. Hand the proposal id to a person or the integrator."
                }
                Group::Review => "Reviews are not enabled in this session; an author must not review its own work.",
                _ => "The operator did not enable this part of the toolset.",
            };
            return finish_err(
                ToolError::refused(
                    "tool_disabled",
                    format!(
                        "The tool `{name}` is switched off by the operator of this MCP server (group `{}` is not in its allow-list), so nothing was sent to AgentGit.",
                        tool.group
                    ),
                )
                .hint(why),
                started,
            );
        }
        let errors = schema::validate(&tool.input, &args);
        if !errors.is_empty() {
            return finish_err(
                ToolError::invalid(format!("The arguments of `{name}` are invalid: {}", errors.join("; ")))
                    .hint("Fix the arguments and call again. Nothing was sent to AgentGit."),
                started,
            );
        }
        let progress = self.progress_fn(params.get("_meta").and_then(|m| m.get("progressToken")).cloned());
        let ctx = CallCtx { cfg: &self.cfg, api: &self.api, cancel: &call.cancelled, progress: progress.as_ref() };
        if tool.group == Group::Integrate {
            log::warn(&format!("integrate tool called ({name})"));
        }
        match (tool.handler)(&ctx, &args) {
            Ok(out) => {
                let (result, stats) = render_ok(out, self.cfg.max_output_bytes, &self.api_secret(), ver);
                log::info(&format!(
                    "tool={name} result=ok ms={} sanitized={} truncated={}",
                    started.elapsed().as_millis(),
                    !stats.report.is_clean(),
                    stats.truncated
                ));
                Ok(self.finish(ver, result, None))
            }
            Err(e) => {
                let result = render_err(&e, &self.api_secret());
                log::info(&format!("tool={name} result=error:{} ms={}", e.code, started.elapsed().as_millis()));
                Ok(self.finish(ver, result, None))
            }
        }
    }

    fn api_secret(&self) -> String {
        self.api.secret()
    }

    fn run_resources_list(&self, ver: Version, call: &InFlight) -> Result<Value, RpcError> {
        let progress = self.progress_fn(None);
        let ctx = CallCtx { cfg: &self.cfg, api: &self.api, cancel: &call.cancelled, progress: progress.as_ref() };
        let list = resources::list(&ctx)
            .map_err(|e| RpcError::new(INTERNAL_ERROR, e.message).with_data(json!({"code": e.code})))?;
        Ok(self.finish(ver, json!({"resources": list}), Some(Cache::Private(0))))
    }

    fn run_resources_read(&self, params: &Value, ver: Version, call: &InFlight) -> Result<Value, RpcError> {
        let Some(uri) = params.get("uri").and_then(Value::as_str) else {
            return Err(RpcError::invalid_params("`uri` is required"));
        };
        let progress = self.progress_fn(None);
        let ctx = CallCtx { cfg: &self.cfg, api: &self.api, cancel: &call.cancelled, progress: progress.as_ref() };
        let contents = resources::read(&ctx, uri)?;
        Ok(self.finish(ver, json!({"contents": contents}), Some(Cache::Private(0))))
    }
}

/// Lists are not paginated here, so a cursor can only be invalid.
fn no_cursor(params: &Value) -> Result<(), RpcError> {
    match params.get("cursor") {
        None | Some(Value::Null) => Ok(()),
        Some(_) => Err(RpcError::invalid_params("invalid cursor: this server returns every item in one page")),
    }
}

/// The revisions this server speaks (for `--version`-style output).
pub fn supported_versions() -> Vec<&'static str> {
    SUPPORTED.iter().map(|v| v.as_str()).collect()
}
