//! The HTTP/JSON transport (and Server-Sent Events).
//!
//! JSON bodies and query strings map one-to-one onto the protocol messages
//! (`proto/agwp/v1/agwp.proto`): path placeholders, the query string and the
//! JSON body are merged into the request message, so the same documented
//! fields work everywhere. Free-form names (streams, revisions, paths) travel
//! in the query string or body, never inside the URL path, so no encoding
//! subtlety can change which resource a request addresses.

use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
use std::time::Duration;

use ag_protocol::v1;
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::app::{App, Call, Ingress};
use crate::error::{ApiError, ApiResult};
use crate::routes::RouteMeta;

/// Maximum accepted length of a client-supplied request id.
const MAX_REQUEST_ID: usize = 64;

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.0.code.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = serde_json::json!({ "error": self.0.to_info() });
        let mut resp = (status, axum::Json(body)).into_response();
        if self.0.retry_after_ms > 0 {
            let secs = (self.0.retry_after_ms as u64).div_ceil(1000).max(1);
            if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
                resp.headers_mut().insert(header::RETRY_AFTER, v);
            }
            // `Retry-After` can only say whole seconds (rounded up, so it is never too early); the exact delay
            // that `retry_after_ms` carries in the body is repeated here for clients that read headers only.
            if let Ok(v) = HeaderValue::from_str(&self.0.retry_after_ms.to_string()) {
                resp.headers_mut().insert(HeaderName::from_static("retry-after-ms"), v);
            }
        }
        if !self.0.request_id.is_empty()
            && let Ok(v) = HeaderValue::from_str(&self.0.request_id)
        {
            resp.headers_mut().insert(HeaderName::from_static("x-request-id"), v);
        }
        resp
    }
}

fn new_request_id() -> String {
    ag_crypto::random_id("req").unwrap_or_else(|_| "req_unknown".into())
}

/// The peer address of the connection a request arrived on (set by the server's connect-info service).
fn remote_addr(request: &Request) -> Option<String> {
    request.extensions().get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|c| c.0.to_string())
}

/// Builds the transport-independent ingress description from HTTP headers.
pub fn ingress_from_headers(headers: &HeaderMap, remote: Option<String>) -> Ingress {
    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim).filter(|s| !s.is_empty());
    let bearer = get("authorization").and_then(|v| {
        let (scheme, token) = v.split_once(' ')?;
        scheme.eq_ignore_ascii_case("bearer").then(|| token.trim().to_string())
    });
    let request_id = get("x-request-id")
        .filter(|id| id.len() <= MAX_REQUEST_ID && id.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)))
        .map(str::to_string)
        .unwrap_or_else(new_request_id);
    Ingress {
        transport: "http",
        bearer,
        run: get("ag-run").map(str::to_string),
        request_id,
        deadline_ms: get("request-timeout").and_then(|v| v.parse().ok()),
        traceparent: get("traceparent").map(str::to_string),
        remote,
        forwarded: get("forwarded").map(str::to_string),
        x_forwarded_for: get("x-forwarded-for").map(str::to_string),
    }
}

/// The query string of the two hand-written event routes, typed. A value that does not parse is rejected
/// (`invalid_argument`) rather than replaced by a default: `after_seq=abc` must not mean "from now" or "from the
/// start" to a client that believes it is resuming.
#[derive(Debug, Default, Deserialize)]
struct EventsQuery {
    after_seq: Option<u64>,
    limit: Option<u32>,
    heartbeat_ms: Option<u32>,
    topics: Option<String>,
}

impl EventsQuery {
    fn parse(query: Option<&str>) -> ApiResult<Self> {
        serde_urlencoded::from_str(query.unwrap_or("")).map_err(|e| ApiError::invalid(format!("bad query string: {e}")))
    }

    fn topics(&self) -> Vec<String> {
        self.topics
            .as_deref()
            .map(|t| t.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect())
            .unwrap_or_default()
    }
}

/// `Last-Event-ID`, if the client sent one. A value that is not a sequence number is rejected.
fn last_event_id(headers: &HeaderMap) -> ApiResult<Option<u64>> {
    headers
        .get("last-event-id")
        .map(|v| {
            v.to_str().ok().and_then(|v| v.trim().parse().ok()).ok_or_else(|| {
                ApiError::invalid("Last-Event-ID must be the sequence number of the last event received")
            })
        })
        .transpose()
}

fn path_with_repo(mut params: HashMap<String, String>) -> HashMap<String, String> {
    if let (Some(o), Some(n)) = (params.remove("owner"), params.remove("rname")) {
        params.insert("repo".into(), format!("{o}/{n}"));
    }
    params
}

