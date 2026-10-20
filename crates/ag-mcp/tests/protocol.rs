//! The MCP protocol layer on its own: version negotiation, JSON-RPC error codes, message bounds, cancellation,
//! progress, the concurrency limit and panics. The first half drives the real `ag-mcp` binary over stdio; the
//! second half drives the library `Server` with tools defined in this file (the protocol layer does not depend on
//! what the tools do, so these tests need no AgentGit server).
#![allow(missing_docs, clippy::result_large_err)]

mod common;

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ag_mcp::Server;
use ag_mcp::config::{Config, Group, TokenSource};
use ag_mcp::output::ToolOutput;
use ag_mcp::tools::{self, Ann, Registry};
use common::*;
use serde_json::{Value, json};

const NOWHERE: &str = "http://127.0.0.1:1";
const TOKEN: &str = "agt_0123456789abcdef0123456789abcdef";

/// `ag-mcp` pointed at a server that does not exist: none of these tests may need it.
fn bare(extra: &[&str]) -> Mcp {
    let mut args = vec!["--server", NOWHERE];
    args.extend_from_slice(extra);
    Mcp::spawn(&args, &[("AG_MCP_TOKEN", TOKEN)])
}

fn error_of(r: &Value) -> (i64, String) {
    (r["error"]["code"].as_i64().unwrap_or(0), r["error"]["message"].as_str().unwrap_or("").to_string())
}

// ---------------------------------------------------------------------------------------------------------
// the binary

