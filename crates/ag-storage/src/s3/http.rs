//! The HTTP side of the S3 client: addressing, signing, sending, retrying.

use std::sync::Arc;
use std::time::Duration;

use ag_types::Timestamp;

use super::sigv4::{self, Credentials, SigningInput, uri_encode};
use super::{S3Config, xml};
use crate::error::StoreError;

/// How long connecting to the endpoint may take. (The deadline of a whole request, body included, is
/// [`S3Config::request_timeout`].)
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The first retry waits this long; every further one waits twice as long as the previous, up to [`BACKOFF_CAP_MS`].
const BACKOFF_BASE_MS: u64 = 50;
/// The longest a retry waits (before jitter).
const BACKOFF_CAP_MS: u64 = 3000;
/// The doubling stops here: `BACKOFF_BASE_MS << 6` is already above [`BACKOFF_CAP_MS`], and the shift stays small.
const BACKOFF_MAX_DOUBLINGS: u32 = 6;
/// A response body is read up to the largest blob this repository allows, but never below this: list pages and
/// error documents are far smaller.
const MIN_RESPONSE_LIMIT: u64 = 64 << 20;
/// Added to the response limit for the object envelope around a maximum-size blob.
const RESPONSE_SLACK: u64 = 1 << 20;

/// One S3 request. `key` is `None` for bucket-level calls.
pub(crate) struct Request<'a> {
    pub method: &'static str,
    pub key: Option<&'a str>,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: &'a [u8],
}

impl<'a> Request<'a> {
    pub fn new(method: &'static str, key: Option<&'a str>) -> Self {
        Self { method, key, query: Vec::new(), headers: Vec::new(), body: &[] }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    pub fn query(mut self, name: &str, value: impl Into<String>) -> Self {
        self.query.push((name.to_string(), value.into()));
        self
    }

    pub fn body(mut self, body: &'a [u8]) -> Self {
        self.body = body;
        self
    }
}

/// A response with its body read.
pub(crate) struct Response {
    pub status: u16,
    /// Header names are lowercase.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub(crate) struct Client {
    agent: ureq::Agent,
    scheme: String,
    authority: String,
    base_path: String,
    bucket: String,
    path_style: bool,
    region: String,
    credentials: Credentials,
    max_retries: u32,
    body_limit: u64,
}

/// Splits `scheme://authority/base/path` (no query, no userinfo).
fn split_endpoint(endpoint: &str) -> Result<(String, String, String), StoreError> {
    let bad =
        |why: &str| StoreError::Remote { status: None, message: format!("invalid S3 endpoint '{endpoint}': {why}") };
    let (scheme, rest) = endpoint.trim().split_once("://").ok_or_else(|| bad("expected http:// or https://"))?;
    if scheme != "http" && scheme != "https" {
        return Err(bad("the scheme must be http or https"));
    }
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].trim_end_matches('/')),
        None => (rest, ""),
    };
    if authority.is_empty() || authority.contains('@') || authority.contains('?') {
        return Err(bad("expected host[:port] without credentials"));
    }
    Ok((scheme.to_string(), authority.to_string(), path.to_string()))
}

