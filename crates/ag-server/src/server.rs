//! Assembling and running the server.

use std::future::Future;
use std::net::SocketAddr;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::app::App;
use crate::error::ApiError;
use crate::routes::{RouteMeta, openapi};

/// Installs the process-wide TLS crypto provider (aws-lc-rs). Several TLS
/// stacks are linked into the server; choosing explicitly avoids a runtime panic
/// when rustls cannot decide. Safe to call repeatedly.
pub fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Builds the complete router: API, gRPC, health, metrics, OpenAPI and (if
/// configured) the Web UI. Returns the route table too (for tests and docs).
pub fn build_router(app: App) -> (Router, Vec<RouteMeta>) {
    let (api, routes) = crate::http::api_router(app.clone());
    let spec = openapi(&routes, env!("CARGO_PKG_VERSION"));
    let spec_json = serde_json::to_string_pretty(&spec).unwrap_or_default();

    let mut extras: Router<App> =
        Router::new().route("/healthz", get(|| async { "ok" })).route("/readyz", get(readyz)).route(
            "/openapi.json",
            get(move || {
                let body = spec_json.clone();
                async move { ([(header::CONTENT_TYPE, "application/json")], body) }
            }),
        );
    if app.cfg.metrics.enabled {
        extras = extras.route("/metrics", get(metrics));
    }
    let router = api.merge(crate::grpc::router(app.clone())).merge(extras.with_state(app.clone()));

    let router = match &app.cfg.server.web_dir {
        Some(dir) => {
            let index = dir.join("index.html");
            let files = ServeDir::new(dir).fallback(ServeFile::new(index));
            let hardened = tower::ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::overriding(
                    HeaderName::from_static("x-content-type-options"),
                    HeaderValue::from_static("nosniff"),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    HeaderName::from_static("referrer-policy"),
                    HeaderValue::from_static("no-referrer"),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    HeaderName::from_static("content-security-policy"),
                    HeaderValue::from_static(
                        "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'",
                    ),
                ))
                .service(files);
            router.fallback_service(tower::service_fn(move |req: Request| {
                let mut svc = hardened.clone();
                async move {
                    let path = req.uri().path().to_string();
                    if path.starts_with("/v1/") || path.starts_with("/agwp.") {
                        return Ok::<_, std::convert::Infallible>(not_found());
                    }
                    tower::Service::call(&mut svc, req).await.map(IntoResponse::into_response)
                }
            }))
        }
        None => router.fallback(|| async { not_found() }),
    };
    (cors(router, &app), routes)
}

fn not_found() -> Response {
    ApiError::not_found("no such route").into_response()
}

fn cors(router: Router, app: &App) -> Router {
    if app.cfg.server.cors_origins.is_empty() {
        return router;
    }
    let origins: Vec<HeaderValue> =
        app.cfg.server.cors_origins.iter().filter_map(|o| HeaderValue::from_str(o).ok()).collect();
    router.layer(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE, Method::OPTIONS])
            .allow_headers([
                header::AUTHORIZATION,
                header::CONTENT_TYPE,
                HeaderName::from_static("ag-run"),
                HeaderName::from_static("idempotency-key"),
                HeaderName::from_static("request-timeout"),
                HeaderName::from_static("last-event-id"),
                HeaderName::from_static("x-request-id"),
            ])
            .expose_headers([HeaderName::from_static("x-request-id"), header::RETRY_AFTER])
            .max_age(std::time::Duration::from_secs(600)),
    )
}

async fn readyz(State(app): State<App>) -> Response {
    let control = app.control.clone();
    match tokio::task::spawn_blocking(move || control.kv_get("ready-probe")).await {
        Ok(Ok(_)) => "ready".into_response(),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "control database unavailable").into_response(),
    }
}

async fn metrics(State(app): State<App>, req: Request) -> Response {
    if let Some(expected) = std::env::var(&app.cfg.metrics.token_env).ok().filter(|t| !t.is_empty()) {
        let presented = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
            .unwrap_or("");
        let (a, b): ([u8; 32], [u8; 32]) =
            (Sha256::digest(presented.as_bytes()).into(), Sha256::digest(expected.as_bytes()).into());
        if !bool::from(a.ct_eq(&b)) {
            return ApiError::new(ag_protocol::ErrorCode::Unauthenticated, "metrics require a bearer token")
                .into_response();
        }
    }
    app.metrics.open_repos.set(app.repos.open_count() as i64);
    app.metrics.uptime.set(app.started.elapsed().as_secs() as i64);
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], app.metrics.render()).into_response()
}

/// Serves until `shutdown` completes.
async fn serve_inner(
    app: App,
    listener: tokio::net::TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    install_crypto_provider();
    let (router, _) = build_router(app.clone());
    let addr = listener.local_addr()?;
    let signal_app = app.clone();
    let shutdown = async move {
        shutdown.await;
        // Streams are long-lived; ask them to finish so graceful shutdown can complete.
        signal_app.begin_shutdown();
    };
    if app.cfg.tls_enabled() {
        let cert = app.cfg.server.tls_cert.clone().expect("checked");
        let key = app.cfg.server.tls_key.clone().expect("checked");
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
        let handle = axum_server::Handle::new();
        let h2 = handle.clone();
        tokio::spawn(async move {
            shutdown.await;
            h2.graceful_shutdown(Some(std::time::Duration::from_secs(20)));
        });
        tracing::info!(%addr, "listening (TLS: HTTPS + gRPC)");
        axum_server::from_tcp_rustls(listener.into_std()?, tls)?
            .handle(handle)
            .serve(router.into_make_service_with_connect_info::<SocketAddr>())
            .await?;
    } else {
        tracing::info!(%addr, "listening (plaintext: HTTP + h2c gRPC)");
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(shutdown)
            .await?;
    }
    Ok(())
}

/// Runs the server until `shutdown` completes, then closes the repositories.
pub async fn serve(
    app: App,
    listener: tokio::net::TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let result = serve_inner(app.clone(), listener, shutdown).await;
    let a = app.clone();
    let _ = tokio::task::spawn_blocking(move || a.repos.close_all()).await;
    result
}