/// Merges path parameters, the query string and the JSON body into one
/// request message. Path parameters win over everything else so a body cannot
/// redirect a request to a different resource than the URL names.
async fn parse_request<Req: DeserializeOwned>(
    app: &App,
    method: &Method,
    headers: &HeaderMap,
    params: HashMap<String, String>,
    request: Request,
) -> ApiResult<Req> {
    let query = request.uri().query().unwrap_or("").to_string();
    let limit = app.cfg.limits.max_request_bytes as usize;
    let has_body = !matches!(*method, Method::GET | Method::DELETE | Method::HEAD);
    let bytes = if has_body {
        let collected = http_body_util::Limited::new(request.into_body(), limit).collect().await.map_err(|e| {
            ApiError::new(ag_protocol::ErrorCode::TooLarge, format!("request body too large or unreadable: {e}"))
        })?;
        collected.to_bytes()
    } else {
        bytes::Bytes::new()
    };
    let mut obj: Map<String, Value> = if bytes.iter().all(u8::is_ascii_whitespace) {
        Map::new()
    } else {
        match serde_json::from_slice::<Value>(&bytes)
            .map_err(|e| ApiError::invalid(format!("invalid JSON body: {e}")))?
        {
            Value::Object(m) => m,
            _ => return Err(ApiError::invalid("the request body must be a JSON object")),
        }
    };
    // Query string values are strings; let serde_urlencoded coerce them using the
    // message's own types by deserializing the query separately and merging.
    let mut merged_query = String::new();
    for (k, v) in params.iter() {
        if !merged_query.is_empty() {
            merged_query.push('&');
        }
        merged_query.push_str(&serde_urlencoded::to_string([(k, v)]).unwrap_or_default());
    }
    if has_body {
        // Query parameters on body routes are only honoured for fields the body lacks.
        if !query.is_empty() {
            let q: Vec<(String, String)> =
                serde_urlencoded::from_str(&query).map_err(|e| ApiError::invalid(format!("bad query string: {e}")))?;
            for (k, v) in q {
                obj.entry(k).or_insert(Value::String(v));
            }
        }
        for (k, v) in &params {
            // Issue numbers are numeric path parameters; resource IDs remain strings.
            let value = if k == "number" {
                Value::from(v.parse::<u64>().map_err(|_| ApiError::invalid("invalid issue number"))?)
            } else {
                Value::String(v.clone())
            };
            obj.insert(k.clone(), value);
        }
        if let Some(key) = headers.get("idempotency-key").and_then(|v| v.to_str().ok())
            && !key.is_empty()
        {
            obj.entry("idempotency_key").or_insert(Value::String(key.to_string()));
        }
        // Numeric/boolean fields may arrive as strings from the query; the lenient
        // pass below handles that for GET-style routes only, so body fields must be typed.
        serde_json::from_value(Value::Object(obj)).map_err(|e| ApiError::invalid(format!("invalid request: {e}")))
    } else {
        let mut all = merged_query;
        if !query.is_empty() {
            if !all.is_empty() {
                all.push('&');
            }
            all.push_str(&query);
        }
        // Path parameters come first; for duplicates serde_urlencoded keeps the last,
        // so move them to the end to make them win.
        let mut pairs: Vec<(String, String)> =
            serde_urlencoded::from_str(&all).map_err(|e| ApiError::invalid(format!("bad query string: {e}")))?;
        pairs.retain(|(k, _)| !params.contains_key(k));
        for (k, v) in &params {
            pairs.push((k.clone(), v.clone()));
        }
        let encoded = serde_urlencoded::to_string(&pairs).map_err(|e| ApiError::invalid(e.to_string()))?;
        // `serde_html_form` (unlike `serde_urlencoded`) reads a repeated key into a list: `?features=a&features=b`.
        serde_html_form::from_str(&encoded).map_err(|e| ApiError::invalid(format!("invalid request: {e}")))
    }
}

/// The common path of every JSON route.
pub async fn handle<Req, Resp, F, Fut>(
    app: App,
    op: &'static str,
    method: Method,
    headers: HeaderMap,
    params: Option<Path<HashMap<String, String>>>,
    request: Request,
    f: F,
) -> Response
where
    Req: DeserializeOwned,
    Resp: Serialize,
    F: FnOnce(App, Call, Req) -> Fut,
    Fut: Future<Output = ApiResult<Resp>>,
{
    let ing = ingress_from_headers(&headers, remote_addr(&request));
    let request_id = ing.request_id.clone();
    let params = path_with_repo(params.map(|p| p.0).unwrap_or_default());
    let result = async {
        let req: Req = parse_request(&app, &method, &headers, params, request).await?;
        let a = app.clone();
        app.dispatch(ing, op, move |call| f(a, call, req)).await
    }
    .await;
    finish(result, &request_id)
}

fn finish<T: Serialize>(result: ApiResult<T>, request_id: &str) -> Response {
    match result {
        Ok(v) => {
            let mut resp = axum::Json(v).into_response();
            if let Ok(h) = HeaderValue::from_str(request_id) {
                resp.headers_mut().insert(HeaderName::from_static("x-request-id"), h);
            }
            resp
        }
        Err(e) => e.with_request_id(request_id).into_response(),
    }
}

fn octet(bytes: Vec<u8>, request_id: &str) -> Response {
    let mut resp = Response::new(Body::from(bytes));
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    if let Ok(h) = HeaderValue::from_str(request_id) {
        resp.headers_mut().insert(HeaderName::from_static("x-request-id"), h);
    }
    resp
}

/// Registers a JSON route and its metadata.
macro_rules! route {
    ($r:ident, $meta:ident, $method:ident, $verb:literal, $path:literal, $op:literal, $tag:literal, $summary:literal,
     $req:ty, $resp:ty, $m:ident) => {{
        $meta.push(RouteMeta::json::<$req, $resp>($verb, $path, $op, $tag, $summary));
        $r = $r.route(
            $path,
            $method(
                |State(app): State<App>,
                 method: Method,
                 headers: HeaderMap,
                 params: Option<Path<HashMap<String, String>>>,
                 request: Request| async move {
                    handle::<$req, $resp, _, _>(
                        app,
                        $op,
                        method,
                        headers,
                        params,
                        request,
                        |app, call, req| async move { app.$m(&call, req).await },
                    )
                    .await
                },
            ),
        );
    }};
}

