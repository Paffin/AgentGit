//! A small client for AGWP v1 over HTTP/JSON.
//!
//! One bearer credential, no redirects, bounded response sizes, retries only where repeating the request cannot
//! do harm (reads, and creations that carry an idempotency key), and a single re-read of the credential file after a
//! `401` so that an operator can rotate an expired credential without restarting the agent session.

use std::collections::HashMap;
use std::io::Read as _;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::Duration;

use serde_json::Value;

use crate::config::{Config, TokenSource};
use crate::sanitize::scrub_secrets;

/// A failed call, shaped like AGWP's `ErrorInfo` (plus transport failures with `status == 0`).
#[derive(Debug, Clone)]
pub struct ApiError {
    /// HTTP status; `0` when no response arrived.
    pub status: u16,
    /// AGWP error code (`permission_denied`, `stream_moved`, …) or `unavailable` / `deadline_exceeded` for transport failures.
    pub code: String,
    /// The server's message (not trusted: it may echo caller-supplied text).
    pub message: String,
    /// The server's hint that the logical operation may succeed if repeated.
    pub retryable: bool,
    /// Backpressure hint in milliseconds.
    pub retry_after_ms: u64,
    /// The server's request id, for the operator's logs.
    pub request_id: String,
    /// Structured details.
    pub details: Value,
}

impl ApiError {
    fn transport(code: &str, message: String) -> Self {
        Self {
            status: 0,
            code: code.into(),
            message,
            retryable: true,
            retry_after_ms: 0,
            request_id: String::new(),
            details: Value::Null,
        }
    }

    /// A client-side failure that never reached the server.
    pub fn local(code: &str, message: impl Into<String>) -> Self {
        Self { retryable: false, ..Self::transport(code, message.into()) }
    }
}

/// Per-call options.
#[derive(Default, Clone)]
pub struct Opts<'a> {
    /// Act as this agent run (`ag-run`).
    pub run_id: Option<&'a str>,
    /// Makes a creation safe to repeat after a lost response.
    pub idempotency_key: Option<&'a str>,
    /// Set when the MCP client cancelled the request: retries and sleeps stop.
    pub cancel: Option<&'a AtomicBool>,
}

impl Opts<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
    }
}

/// The AGWP client.
pub struct Api {
    agent: ureq::Agent,
    base: String,
    token: RwLock<String>,
    /// Credentials bound to one run each (ADR-0010), by run id: calls that act as a run use theirs instead of the
    /// agent's own. They live in this process only and are never printed.
    run_tokens: RwLock<HashMap<String, String>>,
    /// The features the server advertised, read once.
    features: Mutex<Option<Vec<String>>>,
    source: TokenSource,
    timeout: Duration,
    counter: AtomicU64,
}

const MAX_JSON_RESPONSE: u64 = 16 * 1024 * 1024;

/// Percent-encodes one URL path segment or query component.
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `/v1/repos/{owner}/{name}` for a validated repository name.
pub fn repo_path(repo: &str) -> String {
    let (o, n) = repo.split_once('/').unwrap_or((repo, ""));
    format!("/v1/repos/{}/{}", enc(o), enc(n))
}

/// A repository file path as URL path segments.
pub fn file_path_segments(path: &str) -> String {
    path.split('/').map(enc).collect::<Vec<_>>().join("/")
}

