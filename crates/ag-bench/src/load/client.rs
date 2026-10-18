//! The two real clients the load generator drives the server with: HTTP/JSON (`ureq`) and gRPC (`tonic`).
//!
//! Both are blocking from the caller's point of view (a worker is a thread), so one worker loop serves both
//! protocols and the protocols differ only here.

use std::time::Duration;

use ag_changeset::{Changeset, Op};
use ag_object::RawObject;
use ag_protocol::v1;
use ag_protocol::v1::{
    event_service_client::EventServiceClient, integration_service_client::IntegrationServiceClient,
    object_service_client::ObjectServiceClient, proposal_service_client::ProposalServiceClient,
    repository_service_client::RepositoryServiceClient, stream_service_client::StreamServiceClient,
};
use ag_types::{BlobId, ChangesetId, FileType, Identity, Limits, Namespace, RepoPath, Timestamp};
use serde_json::{Value, json};
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;

/// Which wire protocol a session speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, clap::ValueEnum, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Proto {
    /// HTTP/JSON (HTTP/1.1, keep-alive pool).
    Http,
    /// gRPC (one HTTP/2 connection per session, multiplexed).
    Grpc,
}

/// A failed call, reduced to what a load report needs.
#[derive(Clone, Debug)]
pub struct CallError {
    /// The AGWP error code (`rate_limited`, `unavailable`…), or `transport` when no response arrived.
    pub code: String,
    /// The server's retry hint, if it gave one.
    pub retry_after_ms: i64,
    /// HTTP status (0 for gRPC and transport errors).
    pub http_status: u16,
    /// Value of the `Retry-After` header, if any.
    pub retry_after_header: Option<String>,
    /// Human-readable detail for the first few failures of a kind.
    pub message: String,
}

impl CallError {
    fn transport(e: impl std::fmt::Display) -> Self {
        Self {
            code: "transport".into(),
            retry_after_ms: 0,
            http_status: 0,
            retry_after_header: None,
            message: e.to_string(),
        }
    }
}

type CallResult<T> = Result<T, CallError>;

/// What integrating a proposal answered.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// `integrated`, `blocked`, `conflict`…
    pub status: String,
    /// `fast_forward`, `merge`…
    pub strategy: String,
    /// The head the stream moved to (empty unless integrated).
    pub new_head: String,
}

/// An HTTP response.
pub struct Reply {
    /// Status line code.
    pub status: u16,
    /// Parsed body (`Null` if it was not JSON).
    pub json: Value,
    /// `Retry-After`, if present.
    pub retry_after: Option<String>,
}

/// A caller with a credential, connected to one server.
#[derive(Clone)]
pub struct Session {
    /// The protocol.
    pub proto: Proto,
    base: String,
    token: String,
    agent: ureq::Agent,
    grpc: Option<(tokio::runtime::Handle, Channel)>,
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .max_idle_connections_per_host(256)
        .build()
        .into()
}

impl Session {
    /// A session for `token`. A gRPC session connects to `base` (`http://host:port`) now.
    pub fn connect(proto: Proto, base: &str, token: &str, rt: &tokio::runtime::Handle) -> anyhow::Result<Self> {
        let grpc = match proto {
            Proto::Http => None,
            Proto::Grpc => {
                let base = base.to_string();
                let ch = rt.block_on(async move { anyhow::Ok(Channel::from_shared(base)?.connect().await?) })?;
                Some((rt.clone(), ch))
            }
        };
        Ok(Self {
            proto,
            base: base.to_string(),
            token: token.to_string(),
            agent: agent(Duration::from_secs(60)),
            grpc,
        })
    }

    /// The same credential over the other protocol, for setup steps that are protocol-independent.
    pub fn http_only(base: &str, token: &str) -> Self {
        Self {
            proto: Proto::Http,
            base: base.to_string(),
            token: token.to_string(),
            agent: agent(Duration::from_secs(60)),
            grpc: None,
        }
    }