/// Builds the API router and the metadata that describes it.
pub fn api_router(app: App) -> (Router, Vec<RouteMeta>) {
    let mut r: Router<App> = Router::new();
    let mut meta: Vec<RouteMeta> = Vec::new();

    // ---- session -------------------------------------------------------------------
    meta.push(
        RouteMeta::json::<v1::NegotiateRequest, v1::NegotiateResponse>(
            "get",
            "/v1/capabilities",
            "session.negotiate",
            "Session",
            "Protocol version, features, limits and accepted authentication methods (no credentials needed)",
        )
        .public(),
    );
    r = r.route(
        "/v1/capabilities",
        get(|State(app): State<App>, headers: HeaderMap, request: Request| async move {
            let id = ingress_from_headers(&headers, None).request_id;
            let result = async {
                let req: v1::NegotiateRequest =
                    parse_request(&app, &Method::GET, &headers, HashMap::new(), request).await?;
                app.negotiate(req)
            }
            .await;
            finish(result, &id)
        }),
    );
    meta.push(
        RouteMeta::json::<crate::ui::AuthConfigRequest, crate::ui::AuthConfig>(
            "get",
            "/v1/auth/config",
            "session.auth_config",
            "Session",
            "How a browser can sign in: the identity providers the Web UI redirects to, or `token` (no credentials needed)",
        )
        .public(),
    );
    r = r.route(
        "/v1/auth/config",
        get(|State(app): State<App>, headers: HeaderMap| async move {
            let id = ingress_from_headers(&headers, None).request_id;
            let mut resp = finish(Ok(crate::ui::auth_config(&app.cfg)), &id);
            resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            resp
        }),
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/whoami",
        "session.whoami",
        "Session",
        "The authenticated identity, its roles and credential scope",
        v1::WhoamiRequest,
        v1::Principal,
        whoami
    );

    // ---- repositories ----------------------------------------------------------------
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos",
        "repo.create",
        "Repository",
        "Create a repository",
        v1::CreateRepoRequest,
        v1::Repo,
        create_repo
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos",
        "repo.list",
        "Repository",
        "List repositories the caller can read",
        v1::ListReposRequest,
        v1::ListReposResponse,
        list_repos
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}",
        "repo.describe",
        "Repository",
        "Describe a repository",
        v1::GetRepoRequest,
        v1::Repo,
        describe_repo
    );
    route!(
        r,
        meta,
        patch,
        "patch",
        "/v1/repos/{owner}/{rname}",
        "repo.configure",
        "Repository",
        "Change repository settings and quota",
        v1::ConfigureRepoRequest,
        v1::Repo,
        configure_repo
    );

    // ---- objects ------------------------------------------------------------------------
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/objects/missing",
        "objects.missing",
        "Objects",
        "Which of these objects does the repository lack?",
        v1::MissingObjectsRequest,
        v1::MissingObjectsResponse,
        missing_objects
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/objects/verify",
        "objects.verify",
        "Objects",
        "Re-read and hash objects",
        v1::VerifyObjectsRequest,
        v1::VerifyObjectsResponse,
        verify_objects
    );
    meta.push(
        RouteMeta::raw(
            "put",
            "/v1/repos/{owner}/{rname}/objects/{object_id}",
            "objects.put",
            "Objects",
            "Upload one object envelope (binary body); the server verifies that it hashes to object_id and answers {object_id, stored}",
        )
        .request_body("application/octet-stream")
        .response_body("application/json"),
    );
    meta.push(RouteMeta::raw(
        "get",
        "/v1/repos/{owner}/{rname}/objects/{object_id}",
        "objects.get",
        "Objects",
        "Download one object envelope (binary body)",
    ));
    r = r.route(
        "/v1/repos/{owner}/{rname}/objects/{object_id}",
        put(
            |State(app): State<App>,
             headers: HeaderMap,
             params: Option<Path<HashMap<String, String>>>,
             request: Request| async move {
                let ing = ingress_from_headers(&headers, remote_addr(&request));
                let id = ing.request_id.clone();
                let params = path_with_repo(params.map(|p| p.0).unwrap_or_default());
                let (repo, oid) = (
                    params.get("repo").cloned().unwrap_or_default(),
                    params.get("object_id").cloned().unwrap_or_default(),
                );
                let a = app.clone();
                // Nothing is buffered for a caller who is not authenticated, rate limited and allowed to write
                // objects here: the body is read inside the pipeline, after those checks, and no further than the
                // limit that applies to this repository.
                let result = app
                    .dispatch(ing, "objects.put", move |call| async move {
                        a.authorize(&call, &repo, crate::authz::Cap::ObjectsWrite, None).await?;
                        let limit = usize::try_from(a.object_limit(&repo).await?).unwrap_or(usize::MAX);
                        // The bytes this body may take are reserved from the global budget before any is read:
                        // what the client announced, or - when it did not say - the most the limit allows.
                        let announced = request
                            .headers()
                            .get(header::CONTENT_LENGTH)
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.parse::<usize>().ok());
                        let _held = a.inflight.reserve(announced.map_or(limit, |n| n.min(limit)) as u64)?;
                        let body = http_body_util::Limited::new(request.into_body(), limit)
                            .collect()
                            .await
                            .map_err(|e| {
                                ApiError::new(
                                    ag_protocol::ErrorCode::TooLarge,
                                    format!("object too large or unreadable: {e}"),
                                )
                            })?
                            .to_bytes()
                            .to_vec();
                        let newly = a.store_object(&call, &repo, &oid, body).await?;
                        Ok(serde_json::json!({"object_id": oid, "stored": newly}))
                    })
                    .await;
                finish(result, &id)
            },
        )
        .get(
            |State(app): State<App>,
             headers: HeaderMap,
             params: Option<Path<HashMap<String, String>>>,
             request: Request| async move {
                let ing = ingress_from_headers(&headers, remote_addr(&request));
                let id = ing.request_id.clone();
                let params = path_with_repo(params.map(|p| p.0).unwrap_or_default());
                let result = async {
                    let req = v1::GetObjectRequest {
                        repo: params.get("repo").cloned().unwrap_or_default(),
                        object_id: params.get("object_id").cloned().unwrap_or_default(),
                    };
                    let a = app.clone();
                    app.dispatch(ing, "objects.get", move |call| async move { a.get_object(&call, req).await }).await
                }
                .await;
                match result {
                    Ok(bytes) => octet(bytes, &id),
                    Err(e) => e.with_request_id(&id).into_response(),
                }
            },
        )
        .layer(DefaultBodyLimit::disable()),
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/publish",
        "objects.publish",
        "Objects",
        "Publish a head and its unpublished ancestors",
        v1::PublishRequest,
        v1::Publication,
        publish
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/operations",
        "operations.get",
        "Objects",
        "The recorded outcome of an operation (by operation_id or idempotency_key)",
        v1::GetOperationRequest,
        v1::Operation,
        get_operation
    );

    // ---- history -------------------------------------------------------------------------
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/changeset",
        "history.changeset",
        "History",
        "One changeset by revision expression",
        v1::GetChangesetRequest,
        v1::Changeset,
        get_changeset
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/log",
        "history.log",
        "History",
        "History from a revision, newest first",
        v1::LogRequest,
        v1::LogResponse,
        log
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/blame",
        "history.blame",
        "History",
        "Which changeset introduced each line of a file (follows renames)",
        v1::BlameRequest,
        v1::BlameResponse,
        blame
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/history/ancestors",
        "history.ancestors",
        "History",
        "Changesets reachable from want but not from have, parents first",
        v1::AncestorsRequest,
        v1::AncestorsResponse,
        ancestors
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/history/merge-base",
        "history.merge_base",
        "History",
        "Merge bases of two revisions",
        v1::MergeBaseRequest,
        v1::MergeBaseResponse,
        merge_base
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/history/resolve",
        "history.resolve",
        "History",
        "Resolve a revision expression to a changeset id",
        v1::ResolveRevRequest,
        v1::ResolveRevResponse,
        resolve_rev
    );

    // ---- trees -----------------------------------------------------------------------------
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/tree",
        "tree.get",
        "Tree",
        "The whole tree of a revision (paginated manifest)",
        v1::GetTreeRequest,
        v1::GetTreeResponse,
        get_tree
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/tree/resolve",
        "tree.resolve",
        "Tree",
        "What exists at a path",
        v1::ResolvePathRequest,
        v1::ResolvePathResponse,
        resolve_path
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/tree/list",
        "tree.list",
        "Tree",
        "List a directory",
        v1::ListDirectoryRequest,
        v1::ListDirectoryResponse,
        list_directory
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/search",
        "tree.search",
        "Tree",
        "Literal content search at a revision, with bounded resumable scanning",
        v1::SearchTreeRequest,
        v1::SearchTreeResponse,
        search_tree
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/diff",
        "tree.diff",
        "Tree",
        "Per-file diffs between two revisions",
        v1::DiffRequest,
        v1::DiffResponse,
        diff
    );
    meta.push(
        RouteMeta::raw(
            "get",
            "/v1/repos/{owner}/{rname}/files/{*path}",
            "tree.file",
            "Tree",
            "File content at a revision (query: rev); binary body",
        )
        .query("rev", "string", "Revision expression; empty means the repository's default stream"),
    );
    r = r.route(
        "/v1/repos/{owner}/{rname}/files/{*path}",
        get(
            |State(app): State<App>,
             headers: HeaderMap,
             params: Option<Path<HashMap<String, String>>>,
             request: Request| async move {
                let ing = ingress_from_headers(&headers, remote_addr(&request));
                let id = ing.request_id.clone();
                let params = path_with_repo(params.map(|p| p.0).unwrap_or_default());
                let rev = match serde_urlencoded::from_str::<Vec<(String, String)>>(request.uri().query().unwrap_or(""))
                {
                    Ok(pairs) => pairs.into_iter().find(|(k, _)| k == "rev").map(|(_, v)| v).unwrap_or_default(),
                    Err(e) => {
                        return ApiError::invalid(format!("bad query string: {e}"))
                            .with_request_id(&id)
                            .into_response();
                    }
                };
                let req = v1::GetFileRequest {
                    repo: params.get("repo").cloned().unwrap_or_default(),
                    rev,
                    path: params.get("path").cloned().unwrap_or_default(),
                };
                let a = app.clone();
                match app.dispatch(ing, "tree.file", move |call| async move { a.file_content(&call, req).await }).await
                {
                    Ok(bytes) => octet(bytes, &id),
                    Err(e) => e.with_request_id(&id).into_response(),
                }
            },
        ),
    );

    // ---- streams -----------------------------------------------------------------------------
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/streams",
        "stream.list",
        "Stream",
        "List streams",
        v1::ListStreamsRequest,
        v1::ListStreamsResponse,
        list_streams
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/streams",
        "stream.create",
        "Stream",
        "Create a stream",
        v1::CreateStreamRequest,
        v1::Stream,
        create_stream
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/streams/head",
        "stream.head",
        "Stream",
        "Read a stream head (optionally long-poll for a newer version)",
        v1::ReadHeadRequest,
        v1::Stream,
        read_head
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/streams/cas",
        "stream.cas",
        "Stream",
        "Administrative compare-and-swap of a stream head",
        v1::CompareAndSwapRequest,
        v1::Stream,
        compare_and_swap
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/reflog",
        "stream.reflog",
        "Stream",
        "The reflog of a stream",
        v1::ReflogRequest,
        v1::ReflogResponse,
        reflog
    );

    // ---- workspaces ------------------------------------------------------------------------------
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/workspaces",
        "workspace.create",
        "Workspace",
        "Create a virtual workspace",
        v1::CreateWorkspaceRequest,
        v1::Workspace,
        create_workspace
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/workspaces",
        "workspace.list",
        "Workspace",
        "List workspaces",
        v1::ListWorkspacesRequest,
        v1::ListWorkspacesResponse,
        list_workspaces
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/workspaces/{name}",
        "workspace.get",
        "Workspace",
        "One workspace",
        v1::GetWorkspaceRequest,
        v1::Workspace,
        get_workspace
    );
    route!(
        r,
        meta,
        delete,
        "delete",
        "/v1/repos/{owner}/{rname}/workspaces/{name}",
        "workspace.delete",
        "Workspace",
        "Remove a workspace",
        v1::DeleteWorkspaceRequest,
        v1::Empty,
        delete_workspace
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/workspaces/{name}/changes",
        "workspace.modify",
        "Workspace",
        "Apply file changes as one new changeset",
        v1::ModifyWorkspaceRequest,
        v1::Workspace,
        modify_workspace
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/workspaces/{name}/sync",
        "workspace.sync",
        "Workspace",
        "Bring the stream head into the workspace (with optional conflict resolutions)",
        v1::SyncWorkspaceRequest,
        v1::SyncWorkspaceResponse,
        sync_workspace
    );

    // ---- proposals ----------------------------------------------------------------------------------
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/proposals",
        "proposal.create",
        "Proposal",
        "Open a proposal",
        v1::CreateProposalRequest,
        v1::Proposal,
        create_proposal
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/proposals",
        "proposal.list",
        "Proposal",
        "List proposals",
        v1::ListProposalsRequest,
        v1::ListProposalsResponse,
        list_proposals
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/proposals/{id}",
        "proposal.get",
        "Proposal",
        "One proposal with its reviews",
        v1::GetProposalRequest,
        v1::ProposalDetail,
        get_proposal
    );
    route!(
        r,
        meta,
        patch,
        "patch",
        "/v1/repos/{owner}/{rname}/proposals/{id}",
        "proposal.update",
        "Proposal",
        "Move a proposal to a newer head",
        v1::UpdateProposalRequest,
        v1::Proposal,
        update_proposal
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/proposals/{id}/reviews",
        "proposal.review",
        "Proposal",
        "Review the current head",
        v1::ReviewProposalRequest,
        v1::Review,
        review_proposal
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/proposals/{id}/close",
        "proposal.close",
        "Proposal",
        "Close without integrating",
        v1::CloseProposalRequest,
        v1::Proposal,
        close_proposal
    );

    // ---- integration ----------------------------------------------------------------------------------
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/proposals/{proposal}/prepare",
        "integration.prepare",
        "Integration",
        "What integrating would do now, and what is missing",
        v1::PrepareRequest,
        v1::Preparation,
        prepare
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/proposals/{proposal}/validate",
        "integration.validate",
        "Integration",
        "Queue validation jobs for the candidate",
        v1::RequestValidationRequest,
        v1::RequestValidationResponse,
        request_validation
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/proposals/{proposal}/integrate",
        "integration.integrate",
        "Integration",
        "Integrate the proposal into its stream",
        v1::IntegrateRequest,
        v1::IntegrationResult,
        integrate
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/attestations",
        "integration.attest",
        "Integration",
        "Submit a signed attestation",
        v1::SubmitAttestationRequest,
        v1::SubmitAttestationResponse,
        submit_attestation
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/attestations",
        "integration.attestations",
        "Integration",
        "Attestations for a tree",
        v1::ListAttestationsRequest,
        v1::ListAttestationsResponse,
        list_attestations
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/queue",
        "queue.enqueue",
        "Integration",
        "Append a proposal to the integration queue",
        v1::EnqueueRequest,
        v1::QueueEntry,
        enqueue
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/queue",
        "queue.list",
        "Integration",
        "The queue of a stream",
        v1::ListQueueRequest,
        v1::ListQueueResponse,
        list_queue
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/queue/run",
        "queue.run",
        "Integration",
        "Process queued proposals in order",
        v1::RunQueueRequest,
        v1::RunQueueResponse,
        run_queue
    );

    // ---- agents ------------------------------------------------------------------------------------------
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/agents",
        "agent.register",
        "Agent",
        "Register an agent and its public key",
        v1::RegisterAgentRequest,
        v1::Agent,
        register_agent
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/agents",
        "agent.list",
        "Agent",
        "List agents",
        v1::ListAgentsRequest,
        v1::ListAgentsResponse,
        list_agents
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/runs",
        "agent.start_run",
        "Agent",
        "Start a run (returns the identity to act as)",
        v1::StartRunRequest,
        v1::Run,
        start_run
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/runs",
        "agent.list_runs",
        "Agent",
        "List runs",
        v1::ListRunsRequest,
        v1::ListRunsResponse,
        list_runs
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/runs/{run_id}",
        "agent.get_run",
        "Agent",
        "Look up a current or historical agent run",
        v1::GetRunRequest,
        v1::Run,
        get_run
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/runs/reap",
        "agent.reap_runs",
        "Agent",
        "Operator: end the runs that stopped heartbeating, release their leases and revoke their credentials",
        v1::ReapRunsRequest,
        v1::ReapRunsResponse,
        reap_runs
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/runs/{run_id}/heartbeat",
        "agent.heartbeat",
        "Agent",
        "Run heartbeat",
        v1::HeartbeatRequest,
        v1::HeartbeatResponse,
        heartbeat
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/runs/{run_id}/complete",
        "agent.complete",
        "Agent",
        "Finish a run",
        v1::CompleteRunRequest,
        v1::Run,
        complete_run
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/tasks",
        "task.create",
        "Agent",
        "Create a task",
        v1::CreateTaskRequest,
        v1::Task,
        create_task
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/tasks",
        "task.list",
        "Agent",
        "List tasks",
        v1::ListTasksRequest,
        v1::ListTasksResponse,
        list_tasks
    );
    route!(
        r,
        meta,
        put,
        "put",
        "/v1/repos/{owner}/{rname}/agents/{agent}/executor",
        "agent.executor.report",
        "Agent",
        "Report expiring executor connectivity",
        v1::ReportExecutorRequest,
        v1::ExecutorPresence,
        report_executor
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/runs/{run_id}/output",
        "agent.output.get",
        "Agent",
        "Read saved executor output",
        v1::GetRunOutputRequest,
        v1::RunOutput,
        get_run_output
    );
    route!(
        r,
        meta,
        put,
        "put",
        "/v1/repos/{owner}/{rname}/runs/{run_id}/output",
        "agent.output.save",
        "Agent",
        "Save bounded executor output",
        v1::SaveRunOutputRequest,
        v1::RunOutput,
        save_run_output
    );
    route!(
        r,
        meta,
        delete,
        "delete",
        "/v1/repos/{owner}/{rname}/runs/{run_id}/output",
        "agent.output.delete",
        "Agent",
        "Delete saved executor output",
        v1::GetRunOutputRequest,
        v1::Empty,
        delete_run_output
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/tasks/scheduling",
        "task.scheduling.get",
        "Agent",
        "Read the gate for new task launches",
        v1::GetTaskSchedulingRequest,
        v1::TaskScheduling,
        get_task_scheduling
    );
    route!(
        r,
        meta,
        put,
        "put",
        "/v1/repos/{owner}/{rname}/tasks/scheduling",
        "task.scheduling.set",
        "Agent",
        "Pause or resume new task launches",
        v1::SetTaskSchedulingRequest,
        v1::TaskScheduling,
        set_task_scheduling
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/tasks/{id}",
        "task.get",
        "Agent",
        "One task",
        v1::GetTaskRequest,
        v1::Task,
        get_task
    );
    route!(
        r,
        meta,
        put,
        "put",
        "/v1/repos/{owner}/{rname}/tasks/{id}/assignee",
        "task.assign",
        "Agent",
        "Assign queued work to a registered agent",
        v1::AssignTaskRequest,
        v1::Task,
        assign_task
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/tasks/{task}/dependencies",
        "task.depend",
        "Agent",
        "Add a task dependency",
        v1::AddDependencyRequest,
        v1::Task,
        add_dependency
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/tasks/{id}/finish",
        "task.finish",
        "Agent",
        "Mark a task done or cancelled",
        v1::FinishTaskRequest,
        v1::Task,
        finish_task
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/leases",
        "lease.acquire",
        "Agent",
        "Acquire a soft or exclusive lease",
        v1::AcquireLeaseRequest,
        v1::AcquireLeaseResponse,
        acquire_lease
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/leases",
        "lease.list",
        "Agent",
        "List leases",
        v1::ListLeasesRequest,
        v1::ListLeasesResponse,
        list_leases
    );
    route!(
        r,
        meta,
        delete,
        "delete",
        "/v1/repos/{owner}/{rname}/leases/{lease_id}",
        "lease.release",
        "Agent",
        "Release a lease",
        v1::LeaseRef,
        v1::Empty,
        release_lease
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/leases/{lease_id}/extend",
        "lease.extend",
        "Agent",
        "Extend a lease",
        v1::ExtendLeaseRequest,
        v1::Lease,
        extend_lease
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/provenance",
        "provenance.submit",
        "Agent",
        "Submit a signed provenance record",
        v1::SubmitProvenanceRequest,
        v1::ProvenanceEntry,
        submit_provenance
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/provenance",
        "provenance.list",
        "Agent",
        "Provenance records of a changeset",
        v1::ListProvenanceRequest,
        v1::ListProvenanceResponse,
        list_provenance
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/predict",
        "agent.predict",
        "Agent",
        "Predict overlapping work on a stream",
        v1::PredictRequest,
        v1::PredictResponse,
        predict
    );

    // ---- events ---------------------------------------------------------------------------------------------
    meta.push(RouteMeta::json::<v1::ReplayRequest, v1::ReplayResponse>(
        "get",
        "/v1/repos/{owner}/{rname}/events/replay",
        "events.replay",
        "Events",
        "Replay events after a sequence number (query: after_seq, limit, topics=a,b)",
    ));
    r = r.route(
        "/v1/repos/{owner}/{rname}/events/replay",
        get(
            |State(app): State<App>,
             headers: HeaderMap,
             params: Option<Path<HashMap<String, String>>>,
             request: Request| async move {
                let ing = ingress_from_headers(&headers, remote_addr(&request));
                let id = ing.request_id.clone();
                let params = path_with_repo(params.map(|p| p.0).unwrap_or_default());
                let q = match EventsQuery::parse(request.uri().query()) {
                    Ok(q) => q,
                    Err(e) => return e.with_request_id(&id).into_response(),
                };
                let req = v1::ReplayRequest {
                    repo: params.get("repo").cloned().unwrap_or_default(),
                    topics: q.topics(),
                    after_seq: q.after_seq.unwrap_or(0),
                    limit: q.limit.unwrap_or(0),
                };
                let a = app.clone();
                finish(
                    app.dispatch(ing, "events.replay", move |call| async move { a.replay(&call, req).await }).await,
                    &id,
                )
            },
        ),
    );
    meta.push(
        RouteMeta::raw(
            "get",
            "/v1/repos/{owner}/{rname}/events",
            "events.subscribe",
            "Events",
            "Server-Sent Events: replay after after_seq (or Last-Event-ID), then follow live",
        )
        .query(
            "after_seq",
            "integer",
            "Replay events with a larger sequence number first; absent means follow from now",
        )
        .query("topics", "string", "Comma-separated topic prefixes, e.g. stream,proposal")
        .query("heartbeat_ms", "integer", "Interval of keep-alive comments (0 = server default)")
        .response_body("text/event-stream"),
    );
    r = r.route(
        "/v1/repos/{owner}/{rname}/events",
        get(
            |State(app): State<App>,
             headers: HeaderMap,
             params: Option<Path<HashMap<String, String>>>,
             request: Request| async move {
                let ing = ingress_from_headers(&headers, remote_addr(&request));
                let id = ing.request_id.clone();
                let params = path_with_repo(params.map(|p| p.0).unwrap_or_default());
                let (q, last_event_id) = match EventsQuery::parse(request.uri().query())
                    .and_then(|q| last_event_id(&headers).map(|l| (q, l)))
                {
                    Ok(parsed) => parsed,
                    Err(e) => return e.with_request_id(&id).into_response(),
                };
                let req = v1::SubscribeRequest {
                    repo: params.get("repo").cloned().unwrap_or_default(),
                    topics: q.topics(),
                    // No `after_seq` and no Last-Event-ID: follow from now.
                    after_seq: q.after_seq.or(last_event_id).unwrap_or(u64::MAX),
                    heartbeat_ms: q.heartbeat_ms.unwrap_or(0),
                };
                let a = app.clone();
                // Streams are long-lived: authentication happens once up front, then the feed re-checks access on every batch.
                let mut ing = ing;
                ing.deadline_ms = Some(crate::app::NO_DEADLINE_MS);
                let stream = app
                    .dispatch(ing, "events.subscribe", move |call| async move { a.subscribe(call, req).await })
                    .await;
                match stream {
                    Err(e) => e.with_request_id(&id).into_response(),
                    Ok(stream) => {
                        let sse = stream.map(|item| -> Result<SseEvent, Infallible> {
                            Ok(match item {
                                Ok(ev) if ev.seq == 0 => SseEvent::default().comment("heartbeat"),
                                Ok(ev) => SseEvent::default()
                                    .id(ev.seq.to_string())
                                    .event(ev.topic.clone())
                                    .data(serde_json::to_string(&ev).unwrap_or_else(|_| "{}".into())),
                                Err(e) => SseEvent::default()
                                    .event("error")
                                    .data(serde_json::json!({"error": e.0.to_info()}).to_string()),
                            })
                        });
                        let mut resp = Sse::new(sse)
                            .keep_alive(KeepAlive::new().interval(Duration::from_secs(25)))
                            .into_response();
                        resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
                        resp.headers_mut()
                            .insert(HeaderName::from_static("x-accel-buffering"), HeaderValue::from_static("no"));
                        resp
                    }
                }
            },
        ),
    );

    // ---- admin ------------------------------------------------------------------------------------------------------
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/policy",
        "admin.policy.get",
        "Admin",
        "The policy in force for a stream",
        v1::GetPolicyRequest,
        v1::PolicyVersion,
        get_policy
    );
    route!(
        r,
        meta,
        put,
        "put",
        "/v1/repos/{owner}/{rname}/policy",
        "admin.policy.set",
        "Admin",
        "Store a new policy version",
        v1::SetPolicyRequest,
        v1::PolicyVersion,
        set_policy
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/policy/history",
        "admin.policy.history",
        "Admin",
        "All versions of a stream's policy",
        v1::PolicyHistoryRequest,
        v1::PolicyHistoryResponse,
        policy_history
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/pins",
        "admin.pin",
        "Admin",
        "Pin a revision as a GC root",
        v1::PinRequest,
        v1::Pin,
        pin
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/pins",
        "admin.pins",
        "Admin",
        "List pins",
        v1::ListPinsRequest,
        v1::ListPinsResponse,
        list_pins
    );
    route!(
        r,
        meta,
        delete,
        "delete",
        "/v1/repos/{owner}/{rname}/pins",
        "admin.unpin",
        "Admin",
        "Remove a pin (query: name)",
        v1::UnpinRequest,
        v1::Empty,
        unpin
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/audit",
        "admin.audit",
        "Admin",
        "Audit entries (newest first)",
        v1::ListAuditRequest,
        v1::ListAuditResponse,
        list_audit
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/audit/verify",
        "admin.audit.verify",
        "Admin",
        "Verify the audit hash chain",
        v1::VerifyAuditRequest,
        v1::VerifyAuditResponse,
        verify_audit
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/gc",
        "admin.gc",
        "Admin",
        "Garbage collection",
        v1::GcRequest,
        v1::GcResponse,
        gc
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/fsck",
        "admin.fsck",
        "Admin",
        "Integrity check",
        v1::FsckRequest,
        v1::FsckResponse,
        fsck
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/credentials",
        "admin.credentials.mint",
        "Admin",
        "Mint a scoped, expiring credential (the token is shown once)",
        v1::MintCredentialRequest,
        v1::MintCredentialResponse,
        mint_credential
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/credentials",
        "admin.credentials.list",
        "Admin",
        "List credentials",
        v1::ListCredentialsRequest,
        v1::ListCredentialsResponse,
        list_credentials
    );
    route!(
        r,
        meta,
        delete,
        "delete",
        "/v1/credentials/{credential_id}",
        "admin.credentials.revoke",
        "Admin",
        "Revoke a credential",
        v1::RevokeCredentialRequest,
        v1::RevokeCredentialResponse,
        revoke_credential
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/roles",
        "admin.roles.grant",
        "Admin",
        "Grant a role",
        v1::GrantRoleRequest,
        v1::RoleBinding,
        grant_role
    );
    route!(
        r,
        meta,
        delete,
        "delete",
        "/v1/roles",
        "admin.roles.revoke",
        "Admin",
        "Revoke a role (query: subject, repo, role)",
        v1::RevokeRoleRequest,
        v1::Empty,
        revoke_role
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/roles",
        "admin.roles.list",
        "Admin",
        "List role bindings",
        v1::ListRolesRequest,
        v1::ListRolesResponse,
        list_roles
    );

    crate::jobs_http::add_routes(&mut r, &mut meta);
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/issues",
        "issue.create",
        "Issues",
        "Create a user issue",
        v1::CreateIssueRequest,
        v1::Issue,
        create_issue
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/issues",
        "issue.list",
        "Issues",
        "List user issues",
        v1::ListIssuesRequest,
        v1::ListIssuesResponse,
        list_issues
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/issues/{number}",
        "issue.get",
        "Issues",
        "Read a user issue",
        v1::GetIssueRequest,
        v1::Issue,
        get_issue
    );
    route!(
        r,
        meta,
        put,
        "put",
        "/v1/repos/{owner}/{rname}/issues/{number}",
        "issue.update",
        "Issues",
        "Edit or close an issue at an expected version",
        v1::UpdateIssueRequest,
        v1::Issue,
        update_issue
    );
    route!(
        r,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/issues/{number}/comments",
        "issue.comment",
        "Issues",
        "Add an issue comment",
        v1::AddIssueCommentRequest,
        v1::IssueActivity,
        add_issue_comment
    );
    route!(
        r,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/issues/{number}/activity",
        "issue.activity",
        "Issues",
        "Read issue discussion and history",
        v1::ListIssueActivityRequest,
        v1::ListIssueActivityResponse,
        list_issue_activity
    );
    crate::migration::add_routes(&mut r, &mut meta);

    (r.with_state(app), meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_from_clients_are_sanitized() {
        let mut h = HeaderMap::new();
        h.insert("x-request-id", HeaderValue::from_static("abc-123_x.y"));
        assert_eq!(ingress_from_headers(&h, None).request_id, "abc-123_x.y");
        h.insert("x-request-id", HeaderValue::from_static("bad id with spaces"));
        assert!(ingress_from_headers(&h, None).request_id.starts_with("req_"));
        h.insert("x-request-id", HeaderValue::from_str(&"a".repeat(200)).unwrap());
        assert!(ingress_from_headers(&h, None).request_id.starts_with("req_"));
    }

    #[test]
    fn bearer_tokens_are_parsed_case_insensitively() {
        let mut h = HeaderMap::new();
        h.insert("authorization", HeaderValue::from_static("bearer abc"));
        assert_eq!(ingress_from_headers(&h, None).bearer.as_deref(), Some("abc"));
        h.insert("authorization", HeaderValue::from_static("Basic abc"));
        assert_eq!(ingress_from_headers(&h, None).bearer, None);
    }
}
