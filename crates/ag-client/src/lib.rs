//! A Rust client for AGWP over gRPC.
//!
//! [`Client`] owns one HTTP/2 connection (multiplexed, so it is cheap to
//! clone and share) and attaches the credential — and, for agents, the run —
//! to every call. The generated service clients are exposed directly
//! ([`Client::proposals`], [`Client::objects`], ...); on top of them sit the
//! helpers every tool needs: object transfer in chunks, "which of these does
//! the server lack", and retry of *retryable* failures with the delay the
//! server asked for.

use std::future::Future;
use std::path::Path;
use std::time::Duration;

use ag_object::RawObject;
use ag_protocol::v1;
use ag_protocol::v1::{
    admin_service_client::AdminServiceClient, agent_service_client::AgentServiceClient,
    event_service_client::EventServiceClient, execution_service_client::ExecutionServiceClient,
    history_service_client::HistoryServiceClient, integration_service_client::IntegrationServiceClient,
    object_service_client::ObjectServiceClient, proposal_service_client::ProposalServiceClient,
    protocol_service_client::ProtocolServiceClient, repository_service_client::RepositoryServiceClient,
    stream_service_client::StreamServiceClient, tree_service_client::TreeServiceClient,
    workspace_service_client::WorkspaceServiceClient,
};
use ag_protocol::{ErrorCode, ProtocolError};
use ag_types::ObjectId;
use tonic::metadata::{AsciiMetadataValue, MetadataValue};
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};
use tonic::{Request, Status};

/// Errors from the client.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The server answered with an AGWP error.
    #[error("{0}")]
    Protocol(#[from] ProtocolError),
    /// The connection could not be established or broke.
    #[error("cannot reach the server: {0}")]
    Transport(String),
    /// The caller's input is unusable (bad URL, unreadable CA file, ...).
    #[error("{0}")]
    Invalid(String),
}

impl ClientError {
    /// The AGWP error code, if the server sent one.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            ClientError::Protocol(p) => Some(p.code),
            _ => None,
        }
    }
}

impl From<Status> for ClientError {
    fn from(s: Status) -> Self {
        ClientError::Protocol(ProtocolError::from_status(&s))
    }
}

impl From<tonic::transport::Error> for ClientError {
    fn from(e: tonic::transport::Error) -> Self {
        ClientError::Transport(e.to_string())
    }
}

/// Result alias.
pub type Result<T> = std::result::Result<T, ClientError>;

/// Connection options.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// `http://host:port` (plaintext HTTP/2) or `https://host:port`.
    pub endpoint: String,
    /// Bearer credential.
    pub token: String,
    /// Agent run to act as (`ag-run`).
    pub run: Option<String>,
    /// Extra PEM CA certificate to trust (self-signed deployments).
    pub ca_file: Option<std::path::PathBuf>,
    /// Per-call timeout; also sent as the deadline the server enforces.
    pub timeout: Option<Duration>,
    /// Client name for diagnostics.
    pub user_agent: Option<String>,
}

/// Credentials attached to every call (opaque to users of the crate).
#[derive(Clone)]
pub struct Auth {
    authorization: AsciiMetadataValue,
    run: Option<AsciiMetadataValue>,
    user_agent: AsciiMetadataValue,
}

impl Interceptor for Auth {
    fn call(&mut self, mut req: Request<()>) -> std::result::Result<Request<()>, Status> {
        let md = req.metadata_mut();
        md.insert("authorization", self.authorization.clone());
        md.insert("user-agent", self.user_agent.clone());
        if let Some(run) = &self.run {
            md.insert("ag-run", run.clone());
        }
        Ok(req)
    }
}

/// The transport type of the generated service clients.
pub type Svc = InterceptedService<Channel, Auth>;

/// An authenticated AGWP connection.
#[derive(Clone)]
pub struct Client {
    channel: Channel,
    auth: Auth,
    timeout: Option<Duration>,
}

macro_rules! accessors {
    ($($name:ident => $ty:ident),+ $(,)?) => {
        $(
            /// The generated service client (credentials attached).
            pub fn $name(&self) -> $ty<Svc> {
                $ty::with_interceptor(self.channel.clone(), self.auth.clone())
                    .max_decoding_message_size(16 * 1024 * 1024)
                    .max_encoding_message_size(16 * 1024 * 1024)
            }
        )+
    };
}