impl Client {
    pub fn new(cfg: &S3Config) -> Result<Self, StoreError> {
        let (scheme, authority, base_path) = split_endpoint(&cfg.endpoint)?;
        if cfg.bucket.is_empty() || cfg.bucket.contains('/') {
            return Err(StoreError::Remote {
                status: None,
                message: format!("invalid S3 bucket name '{}'", cfg.bucket),
            });
        }
        if !cfg.path_style && authority.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return Err(StoreError::Remote {
                status: None,
                message: "virtual-hosted-style addressing needs a DNS name, not an IP address; use path_style".into(),
            });
        }
        let mut tls = ureq::tls::TlsConfig::builder();
        if let Some(path) = &cfg.ca_bundle {
            let pem = std::fs::read(path).map_err(|e| StoreError::Remote {
                status: None,
                message: format!("cannot read the CA bundle {}: {e}", path.display()),
            })?;
            let certs: Vec<_> = ureq::tls::parse_pem(&pem)
                .filter_map(|item| match item {
                    Ok(ureq::tls::PemItem::Certificate(c)) => Some(c),
                    _ => None,
                })
                .collect();
            if certs.is_empty() {
                return Err(StoreError::Remote {
                    status: None,
                    message: format!("{} contains no certificates", path.display()),
                });
            }
            tls = tls.root_certs(ureq::tls::RootCerts::Specific(Arc::new(certs)));
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            // never follow redirects: they would replay our signed headers somewhere else
            .max_redirects(0)
            .max_redirects_will_error(false)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(cfg.request_timeout))
            .user_agent(format!("agentgit/{}", env!("CARGO_PKG_VERSION")))
            .tls_config(tls.build())
            .build()
            .into();
        Ok(Self {
            agent,
            scheme,
            authority,
            base_path,
            bucket: cfg.bucket.clone(),
            path_style: cfg.path_style,
            region: cfg.region.clone(),
            credentials: cfg.credentials.clone(),
            max_retries: cfg.max_retries,
            // The largest answer we expect is an object, whose size the caller bounds.
            body_limit: cfg.limits.max_blob_size.max(MIN_RESPONSE_LIMIT) + RESPONSE_SLACK,
        })
    }

    /// `(host header, canonical URI)` of a request.
    pub(super) fn address(&self, key: Option<&str>) -> (String, String) {
        let key_path = key.map(|k| uri_encode(k, true));
        if self.path_style {
            let mut p = format!("{}/{}", self.base_path, uri_encode(&self.bucket, false));
            if let Some(k) = key_path {
                p.push('/');
                p.push_str(&k);
            }
            (self.authority.clone(), p)
        } else {
            let p = format!("{}/{}", self.base_path, key_path.unwrap_or_default());
            (format!("{}.{}", self.bucket, self.authority), p)
        }
    }

    /// The `x-amz-copy-source` value for an object of this bucket.
    pub fn copy_source(&self, key: &str) -> String {
        format!("/{}/{}", uri_encode(&self.bucket, false), uri_encode(key, true))
    }

    fn once(&self, req: &Request<'_>) -> Result<Response, ureq::Error> {
        let (host, path) = self.address(req.key);
        // The request is signed with the clock's own reading, whatever it is: a clock that is wrong makes the server
        // answer RequestTimeTooSkewed (which `describe` explains), instead of this signing a made-up 1970.
        let date = sigv4::amz_date(Timestamp::now().as_millis().div_euclid(1000));
        let payload_sha = sigv4::sha256_hex(req.body);

        let mut headers: Vec<(String, String)> = req.headers.clone();
        headers.push(("host".into(), host));
        headers.push(("x-amz-content-sha256".into(), payload_sha.clone()));
        headers.push(("x-amz-date".into(), date.clone()));
        if let Some(t) = &self.credentials.session_token {
            headers.push(("x-amz-security-token".into(), t.clone()));
        }
        let signed = sigv4::sign(
            &self.credentials,
            &SigningInput {
                method: req.method,
                canonical_uri: &path,
                query: &req.query,
                headers: &headers,
                payload_sha256: &payload_sha,
                region: &self.region,
                amz_date: &date,
            },
        );

        let mut url =
            format!("{}://{}{}", self.scheme, headers.iter().find(|(k, _)| k == "host").map_or("", |(_, v)| v), path);
        if !req.query.is_empty() {
            let q: Vec<String> =
                req.query.iter().map(|(k, v)| format!("{}={}", uri_encode(k, false), uri_encode(v, false))).collect();
            url.push('?');
            url.push_str(&q.join("&"));
        }

        macro_rules! with_headers {
            ($b:expr) => {{
                let mut b = $b;
                for (k, v) in &headers {
                    if k != "host" {
                        b = b.header(k.as_str(), v.as_str());
                    }
                }
                b.header("authorization", signed.authorization.as_str())
            }};
        }
        let mut resp = match req.method {
            "GET" => with_headers!(self.agent.get(&url)).call()?,
            "HEAD" => with_headers!(self.agent.head(&url)).call()?,
            "DELETE" => with_headers!(self.agent.delete(&url)).call()?,
            "PUT" => with_headers!(self.agent.put(&url)).send(req.body)?,
            other => return Err(ureq::Error::BadUri(format!("unsupported method {other}"))),
        };
        let status = resp.status().as_u16();
        let out_headers = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_ascii_lowercase(), v.to_string())))
            .collect();
        let body = if req.method == "HEAD" {
            Vec::new()
        } else {
            resp.body_mut().with_config().limit(self.body_limit).read_to_vec()?
        };
        Ok(Response { status, headers: out_headers, body })
    }

    /// Sends a request, retrying transient failures with exponential backoff.
    /// Succeeds only for a 2xx status or one listed in `accept`; every other
    /// answer is an error, never a silent success.
    pub fn send(&self, what: &str, req: &Request<'_>, accept: &[u16]) -> Result<Response, StoreError> {
        let target = match req.key {
            Some(k) => format!("s3://{}/{k}", self.bucket),
            None => format!("s3://{}", self.bucket),
        };
        let mut last: Option<StoreError> = None;
        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                std::thread::sleep(backoff(attempt));
            }
            match self.once(req) {
                Ok(r) if (200..300).contains(&r.status) || accept.contains(&r.status) => {
                    // CopyObject can answer 200 with an error document in the body.
                    if r.status == 200 && r.body.starts_with(b"<?xml") && r.text().contains("<Error>") {
                        let e = describe(what, &target, &r);
                        if is_transient_code(xml::first_text(&r.text(), "Code").as_deref()) {
                            last = Some(e);
                            continue;
                        }
                        return Err(e);
                    }
                    return Ok(r);
                }
                Ok(r) if is_transient_status(r.status) => last = Some(describe(what, &target, &r)),
                Ok(r) => return Err(describe(what, &target, &r)),
                Err(e) if is_transient_error(&e) => {
                    last = Some(StoreError::Remote { status: None, message: format!("{what} {target}: {e}") });
                }
                Err(e) => {
                    return Err(StoreError::Remote { status: None, message: format!("{what} {target}: {e}") });
                }
            }
        }
        let e = last.expect("at least one attempt ran");
        Err(match e {
            StoreError::Remote { status, message } => StoreError::Remote {
                status,
                message: format!("{message} (gave up after {} attempt(s))", self.max_retries + 1),
            },
            other => other,
        })
    }
}