    /// One HTTP request. Transport failures are `Err`; any HTTP status is `Ok`.
    pub fn http(&self, method: &str, path: &str, body: Option<Value>) -> Result<Reply, CallError> {
        let mut b = ureq::http::Request::builder().method(method).uri(format!("{}{path}", self.base));
        if !self.token.is_empty() {
            b = b.header("authorization", format!("Bearer {}", self.token));
        }
        let resp = match body {
            Some(v) => self
                .agent
                .run(b.header("content-type", "application/json").body(v.to_string()).map_err(CallError::transport)?),
            None => self.agent.run(b.body(()).map_err(CallError::transport)?),
        }
        .map_err(CallError::transport)?;
        let status = resp.status().as_u16();
        let retry_after = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
        let mut resp = resp;
        let bytes = resp.body_mut().read_to_vec().map_err(CallError::transport)?;
        Ok(Reply { status, json: serde_json::from_slice(&bytes).unwrap_or(Value::Null), retry_after })
    }

    fn http_ok(&self, method: &str, path: &str, body: Option<Value>) -> CallResult<Value> {
        let r = self.http(method, path, body)?;
        if r.status == 200 {
            return Ok(r.json);
        }
        let e = &r.json["error"];
        Err(CallError {
            code: e["code"].as_str().unwrap_or("http_error").to_string(),
            retry_after_ms: e["retry_after_ms"].as_i64().unwrap_or(0),
            http_status: r.status,
            retry_after_header: r.retry_after,
            message: e["message"].as_str().unwrap_or("").to_string(),
        })
    }

    fn put_object(&self, repo: &str, o: &RawObject) -> CallResult<()> {
        let url = format!("{}/v1/repos/{repo}/objects/{}", self.base, o.id());
        let resp = self
            .agent
            .run(
                ureq::http::Request::builder()
                    .method("PUT")
                    .uri(url)
                    .header("authorization", format!("Bearer {}", self.token))
                    .header("content-type", "application/octet-stream")
                    .body(o.encode_envelope())
                    .map_err(CallError::transport)?,
            )
            .map_err(CallError::transport)?;
        let status = resp.status().as_u16();
        if status == 200 {
            return Ok(());
        }
        let retry_after = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
        let mut resp = resp;
        let json: Value =
            serde_json::from_slice(&resp.body_mut().read_to_vec().unwrap_or_default()).unwrap_or(Value::Null);
        Err(CallError {
            code: json["error"]["code"].as_str().unwrap_or("http_error").to_string(),
            retry_after_ms: json["error"]["retry_after_ms"].as_i64().unwrap_or(0),
            http_status: status,
            retry_after_header: retry_after,
            message: json["error"]["message"].as_str().unwrap_or("").to_string(),
        })
    }

    fn authed<T>(&self, msg: T) -> tonic::Request<T> {
        let mut r = tonic::Request::new(msg);
        if let Ok(v) = MetadataValue::try_from(format!("Bearer {}", self.token)) {
            r.metadata_mut().insert("authorization", v);
        }
        r
    }

    fn grpc(&self) -> (&tokio::runtime::Handle, &Channel) {
        let (h, c) = self.grpc.as_ref().expect("a gRPC session");
        (h, c)
    }

    /// Creates a stream (HTTP/JSON or gRPC).
    pub fn create_stream(&self, repo: &str, name: &str) -> CallResult<()> {
        match self.proto {
            Proto::Http => {
                self.http_ok("POST", &format!("/v1/repos/{repo}/streams"), Some(json!({"name": name}))).map(|_| ())
            }
            Proto::Grpc => {
                let (rt, ch) = self.grpc();
                let req =
                    self.authed(v1::CreateStreamRequest { repo: repo.into(), name: name.into(), ..Default::default() });
                rt.block_on(StreamServiceClient::new(ch.clone()).create(req)).map(|_| ()).map_err(from_status)
            }
        }
    }

    /// The current head (empty string for an empty stream) and version of a stream.
    pub fn head(&self, repo: &str, stream: &str) -> CallResult<(String, u64)> {
        match self.proto {
            Proto::Http => {
                let v = self.http_ok("GET", &format!("/v1/repos/{repo}/streams/head?stream={stream}"), None)?;
                Ok((v["head"].as_str().unwrap_or("").to_string(), v["version"].as_u64().unwrap_or(0)))
            }
            Proto::Grpc => {
                let (rt, ch) = self.grpc();
                let req =
                    self.authed(v1::ReadHeadRequest { repo: repo.into(), stream: stream.into(), ..Default::default() });
                let s =
                    rt.block_on(StreamServiceClient::new(ch.clone()).read_head(req)).map_err(from_status)?.into_inner();
                Ok((s.head, s.version))
            }
        }
    }