impl Client {
    /// Connects (HTTP/2; TLS when the endpoint is `https://`).
    pub async fn connect(opts: Options) -> Result<Self> {
        if opts.token.is_empty() {
            return Err(ClientError::Invalid("no credential given (set a token)".into()));
        }
        let mut endpoint = Endpoint::from_shared(opts.endpoint.clone())
            .map_err(|e| ClientError::Invalid(format!("bad server address '{}': {e}", opts.endpoint)))?
            .connect_timeout(Duration::from_secs(10))
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .http2_keep_alive_interval(Duration::from_secs(30))
            .keep_alive_while_idle(true);
        if opts.endpoint.starts_with("https://") {
            install_crypto_provider();
            let mut tls = ClientTlsConfig::new().with_webpki_roots();
            if let Some(path) = &opts.ca_file {
                let pem = std::fs::read(path)
                    .map_err(|e| ClientError::Invalid(format!("cannot read CA file {}: {e}", path.display())))?;
                tls = tls.ca_certificate(Certificate::from_pem(pem));
            }
            endpoint = endpoint.tls_config(tls)?;
        }
        let channel = endpoint.connect().await?;
        let bearer = AsciiMetadataValue::try_from(format!("Bearer {}", opts.token)).map_err(|_| {
            ClientError::Invalid("the token contains characters that cannot be sent in a header".into())
        })?;
        let run = match &opts.run {
            Some(r) => {
                Some(MetadataValue::try_from(r.as_str()).map_err(|_| ClientError::Invalid("bad run id".into()))?)
            }
            None => None,
        };
        let ua = opts.user_agent.clone().unwrap_or_else(|| format!("ag-client/{}", env!("CARGO_PKG_VERSION")));
        let user_agent = MetadataValue::try_from(ua).map_err(|_| ClientError::Invalid("bad user agent".into()))?;
        Ok(Self { channel, auth: Auth { authorization: bearer, run, user_agent }, timeout: opts.timeout })
    }

    /// The same connection acting as another agent run.
    #[must_use]
    pub fn with_run(&self, run: Option<&str>) -> Self {
        let mut c = self.clone();
        c.auth.run = run.and_then(|r| MetadataValue::try_from(r).ok());
        c
    }

    accessors! {
        protocol => ProtocolServiceClient,
        repos => RepositoryServiceClient,
        objects => ObjectServiceClient,
        history => HistoryServiceClient,
        tree => TreeServiceClient,
        streams => StreamServiceClient,
        workspaces => WorkspaceServiceClient,
        proposals => ProposalServiceClient,
        integration => IntegrationServiceClient,
        agents => AgentServiceClient,
        events => EventServiceClient,
        admin => AdminServiceClient,
        execution => ExecutionServiceClient,
    }

    /// Wraps a request message, applying the configured deadline.
    pub fn req<T>(&self, msg: T) -> Request<T> {
        let mut r = Request::new(msg);
        if let Some(t) = self.timeout {
            r.set_timeout(t);
        }
        r
    }

    /// Capability negotiation and server limits.
    pub async fn negotiate(&self) -> Result<v1::NegotiateResponse> {
        let req = v1::NegotiateRequest {
            client_name: "ag-client".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
            supported_versions: vec![ag_protocol::VERSION.into()],
            features: Vec::new(),
        };
        Ok(self.protocol().negotiate(self.req(req)).await?.into_inner())
    }