#[test]
fn discover_answers_without_a_handshake_and_names_every_revision() {
    let mut m = bare(&[]);
    let r = m.request("server/discover", json!({}));
    let res = &r["result"];
    assert_eq!(res["supportedVersions"], json!(["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26"]));
    assert_eq!(res["resultType"], "complete");
    assert!(res["capabilities"]["tools"].is_object() && res["capabilities"]["resources"].is_object());
    assert!(res["instructions"].as_str().unwrap().contains("task_claim"));
    assert_eq!(res["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], "ag-mcp");
    assert_eq!(res["cacheScope"], "public");
    assert!(res["ttlMs"].as_u64().unwrap() > 0);
    let id = r["id"].clone();
    assert_eq!(id, json!(1));
}

#[test]
fn the_read_group_off_removes_resources_from_the_capabilities() {
    let mut m = bare(&["--allow", "propose"]);
    let r = m.request("server/discover", json!({}));
    assert!(r["result"]["capabilities"].get("resources").is_none(), "{r}");
    let r = m.request("resources/list", json!({}));
    assert_eq!(error_of(&r).0, -32601, "{r}");
}

#[test]
fn legacy_initialize_negotiates_the_newest_revision_at_or_below_the_request() {
    for (asked, answered) in [
        ("2025-11-25", "2025-11-25"),
        ("2025-06-18", "2025-06-18"),
        ("2025-03-26", "2025-03-26"),
        // Unknown or older than anything served: the server answers with the newest handshake revision it has and the
        // client decides whether it can live with it.
        ("2024-11-05", "2025-11-25"),
        ("2099-01-01", "2025-11-25"),
        // The stateless revision has no `initialize`: a client asking for it by handshake gets the last handshake one.
        ("2026-07-28", "2025-11-25"),
        ("garbage", "2025-11-25"),
    ] {
        let mut m = bare(&[]);
        let r = m.initialize_legacy(asked);
        assert_eq!(r["result"]["protocolVersion"], answered, "asked {asked}: {r}");
        assert_eq!(r["result"]["serverInfo"]["name"], "ag-mcp");
        assert!(r["result"].get("resultType").is_none(), "legacy results carry no resultType: {r}");
        // After the handshake requests carry no _meta at all.
        let t = m.request("tools/list", json!({}));
        assert!(t["result"]["tools"].as_array().unwrap().len() >= 20, "{t}");
        let has_schema = t["result"]["tools"][0].get("outputSchema").is_some();
        assert_eq!(has_schema, answered != "2025-03-26", "outputSchema only from 2025-06-18: {asked}");
        // `server/discover` belongs to the stateless revision.
        let d = m.request("server/discover", json!({}));
        assert_eq!(error_of(&d).0, -32601, "{d}");
    }
}

#[test]
fn initialize_without_a_protocol_version_is_invalid_params() {
    let mut m = bare(&[]);
    m.modern = false;
    let r = m.request("initialize", json!({"capabilities": {}}));
    assert_eq!(error_of(&r).0, -32602, "{r}");
}

#[test]
fn requests_without_a_handshake_or_version_are_refused_with_a_hint_but_ping_always_works() {
    let mut m = bare(&[]);
    m.modern = false;
    let r = m.request("tools/list", json!({}));
    let (code, msg) = error_of(&r);
    assert_eq!(code, -32602, "{r}");
    assert!(msg.contains("server/discover") && msg.contains("initialize"), "{msg}");
    let p = m.request("ping", json!({}));
    assert_eq!(p["result"], json!({}), "{p}");
}

#[test]
fn an_unsupported_version_is_32022_with_the_supported_list() {
    let mut m = bare(&[]);
    m.modern = false;
    let r = m.request(
        "tools/list",
        json!({"_meta": {"io.modelcontextprotocol/protocolVersion": "2024-01-01", "io.modelcontextprotocol/clientCapabilities": {}}}),
    );
    assert_eq!(error_of(&r).0, -32022, "{r}");
    assert_eq!(r["error"]["data"]["supported"][0], "2026-07-28");
    assert_eq!(r["error"]["data"]["requested"], "2024-01-01");
    // The version must be a string, and the stateless revision needs the client capabilities on every request.
    let r = m.request("tools/list", json!({"_meta": {"io.modelcontextprotocol/protocolVersion": 20260728}}));
    assert_eq!(error_of(&r).0, -32602, "{r}");
    let r = m.request("tools/list", json!({"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}}));
    let (code, msg) = error_of(&r);
    assert_eq!(code, -32602, "{r}");
    assert!(msg.contains("clientCapabilities"), "{msg}");
}

#[test]
fn json_rpc_errors_use_the_standard_codes_and_the_id_when_it_can_be_read() {
    let mut m = bare(&[]);
    // Parse error: no id can be known.
    m.write_raw("{not json");
    let r = m.recv().unwrap();
    assert_eq!((error_of(&r).0, r["id"].clone()), (-32700, Value::Null), "{r}");
    // A batch is refused (MCP dropped batching in 2025-06-18).
    m.write_raw(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#);
    let r = m.recv().unwrap();
    assert_eq!((error_of(&r).0, r["id"].clone()), (-32600, Value::Null), "{r}");
    // Not an object.
    m.write_raw("42");
    assert_eq!(error_of(&m.recv().unwrap()).0, -32600);
    // Bad ids: null and fractions.
    m.write_raw(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#);
    assert_eq!(error_of(&m.recv().unwrap()).0, -32600);
    m.write_raw(r#"{"jsonrpc":"2.0","id":1.5,"method":"ping"}"#);
    assert_eq!(error_of(&m.recv().unwrap()).0, -32600);
    // Wrong or missing version member: the id is echoed.
    m.write_raw(r#"{"jsonrpc":"1.0","id":7,"method":"ping"}"#);
    let r = m.recv().unwrap();
    assert_eq!((error_of(&r).0, r["id"].clone()), (-32600, json!(7)), "{r}");
    m.write_raw(r#"{"id":8,"method":"ping"}"#);
    assert_eq!(error_of(&m.recv().unwrap()).0, -32600);
    // Missing and non-string method.
    m.write_raw(r#"{"jsonrpc":"2.0","id":9}"#);
    assert_eq!(error_of(&m.recv().unwrap()).0, -32600);
    m.write_raw(r#"{"jsonrpc":"2.0","id":10,"method":5}"#);
    assert_eq!(error_of(&m.recv().unwrap()).0, -32600);
    // params must be an object.
    m.write_raw(r#"{"jsonrpc":"2.0","id":11,"method":"ping","params":[1]}"#);
    let r = m.recv().unwrap();
    assert_eq!((error_of(&r).0, r["id"].clone()), (-32602, json!(11)), "{r}");
    // A string id is echoed as a string.
    m.write_raw(r#"{"jsonrpc":"2.0","id":"abc","method":"ping"}"#);
    let r = m.recv().unwrap();
    assert_eq!((r["id"].clone(), r["result"].clone()), (json!("abc"), json!({})), "{r}");

    // Method-level errors (modern requests, so the version is not the problem).
    let r = m.request("no/such/method", json!({}));
    let (code, msg) = error_of(&r);
    assert_eq!(code, -32601, "{r}");
    assert!(msg.contains("no/such/method"));
    let r = m.request("tools/call", json!({"name": "no_such_tool", "arguments": {}}));
    assert_eq!(error_of(&r).0, -32602, "unknown tool is invalid params: {r}");
    let r = m.request("tools/call", json!({"arguments": {}}));
    assert_eq!(error_of(&r).0, -32602, "{r}");
    let r = m.request("tools/call", json!({"name": "whoami", "arguments": [1]}));
    assert_eq!(error_of(&r).0, -32602, "{r}");
    let r = m.request("tools/list", json!({"cursor": "abc"}));
    assert_eq!(error_of(&r).0, -32602, "{r}");
    let r = m.request("prompts/get", json!({"name": "no_such_prompt"}));
    assert_eq!(error_of(&r).0, -32602, "{r}");
    let r = m.request("resources/read", json!({"uri": "file:///etc/passwd"}));
    assert_eq!(error_of(&r).0, -32602, "a foreign URI is invalid params: {r}");
    let r = m.request("resources/read", json!({}));
    assert_eq!(error_of(&r).0, -32602, "{r}");

    // Nothing above made the process unusable.
    assert_eq!(m.request("ping", json!({}))["result"], json!({}));
}

#[test]
fn arguments_that_break_the_tool_schema_are_a_tool_error_that_never_reaches_the_server() {
    let mut m = bare(&[]);
    let r = m.call("read_file", json!({"path": 5, "unexpected": true}));
    assert!(r.is_error);
    assert_eq!(r.error_code(), "invalid_arguments", "{}", r.text);
    assert!(r.text.contains("Nothing was sent to AgentGit"), "{}", r.text);
}

#[test]
fn notifications_get_no_response() {
    let mut m = bare(&[]);
    m.notify("notifications/initialized", json!({}));
    m.notify("notifications/roots/list_changed", json!({}));
    m.notify("notifications/cancelled", json!({"requestId": 999}));
    // A response from the client to a request the server never sent is ignored too.
    m.write_raw(r#"{"jsonrpc":"2.0","id":5,"result":{}}"#);
    let r = m.request("ping", json!({}));
    assert_eq!(r["result"], json!({}));
    assert!(m.notifications.is_empty(), "{:?}", m.notifications);
}

#[test]
fn an_oversized_line_is_discarded_with_an_error_and_the_next_message_still_works() {
    let mut m = bare(&["--max-message-bytes", "1024"]);
    let big = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"ping","params":{{"pad":"{}"}}}}"#, "x".repeat(5000));
    m.write_raw(&big);
    let r = m.recv().unwrap();
    let (code, msg) = error_of(&r);
    assert_eq!((code, r["id"].clone()), (-32600, Value::Null), "{r}");
    assert!(msg.contains("1024"), "{msg}");
    // Exactly at the limit is accepted; one byte more is not.
    let base = r#"{"jsonrpc":"2.0","id":2,"method":"ping","params":{"p":""}}"#;
    let pad = "y".repeat(1024 - base.len());
    let ok = base.replace(r#""p":"""#, &format!(r#""p":"{pad}""#));
    assert_eq!(ok.len(), 1024);
    m.write_raw(&ok);
    assert_eq!(m.recv().unwrap()["result"], json!({}));
    m.write_raw(&format!("{ok} "));
    assert_eq!(error_of(&m.recv().unwrap()).0, -32600);
    // The discarded line did not leave half a message behind.
    assert_eq!(m.request("ping", json!({}))["result"], json!({}));
}

#[test]
fn closing_stdin_ends_the_process_cleanly() {
    let mut m = bare(&[]);
    assert_eq!(m.request("ping", json!({}))["result"], json!({}));
    m.close_stdin();
    let started = Instant::now();
    let status = m.wait_exit();
    assert!(status.success(), "{status:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    // The banner and nothing else went to stderr; stdout carried only protocol messages (the harness parses every line).
    assert!(m.stderr_with("serving MCP over stdio").contains("serving MCP over stdio"));
}

#[test]
fn an_unterminated_last_line_is_still_handled() {
    let mut m = bare(&[]);
    m.write_unterminated(r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#);
    m.close_stdin();
    let r = m.recv().expect("answer to the last, unterminated line");
    assert_eq!(r["id"], json!(3));
    assert!(m.wait_exit().success());
}

// ---------------------------------------------------------------------------------------------------------
// the library, with tools of its own

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Buf {
    fn lines(&self) -> Vec<Value> {
        let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
        text.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON {l:?}: {e}"))).collect()
    }
    fn response(&self, id: i64) -> Option<Value> {
        self.lines().into_iter().find(|m| m["id"] == json!(id))
    }
    fn wait_response(&self, id: i64) -> Value {
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(r) = self.response(id) {
                return r;
            }
            assert!(Instant::now() < end, "no response to {id}; got {:?}", self.lines());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[derive(Default)]
struct Probe {
    started: AtomicUsize,
    saw_cancel: AtomicBool,
    finished: AtomicUsize,
}

fn custom_tools(probe: &Arc<Probe>) -> Registry {
    let schema_in = tools::input(vec![("n", tools::t_int("a number", 0, 100))], &[]);
    let schema_out = tools::output(vec![], &[]);
    let p = probe.clone();
    let wait = tools::def(
        "wait_for_cancel",
        Group::Read,
        "Wait",
        "Blocks until the client cancels the call (test tool for the protocol layer).",
        Ann::READ,
        schema_in.clone(),
        schema_out.clone(),
        move |ctx, _| {
            p.started.fetch_add(1, Ordering::SeqCst);
            let end = Instant::now() + Duration::from_secs(20);
            while Instant::now() < end {
                if ctx.check_cancelled().is_err() {
                    p.saw_cancel.store(true, Ordering::SeqCst);
                    p.finished.fetch_add(1, Ordering::SeqCst);
                    return Ok(ToolOutput::new("cancelled", json!({})));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            p.finished.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::new("timed out waiting for a cancellation", json!({})))
        },
    );
    let steps = tools::def(
        "count_steps",
        Group::Read,
        "Steps",
        "Reports progress three times, then returns (test tool for the protocol layer).",
        Ann::READ,
        schema_in.clone(),
        schema_out.clone(),
        |ctx, _| {
            ctx.step(1, 3, "one");
            ctx.step(2, 3, "two");
            // not increasing: must be dropped, progress only moves forward
            ctx.step(2, 3, "two again");
            ctx.step(3, 3, "three");
            Ok(ToolOutput::new("counted", json!({})))
        },
    );
    let boom = tools::def(
        "boom",
        Group::Read,
        "Boom",
        "Panics (test tool for the protocol layer: a bug in a tool must not take the server down).",
        Ann::READ,
        schema_in.clone(),
        schema_out.clone(),
        |_, _| -> Result<ToolOutput, ag_mcp::output::ToolError> { panic!("deliberate panic in a test tool") },
    );
    let echo = tools::def(
        "echo",
        Group::Read,
        "Echo",
        "Returns its argument (test tool for the protocol layer).",
        Ann::READ,
        schema_in,
        schema_out,
        |ctx, a| {
            ctx.step(1, 2, "working");
            ctx.step(2, 2, "done");
            Ok(ToolOutput::new(format!("echo {}", a["n"]), json!({"n": a["n"]})))
        },
    );
    Registry::from_tools(vec![wait, steps, boom, echo])
}

fn lib_server(max_concurrent: usize) -> (Arc<Server>, Buf, Arc<Probe>) {
    let probe = Arc::new(Probe::default());
    let mut cfg = Config::new(NOWHERE, TokenSource::Fixed(TOKEN.into()));
    cfg.max_concurrent = max_concurrent;
    let buf = Buf::default();
    let server = Server::new(cfg, custom_tools(&probe), Box::new(buf.clone())).unwrap();
    (server, buf, probe)
}

fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

fn call(server: &Arc<Server>, id: i64, tool: &str, args: Value, extra_meta: Value) {
    let mut m = meta();
    if let (Some(o), Some(e)) = (m.as_object_mut(), extra_meta.as_object()) {
        o.extend(e.clone());
    }
    let req = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {"name": tool, "arguments": args, "_meta": m}});
    server.handle_line(req.to_string().as_bytes());
}

#[test]
fn a_cancelled_call_gets_no_response_and_stops_working() {
    let (server, buf, probe) = lib_server(8);
    call(&server, 7, "wait_for_cancel", json!({}), json!({}));
    wait_until("the tool to start", || probe.started.load(Ordering::SeqCst) == 1);
    // The cancellation of another request, an unknown one, and one without an id change nothing.
    server.handle_line(br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":8}}"#);
    server.handle_line(br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{}}"#);
    assert!(!probe.saw_cancel.load(Ordering::SeqCst));
    server.handle_line(br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7,"reason":"user pressed stop"}}"#);
    wait_until("the tool to see the cancellation", || probe.saw_cancel.load(Ordering::SeqCst));
    // The server is still serving, and later calls are answered.
    call(&server, 9, "echo", json!({"n": 4}), json!({}));
    let r = buf.wait_response(9);
    assert_eq!(r["result"]["structuredContent"]["n"], 4, "{r}");
    std::thread::sleep(Duration::from_millis(150));
    assert!(buf.response(7).is_none(), "a cancelled request must not be answered: {:?}", buf.lines());
}

#[test]
fn a_cancellation_that_arrives_after_the_answer_is_harmless() {
    let (server, buf, _) = lib_server(8);
    call(&server, 1, "echo", json!({"n": 1}), json!({}));
    buf.wait_response(1);
    server.handle_line(br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#);
    call(&server, 2, "echo", json!({"n": 2}), json!({}));
    assert_eq!(buf.wait_response(2)["result"]["structuredContent"]["n"], 2);
}

#[test]
fn progress_is_reported_only_when_asked_for_and_only_forward() {
    let (server, buf, _) = lib_server(8);
    call(&server, 1, "count_steps", json!({}), json!({"progressToken": "tok-1"}));
    let r = buf.wait_response(1);
    assert!(!r["result"]["isError"].as_bool().unwrap_or(false), "{r}");
    let progress: Vec<Value> = buf.lines().into_iter().filter(|m| m["method"] == "notifications/progress").collect();
    let seen: Vec<(f64, String)> = progress
        .iter()
        .map(|m| (m["params"]["progress"].as_f64().unwrap(), m["params"]["message"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(seen, vec![(1.0, "one".into()), (2.0, "two".into()), (3.0, "three".into())], "{progress:?}");
    for m in &progress {
        assert_eq!(m["params"]["progressToken"], "tok-1");
        assert_eq!(m["params"]["total"], 3.0);
        assert!(m.get("id").is_none(), "progress is a notification");
    }
    // Every progress notification precedes the response it belongs to.
    let lines = buf.lines();
    let pos_resp = lines.iter().position(|m| m["id"] == json!(1)).unwrap();
    assert!(lines.iter().take(pos_resp).filter(|m| m["method"] == "notifications/progress").count() == 3);

    // No token, no progress. An integer token is echoed as an integer.
    let before = lines.len();
    call(&server, 2, "count_steps", json!({}), json!({}));
    buf.wait_response(2);
    assert_eq!(buf.lines().len(), before + 1, "only the response: {:?}", buf.lines());
    call(&server, 3, "echo", json!({"n": 1}), json!({"progressToken": 42}));
    buf.wait_response(3);
    assert!(buf.lines().iter().any(|m| m["method"] == "notifications/progress" && m["params"]["progressToken"] == 42));
}

#[test]
fn the_concurrency_limit_answers_busy_instead_of_queueing() {
    let (server, buf, probe) = lib_server(1);
    call(&server, 1, "wait_for_cancel", json!({}), json!({}));
    wait_until("the first call", || probe.started.load(Ordering::SeqCst) == 1);
    call(&server, 2, "echo", json!({"n": 1}), json!({}));
    let r = buf.wait_response(2);
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert_eq!(r["result"]["_meta"]["agentgit/error"]["code"], "busy", "{r}");
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("limit 1"));
    // Releasing the slot lets the next call through.
    server.handle_line(br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#);
    wait_until("the first call to finish", || probe.finished.load(Ordering::SeqCst) == 1);
    wait_until("the slot to free up", || {
        call(&server, 100 + buf.lines().len() as i64, "echo", json!({"n": 1}), json!({}));
        std::thread::sleep(Duration::from_millis(20));
        buf.lines().iter().any(|m| m["result"]["structuredContent"]["n"] == 1)
    });
}

#[test]
fn a_panic_in_a_tool_is_an_internal_error_and_the_server_carries_on() {
    let (server, buf, _) = lib_server(2);
    call(&server, 1, "boom", json!({}), json!({}));
    let r = buf.wait_response(1);
    assert_eq!(error_of(&r).0, -32603, "{r}");
    assert_eq!(error_of(&r).1, "internal error", "the panic message must not leak: {r}");
    // The slot was given back: with a limit of 2, three more calls in a row all work.
    for id in 2..5 {
        call(&server, id, "echo", json!({"n": id}), json!({}));
        assert_eq!(buf.wait_response(id)["result"]["structuredContent"]["n"], id);
    }
    call(&server, 5, "boom", json!({}), json!({}));
    assert_eq!(error_of(&buf.wait_response(5)).0, -32603);
}

#[test]
fn concurrent_calls_never_interleave_their_output_lines() {
    let (server, buf, _) = lib_server(16);
    let mut handles = Vec::new();
    for t in 0..8 {
        let s = server.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..25 {
                let id = t * 1000 + i + 1;
                call(&s, id, "echo", json!({"n": 7}), json!({"progressToken": id}));
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    wait_until("all answers", || buf.lines().iter().filter(|m| m.get("id").is_some()).count() >= 200);
    // `lines()` panics on any line that is not one whole JSON message; busy answers are fine, interleaving is not.
    let lines = buf.lines();
    assert!(lines.iter().all(|m| m["jsonrpc"] == "2.0"));
    let answered: std::collections::BTreeSet<i64> = lines.iter().filter_map(|m| m["id"].as_i64()).collect();
    assert_eq!(answered.len(), 200, "every request is answered exactly once");
}

#[test]
fn serve_stops_at_the_end_of_input_after_the_running_calls_finish() {
    let (server, buf, probe) = lib_server(8);
    let input = format!(
        "{}\n{}\n",
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "echo", "arguments": {"n": 3}, "_meta": meta()}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "ping", "params": {"_meta": meta()}}),
    );
    server.serve(std::io::Cursor::new(input.into_bytes())).unwrap();
    // serve() waits for the running call before it returns, so the answer to 1 exists by now.
    assert_eq!(buf.response(1).unwrap()["result"]["structuredContent"]["n"], 3);
    assert!(buf.response(2).is_some());
    assert_eq!(probe.started.load(Ordering::SeqCst), 0);
}
