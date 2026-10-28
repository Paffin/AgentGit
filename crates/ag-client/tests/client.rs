//! The client against a real `ag-server` (objects, batches, chunks) and against a scripted server that does
//! what no real one does: fails on demand, lies about a stream, goes quiet. The scripted server is a test
//! double for exactly the behaviours a correct server never shows; everything else runs on the real one.
#![allow(missing_docs)]

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ag_client::{Client, ClientError, Options};
use ag_object::RawObject;
use ag_protocol::v1;
use ag_protocol::v1::object_service_server::{ObjectService, ObjectServiceServer};
use ag_protocol::v1::protocol_service_server::{ProtocolService, ProtocolServiceServer};
use ag_protocol::v1::tree_service_server::{TreeService, TreeServiceServer};
use ag_protocol::{ErrorCode, ProtocolError};
use ag_server::app::App;
use ag_server::auth::mint_token;
use ag_server::authz::Role;
use ag_server::config::ServerConfig;
use ag_server::control::CredentialRow;
use ag_storage::SystemClock;
use ag_types::ObjectId;
use futures_util::Stream;
use tonic::{Request, Response, Status, Streaming};

const REPO: &str = "acme/demo";

// ---- the real server --------------------------------------------------------------------------------------

struct RealServer {
    url: String,
    app: App,
    _dir: tempfile::TempDir,
    _thread: std::thread::JoinHandle<()>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl RealServer {
    fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = ServerConfig::default();
        cfg.server.data_dir = dir.path().to_path_buf();
        cfg.limits.requests_per_second = 0;
        let app = App::with_options(cfg, Arc::new(SystemClock), None).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
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
        app.repos
            .create(REPO, "main", "", &ag_types::Identity::new("system:test").unwrap(), Default::default())
            .unwrap();
        Self { url: format!("http://{addr}"), app, _dir: dir, _thread: thread, stop: Some(stop_tx) }
    }

    fn token(&self, subject: &str, role: Role) -> String {
        self.app.control.grant_role(subject, REPO, role, "system:test").unwrap();
        self.app.invalidate_bindings();
        let m = mint_token().unwrap();
        let now = self.app.clock.now().as_millis();
        self.app
            .control
            .insert_credential(&CredentialRow {
                id: m.credential_id,
                secret_hash: m.secret_hash,
                subject: subject.into(),
                kind: "user".into(),
                description: String::new(),
                scope: Default::default(),
                created_at_ms: now,
                expires_at_ms: now + 3_600_000,
                revoked_at_ms: None,
                last_used_ms: 0,
                created_by: "system:test".into(),
            })
            .unwrap();
        m.token
    }

    async fn client(&self, edit: impl FnOnce(&mut Options)) -> Client {
        let mut opts = Options {
            endpoint: self.url.clone(),
            token: self.token("user:alice", Role::Writer),
            timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        };
        edit(&mut opts);
        Client::connect(opts).await.unwrap()
    }
}