    /// Runs `op`, retrying errors the server marked retryable (honouring its
    /// `retry_after_ms`) with exponential backoff. Use only for idempotent
    /// calls or calls that carry an idempotency key.
    pub async fn retrying<T, F, Fut>(&self, mut op: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = std::result::Result<T, Status>>,
    {
        let mut delay = Duration::from_millis(100);
        for attempt in 0..6 {
            match op().await {
                Ok(v) => return Ok(v),
                Err(status) => {
                    let e = ProtocolError::from_status(&status);
                    let transient = e.code.retryable() || matches!(status.code(), tonic::Code::Unavailable);
                    if !transient || attempt == 5 {
                        return Err(ClientError::Protocol(e));
                    }
                    let wait =
                        if e.retry_after_ms > 0 { Duration::from_millis(e.retry_after_ms as u64) } else { delay };
                    tracing::debug!(code = %e.code, ?wait, attempt, "retrying");
                    tokio::time::sleep(wait.min(Duration::from_secs(30))).await;
                    delay = (delay * 2).min(Duration::from_secs(5));
                }
            }
        }
        unreachable!("the loop returns on the last attempt")
    }

    /// Which of `ids` does the repository lack?
    pub async fn missing_objects(&self, repo: &str, ids: &[ObjectId]) -> Result<Vec<ObjectId>> {
        let mut missing = Vec::new();
        for batch in ids.chunks(5000) {
            let req = v1::MissingObjectsRequest {
                repo: repo.into(),
                object_ids: batch.iter().map(|i| i.to_string()).collect(),
            };
            let resp =
                self.retrying(|| async { self.objects().missing(self.req(req.clone())).await }).await?.into_inner();
            for m in resp.missing {
                missing
                    .push(m.parse().map_err(|_| ClientError::Invalid(format!("server sent a bad object id '{m}'")))?);
            }
        }
        Ok(missing)
    }

    /// Uploads objects as one chunked client stream. The server re-hashes
    /// every object and refuses any that does not match its id.
    pub async fn upload_objects(&self, repo: &str, objects: &[RawObject]) -> Result<v1::UploadObjectsResponse> {
        if objects.is_empty() {
            return Ok(v1::UploadObjectsResponse::default());
        }
        let chunk = ag_protocol::limits::CHUNK_BYTES;
        let mut msgs: Vec<v1::UploadObjectsRequest> = Vec::new();
        for o in objects {
            let id = o.id().to_string();
            let bytes = o.encode_envelope();
            let total = bytes.len() as u64;
            let n = bytes.len().div_ceil(chunk).max(1);
            for i in 0..n {
                msgs.push(v1::UploadObjectsRequest {
                    repo: if msgs.is_empty() { repo.into() } else { String::new() },
                    chunk: Some(v1::ObjectChunk {
                        object_id: id.clone(),
                        total_size: if i == 0 { total } else { 0 },
                        data: bytes[i * chunk..((i + 1) * chunk).min(bytes.len())].to_vec(),
                        last: i + 1 == n,
                    }),
                });
            }
        }
        let resp = self.objects().upload(self.req(futures_util::stream::iter(msgs))).await?;
        Ok(resp.into_inner())
    }

    /// Downloads one object envelope (verified by the caller via [`RawObject`]).
    pub async fn get_object(&self, repo: &str, id: &ObjectId) -> Result<RawObject> {
        let req = v1::GetObjectRequest { repo: repo.into(), object_id: id.to_string() };
        let mut stream = self.objects().get(self.req(req)).await?.into_inner();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.message().await? {
            bytes.extend_from_slice(&chunk.data);
            if chunk.last {
                break;
            }
        }
        let raw = RawObject::decode_envelope(&bytes).map_err(|e| ClientError::Invalid(format!("object {id}: {e}")))?;
        raw.verify(id).map_err(|_| ClientError::Invalid(format!("the server sent bytes that do not hash to {id}")))?;
        Ok(raw)
    }

    /// Reads a file's content at a revision.
    pub async fn get_file(&self, repo: &str, rev: &str, path: &str) -> Result<Vec<u8>> {
        let req = v1::GetFileRequest { repo: repo.into(), rev: rev.into(), path: path.into() };
        let mut stream = self.tree().get_file(self.req(req)).await?.into_inner();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.message().await? {
            bytes.extend_from_slice(&chunk.data);
            if chunk.last {
                break;
            }
        }
        Ok(bytes)
    }
}

/// Reads a CA bundle path for convenience.
pub fn read_ca(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| ClientError::Invalid(format!("cannot read {}: {e}", path.display())))
}

/// Installs the process-wide TLS crypto provider (aws-lc-rs). Several TLS
/// stacks may be linked into one binary; choosing explicitly avoids a runtime
/// panic when rustls cannot decide. Safe to call repeatedly.
pub fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}