fn is_transient_status(status: u16) -> bool {
    matches!(status, 408 | 409 | 429 | 500 | 502 | 503 | 504)
}

fn is_transient_code(code: Option<&str>) -> bool {
    matches!(code, Some("InternalError" | "ServiceUnavailable" | "SlowDown" | "RequestTimeout" | "OperationAborted"))
}

fn is_transient_error(e: &ureq::Error) -> bool {
    matches!(
        e,
        ureq::Error::Io(_)
            | ureq::Error::Timeout(_)
            | ureq::Error::HostNotFound
            | ureq::Error::ConnectionFailed
            | ureq::Error::Protocol(_)
    )
}

/// 50 ms, 100 ms, 200 ms … capped at 3 s, plus up to 50 % jitter (see the `BACKOFF_*` constants). Without
/// randomness the jitter is skipped: the retry then waits the plain delay.
fn backoff(attempt: u32) -> Duration {
    let base = BACKOFF_BASE_MS.saturating_mul(1 << attempt.min(BACKOFF_MAX_DOUBLINGS)).min(BACKOFF_CAP_MS);
    let jitter = ag_crypto::random_bytes::<2>().map_or(0, |b| u64::from(u16::from_le_bytes(b)) % (base / 2 + 1));
    Duration::from_millis(base + jitter)
}

/// An error for an unexpected response, with the service's own code and message.
fn describe(what: &str, target: &str, r: &Response) -> StoreError {
    let text = r.text();
    let code = xml::first_text(&text, "Code");
    let msg = xml::first_text(&text, "Message");
    let mut message = format!("{what} {target}: HTTP {}", r.status);
    if let Some(c) = &code {
        message.push_str(&format!(" {c}"));
    }
    if let Some(m) = msg {
        message.push_str(&format!(": {m}"));
    }
    let hint = match (r.status, code.as_deref()) {
        (_, Some("SignatureDoesNotMatch" | "InvalidAccessKeyId")) | (401, _) => {
            Some("check the access key id and secret access key")
        }
        (_, Some("RequestTimeTooSkewed")) => Some("the clock of this machine is too far from the server's"),
        (_, Some("NoSuchBucket")) => Some("the bucket must exist: it is not created automatically"),
        (301 | 307, _) | (_, Some("PermanentRedirect" | "AuthorizationHeaderMalformed")) => {
            Some("the region or endpoint probably does not match the bucket")
        }
        (403, _) => Some("the credentials may lack permission on this bucket or prefix"),
        _ => None,
    };
    if let Some(h) = hint {
        message.push_str(&format!(" ({h})"));
    }
    StoreError::Remote { status: Some(r.status), message }
}