impl Api {
    /// Builds the client from the configuration and loads the credential.
    pub fn new(cfg: &Config) -> Result<Self, String> {
        let token = cfg.token.load()?;
        let mut tls = ureq::tls::TlsConfig::builder();
        if let Some(ca) = &cfg.ca_file {
            let pem = std::fs::read(ca).map_err(|e| format!("cannot read --ca-file {}: {e}", ca.display()))?;
            let certs: Vec<_> = ureq::tls::parse_pem(&pem)
                .filter_map(|i| match i {
                    Ok(ureq::tls::PemItem::Certificate(c)) => Some(c),
                    _ => None,
                })
                .collect();
            if certs.is_empty() {
                return Err(format!("--ca-file {} contains no certificate", ca.display()));
            }
            tls = tls.root_certs(ureq::tls::RootCerts::new_with_certs(&certs));
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .timeout_global(Some(cfg.timeout))
            .user_agent(format!("ag-mcp/{}", env!("CARGO_PKG_VERSION")))
            .tls_config(tls.build())
            .build()
            .into();
        Ok(Self {
            agent,
            base: cfg.base_url().to_string(),
            token: RwLock::new(token),
            run_tokens: RwLock::new(HashMap::new()),
            features: Mutex::new(None),
            source: cfg.token.clone(),
            timeout: cfg.timeout,
            counter: AtomicU64::new(0),
        })
    }

    fn token(&self) -> String {
        self.token.read().map(|t| t.clone()).unwrap_or_default()
    }

    /// The current credential, for scrubbing it out of text. Never put it into output.
    pub(crate) fn secret(&self) -> String {
        self.token()
    }

    /// Replaces any occurrence of the credential in `s`.
    pub fn scrub(&self, s: &str) -> String {
        let s = scrub_secrets(s, &self.token());
        // The credentials of runs have the shape of any other and are scrubbed by it; the exact value goes too.
        let runs = self.run_tokens.read().map(|m| m.values().cloned().collect::<Vec<_>>()).unwrap_or_default();
        runs.iter().fold(s, |s, t| scrub_secrets(&s, t))
    }

    /// The credential bound to `run`, if this process minted one.
    pub fn run_token(&self, run: &str) -> Option<String> {
        self.run_tokens.read().ok().and_then(|m| m.get(run).cloned())
    }

    /// Makes calls that act as `run` use `token`, a credential bound to that run.
    pub fn bind_run(&self, run: &str, token: String) {
        if let Ok(mut m) = self.run_tokens.write() {
            m.insert(run.to_string(), token);
        }
    }

    /// Forgets the credential of a run that ended.
    pub fn unbind_run(&self, run: &str) {
        if let Ok(mut m) = self.run_tokens.write() {
            m.remove(run);
        }
    }

    /// Whether the server advertises `feature` (`GET /v1/capabilities`, asked once and remembered).
    pub fn supports(&self, feature: &str, o: &Opts<'_>) -> Result<bool, ApiError> {
        if let Some(f) = self.features.lock().ok().and_then(|g| g.clone()) {
            return Ok(f.iter().any(|x| x == feature));
        }
        let caps = self.get("/v1/capabilities", &[], o)?;
        let features: Vec<String> = caps
            .get("features")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|f| f.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let has = features.iter().any(|x| x == feature);
        if let Ok(mut g) = self.features.lock() {
            *g = Some(features);
        }
        Ok(has)
    }

    fn request_id(&self) -> String {
        format!("mcp-{:x}-{}", std::process::id(), self.counter.fetch_add(1, Ordering::Relaxed))
    }

    /// `GET path?query` returning JSON.
    pub fn get(&self, path: &str, query: &[(&str, String)], o: &Opts<'_>) -> Result<Value, ApiError> {
        let (_, body) = self.exec("GET", path, query, None, o, MAX_JSON_RESPONSE)?;
        parse_json(&body)
    }

    /// A request with a JSON body (or none) returning JSON.
    pub fn send(&self, method: &str, path: &str, body: Option<&Value>, o: &Opts<'_>) -> Result<Value, ApiError> {
        let (_, bytes) = self.exec(method, path, &[], body, o, MAX_JSON_RESPONSE)?;
        parse_json(&bytes)
    }

    /// `GET` a binary body (a file), reading at most `limit` bytes; the flag says the body was longer.
    pub fn get_bytes(
        &self,
        path: &str,
        query: &[(&str, String)],
        limit: usize,
        o: &Opts<'_>,
    ) -> Result<(Vec<u8>, bool), ApiError> {
        let (_, bytes) = self.exec("GET", path, query, None, o, limit as u64 + 1)?;
        let over = bytes.len() > limit;
        Ok((if over { bytes[..limit].to_vec() } else { bytes }, over))
    }

    fn exec(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
        o: &Opts<'_>,
        max_body: u64,
    ) -> Result<(u16, Vec<u8>), ApiError> {
        let safe_to_repeat = method == "GET" || o.idempotency_key.is_some();
        let mut url = format!("{}{}", self.base, path);
        if !query.is_empty() {
            let qs: Vec<String> = query.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect();
            url.push('?');
            url.push_str(&qs.join("&"));
        }
        let rid = self.request_id();
        let payload = body.map(|b| b.to_string());
        let mut attempt = 0u32;
        let mut refreshed = false;
        loop {
            attempt += 1;
            if o.cancelled() {
                return Err(ApiError::local("cancelled", "the request was cancelled"));
            }
            // A call that acts as a run with a credential of its own uses that one - and only that one.
            let bound = o.run_id.and_then(|r| self.run_token(r));
            let used_token = bound.clone().unwrap_or_else(|| self.token());
            let res = self.once(method, &url, &rid, payload.as_deref(), o, &used_token, max_body);
            let err = match res {
                Ok((status, bytes)) if status < 300 => return Ok((status, bytes)),
                Ok((status, bytes)) => self.error_from_response(status, &bytes),
                Err(e) => e,
            };
            if err.status == 401 && !refreshed && bound.is_none() && self.source.refreshable() {
                refreshed = true;
                if let Ok(fresh) = self.source.load()
                    && fresh != used_token
                {
                    if let Ok(mut w) = self.token.write() {
                        *w = fresh;
                    }
                    continue;
                }
            }
            let transient = matches!(err.status, 0 | 429 | 502 | 503 | 504);
            if transient && safe_to_repeat && attempt < 3 && !o.cancelled() {
                let wait =
                    if err.retry_after_ms > 0 { err.retry_after_ms.min(2000) } else { 200 * (1 << (attempt - 1)) };
                if !sleep_unless_cancelled(wait, o) {
                    return Err(ApiError::local("cancelled", "the request was cancelled"));
                }
                continue;
            }
            return Err(err);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn once(
        &self,
        method: &str,
        url: &str,
        rid: &str,
        payload: Option<&str>,
        o: &Opts<'_>,
        token: &str,
        max_body: u64,
    ) -> Result<(u16, Vec<u8>), ApiError> {
        let mut b = ureq::http::Request::builder()
            .method(method)
            .uri(url)
            .header("authorization", format!("Bearer {token}"))
            .header("accept", "application/json, application/octet-stream")
            .header("x-request-id", rid)
            .header("request-timeout", self.timeout.as_millis().to_string());
        if let Some(run) = o.run_id {
            b = b.header("ag-run", run);
        }
        if let Some(k) = o.idempotency_key {
            b = b.header("idempotency-key", k);
        }
        let result = match payload {
            Some(p) => self.agent.run(
                b.header("content-type", "application/json")
                    .body(p.to_string())
                    .map_err(|e| ApiError::local("invalid_argument", e.to_string()))?,
            ),
            None => self.agent.run(b.body(()).map_err(|e| ApiError::local("invalid_argument", e.to_string()))?),
        };
        let mut resp = result.map_err(|e| self.transport_error(&e))?;
        let status = resp.status().as_u16();
        // Read at most `max_body` bytes and drop the rest of the connection: a hostile or broken server cannot make
        // this process buffer an unbounded body.
        let mut bytes = Vec::new();
        std::io::Read::take(resp.body_mut().with_config().limit(u64::MAX).reader(), max_body)
            .read_to_end(&mut bytes)
            .map_err(|e| {
            ApiError::transport("unavailable", format!("reading the response failed: {}", self.scrub(&e.to_string())))
        })?;
        Ok((status, bytes))
    }

    fn transport_error(&self, e: &ureq::Error) -> ApiError {
        let msg = self.scrub(&e.to_string());
        match e {
            ureq::Error::Timeout(_) => ApiError::transport(
                "deadline_exceeded",
                format!("the AgentGit server did not answer in time ({msg}); a write may or may not have taken effect"),
            ),
            _ => ApiError::transport("unavailable", format!("cannot reach the AgentGit server: {msg}")),
        }
    }

    fn error_from_response(&self, status: u16, bytes: &[u8]) -> ApiError {
        if (300..400).contains(&status) {
            return ApiError {
                status,
                code: "unexpected_redirect".into(),
                message:
                    "the server answered with a redirect, which ag-mcp never follows (the credential would travel \
                          with it); check --server"
                        .into(),
                retryable: false,
                retry_after_ms: 0,
                request_id: String::new(),
                details: Value::Null,
            };
        }
        let parsed: Option<Value> = serde_json::from_slice(bytes).ok();
        let info = parsed.as_ref().and_then(|v| v.get("error")).cloned().unwrap_or(Value::Null);
        let s = |k: &str| info.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let code = match (s("code"), status) {
            (c, _) if !c.is_empty() => c,
            (_, 401) => "unauthenticated".into(),
            (_, 403) => "permission_denied".into(),
            (_, 404) => "not_found".into(),
            (_, 429) => "rate_limited".into(),
            (_, 503) => "unavailable".into(),
            _ => "internal".into(),
        };
        let message = {
            let m = s("message");
            if m.is_empty() { format!("HTTP {status}") } else { self.scrub(&m) }
        };
        ApiError {
            status,
            code,
            message,
            retryable: info.get("retryable").and_then(Value::as_bool).unwrap_or(false),
            retry_after_ms: info.get("retry_after_ms").and_then(Value::as_u64).unwrap_or(0),
            request_id: s("request_id"),
            details: info.get("details").cloned().unwrap_or(Value::Null),
        }
    }
}

fn parse_json(bytes: &[u8]) -> Result<Value, ApiError> {
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    if bytes.len() as u64 >= MAX_JSON_RESPONSE {
        return Err(ApiError {
            status: 200,
            code: "too_large".into(),
            message: "the server's answer is larger than ag-mcp accepts (16 MiB); ask for a smaller page".into(),
            retryable: false,
            retry_after_ms: 0,
            request_id: String::new(),
            details: Value::Null,
        });
    }
    serde_json::from_slice(bytes).map_err(|e| ApiError {
        status: 200,
        code: "bad_response".into(),
        message: format!("the server sent a body that is not JSON: {e}"),
        retryable: false,
        retry_after_ms: 0,
        request_id: String::new(),
        details: Value::Null,
    })
}

/// Sleeps in small steps; false if cancelled meanwhile.
fn sleep_unless_cancelled(ms: u64, o: &Opts<'_>) -> bool {
    let mut left = ms;
    while left > 0 {
        if o.cancelled() {
            return false;
        }
        let step = left.min(25);
        std::thread::sleep(Duration::from_millis(step));
        left -= step;
    }
    !o.cancelled()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_is_strict() {
        assert_eq!(enc("a b/c?d=e&f"), "a%20b%2Fc%3Fd%3De%26f");
        assert_eq!(enc("ünï"), "%C3%BCn%C3%AF");
        assert_eq!(enc("safe-._~09AZ"), "safe-._~09AZ");
        assert_eq!(repo_path("acme/web"), "/v1/repos/acme/web");
        assert_eq!(file_path_segments("src/a b/ü.rs"), "src/a%20b/%C3%BC.rs");
    }
}