    /// Uploads objects: one request per object over HTTP/JSON, one client stream over gRPC (the protocol's own shapes).
    pub fn upload(&self, repo: &str, objects: &[RawObject]) -> CallResult<()> {
        match self.proto {
            Proto::Http => objects.iter().try_for_each(|o| self.put_object(repo, o)),
            Proto::Grpc => {
                let (rt, ch) = self.grpc();
                let msgs: Vec<v1::UploadObjectsRequest> = objects
                    .iter()
                    .enumerate()
                    .map(|(i, o)| {
                        let bytes = o.encode_envelope();
                        v1::UploadObjectsRequest {
                            repo: if i == 0 { repo.into() } else { String::new() },
                            chunk: Some(v1::ObjectChunk {
                                object_id: o.id().to_string(),
                                total_size: bytes.len() as u64,
                                data: bytes,
                                last: true,
                            }),
                        }
                    })
                    .collect();
                let req = self.authed(futures_util::stream::iter(msgs));
                rt.block_on(ObjectServiceClient::new(ch.clone()).upload(req)).map(|_| ()).map_err(from_status)
            }
        }
    }

    /// Opens a proposal; returns its id.
    pub fn propose(&self, repo: &str, stream: &str, head: &str, title: &str, key: &str) -> CallResult<String> {
        match self.proto {
            Proto::Http => {
                let v = self.http_ok(
                    "POST",
                    &format!("/v1/repos/{repo}/proposals"),
                    Some(json!({"stream": stream, "head": head, "title": title, "idempotency_key": key})),
                )?;
                Ok(v["id"].as_str().unwrap_or("").to_string())
            }
            Proto::Grpc => {
                let (rt, ch) = self.grpc();
                let req = self.authed(v1::CreateProposalRequest {
                    repo: repo.into(),
                    stream: stream.into(),
                    head: head.into(),
                    title: title.into(),
                    idempotency_key: key.into(),
                    ..Default::default()
                });
                Ok(rt
                    .block_on(ProposalServiceClient::new(ch.clone()).create(req))
                    .map_err(from_status)?
                    .into_inner()
                    .id)
            }
        }
    }

    /// Integrates a proposal.
    pub fn integrate(&self, repo: &str, proposal: &str, key: &str) -> CallResult<Outcome> {
        match self.proto {
            Proto::Http => {
                let v = self.http_ok(
                    "POST",
                    &format!("/v1/repos/{repo}/proposals/{proposal}/integrate"),
                    Some(json!({"idempotency_key": key})),
                )?;
                let o = &v["outcome"];
                Ok(Outcome {
                    status: o["status"].as_str().unwrap_or("").into(),
                    strategy: o["strategy"].as_str().unwrap_or("").into(),
                    new_head: o["new_head"].as_str().unwrap_or("").into(),
                })
            }
            Proto::Grpc => {
                let (rt, ch) = self.grpc();
                let req = self.authed(v1::IntegrateRequest {
                    repo: repo.into(),
                    proposal: proposal.into(),
                    idempotency_key: key.into(),
                    ..Default::default()
                });
                let r = rt
                    .block_on(IntegrationServiceClient::new(ch.clone()).integrate(req))
                    .map_err(from_status)?
                    .into_inner();
                let o = r.outcome.unwrap_or_default();
                Ok(Outcome { status: o.status, strategy: o.strategy, new_head: o.new_head })
            }
        }
    }

    /// A cheap authenticated read (`whoami` over HTTP, the repository listing over gRPC).
    pub fn ping(&self) -> CallResult<()> {
        match self.proto {
            Proto::Http => self.http_ok("GET", "/v1/whoami", None).map(|_| ()),
            Proto::Grpc => {
                let (rt, ch) = self.grpc();
                let req = self.authed(v1::ListReposRequest::default());
                rt.block_on(RepositoryServiceClient::new(ch.clone()).list(req)).map(|_| ()).map_err(from_status)
            }
        }
    }