impl Drop for RealServer {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

/// A blob of `n` bytes that differs from every other blob made with another `seed`.
fn blob(seed: u32, n: usize) -> RawObject {
    let mut bytes = Vec::with_capacity(n);
    let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    while bytes.len() < n {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        bytes.extend_from_slice(&x.to_be_bytes());
    }
    bytes.truncate(n);
    RawObject::blob(bytes)
}

#[tokio::test(flavor = "multi_thread")]
async fn objects_travel_in_chunks_and_batches_and_arrive_whole() {
    let srv = RealServer::start();
    let chunk = ag_protocol::limits::CHUNK_BYTES;
    // Two objects of more than two chunks each, and a crowd of small ones; a batch holds about one big object.
    let mut objects = vec![blob(1, 2 * chunk + 17), blob(2, 2 * chunk + 5)];
    objects.extend((10..40).map(|i| blob(i, 300 + i as usize)));
    let ids: Vec<ObjectId> = objects.iter().map(RawObject::id).collect();
    let client = srv.client(|o| o.upload_batch_bytes = (2 * chunk + 100) as u64).await;

    assert_eq!(client.missing_objects(REPO, &ids).await.unwrap().len(), ids.len(), "nothing there yet");
    let first = client.upload_objects(REPO, &objects).await.unwrap();
    assert_eq!(first.stored.len(), objects.len(), "{first:?}");
    assert!(first.already_present.is_empty());
    assert!(first.bytes >= (4 * chunk) as u64);
    assert!(client.missing_objects(REPO, &ids).await.unwrap().is_empty());
    // Repeating it is harmless, and the response adds the batches up the same way.
    let again = client.upload_objects(REPO, &objects).await.unwrap();
    assert!(again.stored.is_empty() && again.already_present.len() == objects.len(), "{again:?}");
    for (o, id) in objects.iter().zip(&ids) {
        assert_eq!(&client.get_object(REPO, id).await.unwrap(), o);
    }
    // Nothing to send is not a call.
    assert_eq!(client.upload_objects(REPO, &[]).await.unwrap(), v1::UploadObjectsResponse::default());
}

#[tokio::test(flavor = "multi_thread")]
async fn asking_what_is_missing_works_in_pieces_of_a_request() {
    let srv = RealServer::start();
    let client = srv.client(|_| {}).await;
    // More ids than one request may carry (the server refuses more than MAX_BATCH in one).
    let n = ag_protocol::limits::MAX_BATCH as usize + 500;
    let ids: Vec<ObjectId> = (0..n as u32).map(|i| blob(100_000 + i, 8).id()).collect();
    let missing = client.missing_objects(REPO, &ids).await.unwrap();
    assert_eq!(missing.len(), n);
    assert_eq!(missing, ids, "in the order they were asked about");
    // One of them present: it is the only one missing from the answer.
    client.upload_objects(REPO, &[blob(100_000 + 7_000, 8)]).await.unwrap();
    let missing = client.missing_objects(REPO, &ids).await.unwrap();
    assert_eq!(missing.len(), n - 1);
    assert!(!missing.contains(&ids[7_000]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_id_that_cannot_be_sent_is_an_error_and_never_dropped() {
    let srv = RealServer::start();
    // At connect ...
    let r = Client::connect(Options {
        endpoint: srv.url.clone(),
        token: srv.token("user:alice", Role::Writer),
        run: Some("run\nwith a newline".into()),
        ..Default::default()
    })
    .await;
    assert!(matches!(r, Err(ClientError::Invalid(ref m)) if m.contains("run id")), "{:?}", r.err());
    // ... and when a connection is lent to another run: the call must not go out as the bare credential.
    let client = srv.client(|_| {}).await;
    assert!(matches!(client.with_run(Some("bad\u{7f}run")), Err(ClientError::Invalid(_))));
    assert!(client.with_run(Some("run_good")).is_ok());
    assert!(client.with_run(None).is_ok());
}

// ---- the scripted server ----------------------------------------------------------------------------------

type BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send + 'static>>;

/// What the scripted server does. Everything else a real server does is not here.
#[derive(Default)]
struct Script {
    /// `whoami` fails this many times before it answers.
    whoami_failures: usize,
    /// ... with this error.
    whoami_error: Option<ProtocolError>,
    /// ... and when it answers it first waits this long.
    whoami_delay: Duration,
    /// The chunks `objects.get` sends.
    object_chunks: Vec<v1::ObjectChunk>,
    /// The chunks `tree.get_file` sends.
    file_chunks: Vec<v1::ObjectChunk>,
    /// After its chunks, the stream stays open without sending anything.
    stall_after_chunks: bool,
}

#[derive(Clone, Default)]
struct Fake {
    script: Arc<Mutex<Script>>,
    whoami_calls: Arc<AtomicUsize>,
    /// For each `upload` call, the object ids it carried.
    uploads: Arc<Mutex<Vec<Vec<String>>>>,
}

fn unimplemented<T>() -> Result<Response<T>, Status> {
    Err(Status::unimplemented("the scripted server does not do this"))
}

fn chunk_stream(chunks: Vec<v1::ObjectChunk>, stall: bool) -> BoxStream<v1::ObjectChunk> {
    let items = futures_util::stream::iter(chunks.into_iter().map(Ok));
    if stall {
        Box::pin(futures_util::StreamExt::chain(items, futures_util::stream::pending()))
    } else {
        Box::pin(items)
    }
}

#[tonic::async_trait]
impl ProtocolService for Fake {
    async fn negotiate(&self, _: Request<v1::NegotiateRequest>) -> Result<Response<v1::NegotiateResponse>, Status> {
        unimplemented()
    }
    async fn whoami(&self, _: Request<v1::WhoamiRequest>) -> Result<Response<v1::Principal>, Status> {
        let n = self.whoami_calls.fetch_add(1, Ordering::SeqCst) + 1;
        let (fails, err, delay) = {
            let s = self.script.lock().unwrap();
            (s.whoami_failures, s.whoami_error.clone(), s.whoami_delay)
        };
        if n <= fails {
            return Err(err.expect("an error to fail with").to_status());
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        Ok(Response::new(v1::Principal { identity: "user:scripted".into(), ..Default::default() }))
    }
}

#[tonic::async_trait]
impl ObjectService for Fake {
    async fn upload(
        &self,
        request: Request<Streaming<v1::UploadObjectsRequest>>,
    ) -> Result<Response<v1::UploadObjectsResponse>, Status> {
        let mut stream = request.into_inner();
        let mut ids: Vec<String> = Vec::new();
        while let Some(msg) = stream.message().await? {
            if let Some(c) = msg.chunk
                && ids.last() != Some(&c.object_id)
            {
                ids.push(c.object_id);
            }
        }
        self.uploads.lock().unwrap().push(ids.clone());
        Ok(Response::new(v1::UploadObjectsResponse { stored: ids, ..Default::default() }))
    }
    type GetStream = BoxStream<v1::ObjectChunk>;
    async fn get(&self, _: Request<v1::GetObjectRequest>) -> Result<Response<Self::GetStream>, Status> {
        let s = self.script.lock().unwrap();
        Ok(Response::new(chunk_stream(s.object_chunks.clone(), s.stall_after_chunks)))
    }
    async fn missing(
        &self,
        _: Request<v1::MissingObjectsRequest>,
    ) -> Result<Response<v1::MissingObjectsResponse>, Status> {
        unimplemented()
    }
    async fn verify(
        &self,
        _: Request<v1::VerifyObjectsRequest>,
    ) -> Result<Response<v1::VerifyObjectsResponse>, Status> {
        unimplemented()
    }
    async fn publish(&self, _: Request<v1::PublishRequest>) -> Result<Response<v1::Publication>, Status> {
        unimplemented()
    }
    async fn get_operation(&self, _: Request<v1::GetOperationRequest>) -> Result<Response<v1::Operation>, Status> {
        unimplemented()
    }
}

#[tonic::async_trait]
impl TreeService for Fake {
    async fn search(&self, _: Request<v1::SearchTreeRequest>) -> Result<Response<v1::SearchTreeResponse>, Status> {
        Err(Status::unimplemented("search is outside this transport fixture"))
    }

    async fn resolve_path(
        &self,
        _: Request<v1::ResolvePathRequest>,
    ) -> Result<Response<v1::ResolvePathResponse>, Status> {
        unimplemented()
    }
    async fn list_directory(
        &self,
        _: Request<v1::ListDirectoryRequest>,
    ) -> Result<Response<v1::ListDirectoryResponse>, Status> {
        unimplemented()
    }
    async fn get_tree(&self, _: Request<v1::GetTreeRequest>) -> Result<Response<v1::GetTreeResponse>, Status> {
        unimplemented()
    }
    async fn diff(&self, _: Request<v1::DiffRequest>) -> Result<Response<v1::DiffResponse>, Status> {
        unimplemented()
    }
    type GetFileStream = BoxStream<v1::ObjectChunk>;
    async fn get_file(&self, _: Request<v1::GetFileRequest>) -> Result<Response<Self::GetFileStream>, Status> {
        let s = self.script.lock().unwrap();
        Ok(Response::new(chunk_stream(s.file_chunks.clone(), s.stall_after_chunks)))
    }
}

/// A scripted server and a client of it.
async fn scripted(script: Script, timeout: Option<Duration>, batch: u64) -> (Fake, Client) {
    let fake = Fake { script: Arc::new(Mutex::new(script)), ..Default::default() };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let svc = fake.clone();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(ProtocolServiceServer::new(svc.clone()))
            .add_service(ObjectServiceServer::new(svc.clone()))
            .add_service(TreeServiceServer::new(svc))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    let client = Client::connect(Options {
        endpoint: format!("http://{addr}"),
        token: "agt_scripted_token".into(),
        timeout,
        upload_batch_bytes: batch,
        ..Default::default()
    })
    .await
    .unwrap();
    (fake, client)
}

async fn whoami(client: &Client) -> Result<v1::Principal, ClientError> {
    client
        .retrying(|| async { client.protocol().whoami(client.req(v1::WhoamiRequest {})).await.map(|r| r.into_inner()) })
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_retryable_error_is_retried_after_the_delay_the_server_names() {
    let script = Script {
        whoami_failures: 2,
        whoami_error: Some(ProtocolError::new(ErrorCode::Unavailable, "busy").with_retry_after(250)),
        ..Default::default()
    };
    let (fake, client) = scripted(script, Some(Duration::from_secs(10)), 0).await;
    let started = Instant::now();
    let p = whoami(&client).await.unwrap();
    assert_eq!(p.identity, "user:scripted");
    assert_eq!(fake.whoami_calls.load(Ordering::SeqCst), 3, "two failures, then the answer");
    assert!(started.elapsed() >= Duration::from_millis(500), "waited the 250 ms the server asked for, twice");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_error_that_retrying_cannot_fix_is_returned_at_once() {
    let script = Script {
        whoami_failures: 99,
        whoami_error: Some(ProtocolError::new(ErrorCode::PermissionDenied, "no")),
        ..Default::default()
    };
    let (fake, client) = scripted(script, Some(Duration::from_secs(10)), 0).await;
    let err = whoami(&client).await.unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::PermissionDenied));
    assert_eq!(fake.whoami_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn retrying_gives_up_after_a_fixed_number_of_attempts() {
    let script = Script {
        whoami_failures: 99,
        whoami_error: Some(ProtocolError::new(ErrorCode::Unavailable, "down").with_retry_after(1)),
        ..Default::default()
    };
    let (fake, client) = scripted(script, Some(Duration::from_secs(10)), 0).await;
    let err = whoami(&client).await.unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::Unavailable));
    assert_eq!(fake.whoami_calls.load(Ordering::SeqCst), 6, "one call and five retries");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_that_the_server_never_answers_ends_at_the_deadline() {
    let script = Script { whoami_delay: Duration::from_secs(30), ..Default::default() };
    let (_fake, client) = scripted(script, Some(Duration::from_millis(300)), 0).await;
    let started = Instant::now();
    let r = tokio::time::timeout(Duration::from_secs(10), async {
        client.protocol().whoami(client.req(v1::WhoamiRequest {})).await.map_err(ClientError::from)
    })
    .await
    .expect("the call returned instead of waiting for the server");
    let err = r.unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::DeadlineExceeded), "{err}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

fn envelope_chunks(raw: &RawObject, split: usize) -> Vec<v1::ObjectChunk> {
    let bytes = raw.encode_envelope();
    let id = raw.id().to_string();
    let parts: Vec<&[u8]> = bytes.chunks(split).collect();
    parts
        .iter()
        .enumerate()
        .map(|(i, p)| v1::ObjectChunk {
            object_id: id.clone(),
            total_size: if i == 0 { bytes.len() as u64 } else { 0 },
            data: p.to_vec(),
            last: i + 1 == parts.len(),
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_downloaded_object_is_whole_and_is_the_one_asked_for() {
    let wanted = blob(1, 1000);
    let other = blob(2, 1000);
    // A faithful server: the object arrives, in as many chunks as it was sent in.
    let (_f, client) = scripted(
        Script { object_chunks: envelope_chunks(&wanted, 300), ..Default::default() },
        Some(Duration::from_secs(5)),
        0,
    )
    .await;
    assert_eq!(client.get_object(REPO, &wanted.id()).await.unwrap(), wanted);
    // Another object's bytes under this one's id are refused (the hash is the check)
    let err = client.get_object(REPO, &other.id()).await.unwrap_err();
    assert!(matches!(err, ClientError::Invalid(ref m) if m.contains("do not hash")), "{err}");
    // A stream that ends before its last chunk is a broken transfer, not a shorter object
    let mut chunks = envelope_chunks(&wanted, 300);
    chunks.pop();
    chunks.last_mut().unwrap().last = false;
    let (_f, client) =
        scripted(Script { object_chunks: chunks, ..Default::default() }, Some(Duration::from_secs(5)), 0).await;
    let err = client.get_object(REPO, &wanted.id()).await.unwrap_err();
    assert!(matches!(err, ClientError::Transport(ref m) if m.contains("before its last chunk")), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_that_stops_early_or_is_not_the_announced_size_is_an_error() {
    let content = vec![b'x'; 900];
    let chunk = |i: usize, total: u64, last: bool| v1::ObjectChunk {
        object_id: "f".into(),
        total_size: total,
        data: content[i * 300..(i + 1) * 300].to_vec(),
        last,
    };
    // faithful
    let (_f, client) = scripted(
        Script { file_chunks: vec![chunk(0, 900, false), chunk(1, 0, false), chunk(2, 0, true)], ..Default::default() },
        Some(Duration::from_secs(5)),
        0,
    )
    .await;
    assert_eq!(client.get_file(REPO, "main", "f").await.unwrap(), content);
    // cut off: no chunk says it is the last
    let (_f, client) = scripted(
        Script { file_chunks: vec![chunk(0, 900, false), chunk(1, 0, false)], ..Default::default() },
        Some(Duration::from_secs(5)),
        0,
    )
    .await;
    let err = client.get_file(REPO, "main", "f").await.unwrap_err();
    assert!(matches!(err, ClientError::Transport(ref m) if m.contains("before its last chunk")), "{err}");
    // finished, but not with the size it announced
    let (_f, client) = scripted(
        Script { file_chunks: vec![chunk(0, 900, false), chunk(1, 0, true)], ..Default::default() },
        Some(Duration::from_secs(5)),
        0,
    )
    .await;
    let err = client.get_file(REPO, "main", "f").await.unwrap_err();
    assert!(matches!(err, ClientError::Transport(ref m) if m.contains("announced 900 bytes")), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_that_goes_quiet_ends_at_the_deadline_instead_of_hanging() {
    let wanted = blob(3, 1000);
    let mut chunks = envelope_chunks(&wanted, 300);
    chunks.truncate(2);
    chunks.last_mut().unwrap().last = false;
    let (_f, client) = scripted(
        Script { object_chunks: chunks.clone(), file_chunks: chunks, stall_after_chunks: true, ..Default::default() },
        Some(Duration::from_millis(400)),
        0,
    )
    .await;
    for what in ["object", "file"] {
        let started = Instant::now();
        let r = tokio::time::timeout(Duration::from_secs(10), async {
            if what == "object" {
                client.get_object(REPO, &wanted.id()).await.map(|_| ())
            } else {
                client.get_file(REPO, "main", "f").await.map(|_| ())
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the {what} download hangs on a server that went quiet"));
        let err = r.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::DeadlineExceeded), "{what}: {err}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn uploads_are_split_into_calls_of_a_bounded_size() {
    let objects: Vec<RawObject> = (0..10).map(|i| blob(i, 1000)).collect();
    let one = objects[0].encode_envelope().len() as u64;
    // room for three envelopes per call
    let (fake, client) = scripted(Script::default(), Some(Duration::from_secs(5)), one * 3 + 10).await;
    let r = client.upload_objects(REPO, &objects).await.unwrap();
    assert_eq!(r.stored.len(), objects.len());
    let calls = fake.uploads.lock().unwrap().clone();
    assert_eq!(calls.iter().map(Vec::len).collect::<Vec<_>>(), [3, 3, 3, 1], "{calls:?}");
    let sent: Vec<String> = calls.concat();
    assert_eq!(sent, objects.iter().map(|o| o.id().to_string()).collect::<Vec<_>>(), "in order, none twice");
    // an object larger than the bound goes alone
    let (fake, client) = scripted(Script::default(), Some(Duration::from_secs(5)), 100).await;
    client.upload_objects(REPO, &objects[..3]).await.unwrap();
    assert_eq!(fake.uploads.lock().unwrap().iter().map(Vec::len).collect::<Vec<_>>(), [1, 1, 1]);
}