    /// Follows the repository's event feed from now on, calling `on_event(seq, topic, timestamp_ms, payload)` for each
    /// event until `stop` returns true (checked between events and at least every 200 ms) or the feed ends.
    /// `ready` is called once the subscription is established. Returns an error string if the feed failed.
    pub fn follow(
        &self,
        repo: &str,
        ready: &dyn Fn(),
        stop: &dyn Fn() -> bool,
        on_event: &mut dyn FnMut(u64, &str, i64, &str),
    ) -> Result<(), String> {
        match self.proto {
            Proto::Http => {
                use std::io::{BufRead, BufReader};
                // No timeout: the server's heartbeat comment (every second) wakes the loop so `stop` is seen.
                let agent: ureq::Agent =
                    ureq::Agent::config_builder().http_status_as_error(false).timeout_global(None).build().into();
                let resp = agent
                    .run(
                        ureq::http::Request::builder()
                            .method("GET")
                            .uri(format!("{}/v1/repos/{repo}/events?heartbeat_ms=1000", self.base))
                            .header("authorization", format!("Bearer {}", self.token))
                            .header("accept", "text/event-stream")
                            .body(())
                            .map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
                if resp.status().as_u16() != 200 {
                    return Err(format!("events answered {}", resp.status()));
                }
                ready();
                let mut reader = BufReader::new(resp.into_body().into_reader());
                let (mut seq, mut topic, mut data) = (0u64, String::new(), String::new());
                let mut line = String::new();
                loop {
                    if stop() {
                        return Ok(());
                    }
                    match reader.read_line(&mut line) {
                        Ok(0) => return Ok(()),
                        Ok(_) => {
                            let owned = line.trim_end().to_string();
                            line.clear();
                            let l = owned.as_str();
                            if let Some(v) = l.strip_prefix("id:") {
                                seq = v.trim().parse().unwrap_or(0);
                            } else if let Some(v) = l.strip_prefix("event:") {
                                topic = v.trim().to_string();
                            } else if let Some(v) = l.strip_prefix("data:") {
                                data = v.trim().to_string();
                            } else if l.is_empty() && !topic.is_empty() {
                                let ts = serde_json::from_str::<Value>(&data)
                                    .ok()
                                    .and_then(|v| v["timestamp_ms"].as_i64())
                                    .unwrap_or(0);
                                on_event(seq, &topic, ts, &data);
                                topic.clear();
                                data.clear();
                            }
                        }
                        Err(e) => return Err(e.to_string()),
                    }
                }
            }
            Proto::Grpc => {
                let (rt, ch) = self.grpc();
                let req = self.authed(v1::SubscribeRequest {
                    repo: repo.into(),
                    after_seq: u64::MAX,
                    heartbeat_ms: 1000,
                    ..Default::default()
                });
                rt.block_on(async {
                    let mut feed = EventServiceClient::new(ch.clone())
                        .subscribe(req)
                        .await
                        .map_err(|e| e.to_string())?
                        .into_inner();
                    ready();
                    loop {
                        if stop() {
                            return Ok(());
                        }
                        match tokio::time::timeout(Duration::from_millis(200), feed.message()).await {
                            Err(_) => {}
                            Ok(Ok(Some(ev))) => {
                                if ev.topic != "heartbeat" {
                                    on_event(ev.seq, &ev.topic, ev.timestamp_ms, &ev.payload_json);
                                }
                            }
                            Ok(Ok(None)) => return Ok(()),
                            Ok(Err(e)) => return Err(e.to_string()),
                        }
                    }
                })
            }
        }
    }
}

fn from_status(s: tonic::Status) -> CallError {
    let e = ag_protocol::ProtocolError::from_status(&s);
    CallError {
        code: e.code.as_str().to_string(),
        retry_after_ms: e.retry_after_ms,
        http_status: 0,
        retry_after_header: None,
        message: s.message().to_string(),
    }
}

/// A changeset the way a client builds it: `files` are put on top of `parents`; returns its id and every object to upload.
pub fn build_changeset(
    namespace: &Namespace,
    parents: Vec<ChangesetId>,
    author: &str,
    message: &str,
    files: &[(String, Vec<u8>)],
) -> anyhow::Result<(ChangesetId, Vec<RawObject>)> {
    let mut objects = Vec::new();
    let mut ops = Vec::new();
    for (path, content) in files {
        let blob = RawObject::blob(content.clone());
        let id = BlobId::new(blob.id());
        objects.push(blob);
        ops.push(Op::Put {
            path: RepoPath::new(path.as_str())?,
            content: id,
            file_type: FileType::Regular,
            executable: false,
        });
    }
    let cs = Changeset::build(
        namespace.clone(),
        parents,
        Identity::new(author)?,
        Timestamp::now(),
        message,
        ops,
        &Limits::default(),
    )?;
    let id = cs.id();
    objects.push(cs.to_raw());
    Ok((id, objects))
}
