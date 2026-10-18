//! Shared application state and the request pipeline.
//!
//! Both transports build an [`Ingress`] (credentials, deadline, trace id) and
//! call [`App::dispatch`], which authenticates, applies the default deadline,
//! tracks in-flight requests and records metrics around the operation.
//! Repository-scoped operations then go through [`App::with_repo`], which
//! authorizes, rate limits and runs the engine call on a blocking thread.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ag_integrate::Repository;
use ag_storage::{Clock, SystemClock};
use ag_types::{Identity, IdentityKind};
use tokio::sync::{Notify, Semaphore};

use crate::auth::{Authenticator, Principal};
use crate::authz::{Authorizer, Binding, Cap};
use crate::cluster::{Bus, BusMetrics, Local, Signal};
use crate::config::ServerConfig;
use crate::control::Control;
use crate::error::{ApiError, ApiResult};
use crate::limits::RateLimiter;
use crate::metrics::Metrics;
use crate::repos::RepoManager;

/// What a transport extracted from a request before the operation runs.
#[derive(Clone, Debug, Default)]
pub struct Ingress {
    /// `http` or `grpc`.
    pub transport: &'static str,
    /// Bearer credential, if presented.
    pub bearer: Option<String>,
    /// Agent run to act as (`ag-run` header/metadata).
    pub run: Option<String>,
    /// Correlation id (generated when the client sent none).
    pub request_id: String,
    /// Client-requested deadline in milliseconds.
    pub deadline_ms: Option<u64>,
    /// W3C `traceparent`, passed through to logs.
    pub traceparent: Option<String>,
    /// Peer address, for logs.
    pub remote: Option<String>,
}

/// An authenticated request in flight.
#[derive(Clone, Debug)]
pub struct Call {
    /// Who authenticated.
    pub principal: Arc<Principal>,
    /// Agent run to act as, if requested.
    pub run: Option<String>,
    /// Correlation id.
    pub request_id: String,
    /// Operation name, for metrics and logs.
    pub op: &'static str,
    /// Transport name.
    pub transport: &'static str,
    /// Deadline.
    pub deadline: Option<Instant>,
}

/// Role bindings with the time and epoch they were loaded at.
type BindingsCache = (Instant, u64, Arc<Vec<Binding>>);

/// Process-wide state, shared by every request.
pub struct AppState {
    /// Configuration.
    pub cfg: ServerConfig,
    /// Time source.
    pub clock: Arc<dyn Clock>,
    /// Control-plane database.
    pub control: Arc<Control>,
    /// Credential verification.
    pub auth: Authenticator,
    /// Hosted repositories.
    pub repos: RepoManager,
    /// Trace exporter, when `telemetry.otlp_endpoint` is set.
    pub tracer: Option<Arc<crate::trace::Tracer>>,
    /// Request rate limiting.
    pub limiter: RateLimiter,
    /// Prometheus metrics.
    pub metrics: Metrics,
    /// Bounds concurrent requests; excess is shed.
    pub concurrency: Semaphore,
    /// When the process started.
    pub started: Instant,
    /// Set when the server begins shutting down; long-lived streams end.
    shutdown: tokio::sync::watch::Sender<bool>,
    bindings: Mutex<Option<BindingsCache>>,
    /// What waits inside this process (event feeds, job claimers, the bindings cache epoch).
    pub(crate) local: Arc<Local>,
    pub(crate) job_rescan: Mutex<Instant>,
    /// Tells the other replicas of this installation what changed (inert on SQLite).
    bus: Bus,
    /// Name of this replica (logs, metrics).
    pub replica_id: String,
    /// Set from SIGTERM until exit: `/readyz` says "not ready" so balancers stop sending requests.
    draining: AtomicBool,
}

/// Cheap, clonable handle to the shared state.
#[derive(Clone)]
pub struct App(pub Arc<AppState>);

impl std::ops::Deref for App {
    type Target = AppState;
    fn deref(&self) -> &AppState {
        &self.0
    }
}

/// `server.replica_id`, else `$HOSTNAME` (the pod name on Kubernetes), else random.
fn resolve_replica_id(cfg: &ServerConfig) -> String {
    let clean = |s: &str| -> Option<String> {
        let s: String = s
            .trim()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' })
            .take(63)
            .collect();
        (!s.is_empty()).then_some(s)
    };
    cfg.server
        .replica_id
        .as_deref()
        .and_then(clean)
        .or_else(|| std::env::var("HOSTNAME").ok().as_deref().and_then(clean))
        .unwrap_or_else(|| format!("r-{}", random_hex()))
}

fn random_hex() -> String {
    ag_crypto::random_bytes::<4>().map(hex::encode).unwrap_or_else(|_| format!("{:08x}", std::process::id()))
}

/// Parses `name=value,name2=value2` (the `OTEL_EXPORTER_OTLP_HEADERS` format).
fn parse_headers(s: &str) -> Vec<(String, String)> {
    s.split(',')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            let (k, v) = (k.trim(), v.trim());
            (!k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') && !v.is_empty())
                .then(|| (k.to_ascii_lowercase(), v.to_string()))
        })
        .collect()
}

impl App {
    /// Builds the state: opens the control database, prepares authentication.
    pub fn new(cfg: ServerConfig) -> ApiResult<Self> {
        Self::with_clock(cfg, Arc::new(SystemClock))
    }

    /// Like [`App::new`] with an explicit clock (tests).
    pub fn with_clock(cfg: ServerConfig, clock: Arc<dyn Clock>) -> ApiResult<Self> {
        let token = std::env::var(&cfg.auth.bootstrap_token_env).ok();
        Self::with_options(cfg, clock, token)
    }

    /// Full control: explicit clock and bootstrap token (tests, embedding).
    pub fn with_options(cfg: ServerConfig, clock: Arc<dyn Clock>, bootstrap_token: Option<String>) -> ApiResult<Self> {
        std::fs::create_dir_all(&cfg.server.data_dir).map_err(ApiError::internal)?;
        let control = Arc::new(Control::from_config(&cfg, clock.clone())?);
        if cfg.control.is_postgres() && !cfg.repos_are_shared() {
            tracing::warn!(
                "the control database is shared (PostgreSQL) but repositories are stored on this host's disk: run \
                 exactly one replica, or set [repos.storage] to PostgreSQL metadata and S3 objects"
            );
        }
        let auth = Authenticator::with_bootstrap(control.clone(), clock.clone(), &cfg.auth, bootstrap_token)?;
        let repos = RepoManager::new(
            &cfg.server.data_dir,
            control.clone(),
            clock.clone(),
            cfg.repos.pool_size,
            cfg.repos.max_open,
            cfg.repos.storage.clone(),
        );
        let metrics = Metrics::new();
        let tracer = (!cfg.telemetry.otlp_endpoint.is_empty()).then(|| {
            let headers =
                std::env::var(&cfg.telemetry.otlp_headers_env).ok().map(|v| parse_headers(&v)).unwrap_or_default();
            crate::trace::Tracer::start(
                crate::trace::TracerConfig {
                    endpoint: cfg.telemetry.otlp_endpoint.clone(),
                    service_name: cfg.telemetry.service_name.clone(),
                    service_version: env!("CARGO_PKG_VERSION").into(),
                    sample_ratio: cfg.telemetry.otlp_sample_ratio,
                    headers,
                },
                crate::trace::TraceCounters {
                    exported: metrics.trace_exported.clone(),
                    dropped: metrics.trace_dropped.clone(),
                    failures: metrics.trace_failures.clone(),
                },
            )
        });
        let limiter = RateLimiter::new(cfg.limits.requests_per_second, cfg.limits.burst);
        let concurrency = Semaphore::new(cfg.limits.max_concurrent_requests);
        let replica_id = resolve_replica_id(&cfg);
        let local = Arc::new(Local::default());
        // The bus's origin must differ between processes even if two were misconfigured with one replica id.
        let bus = Bus::start(
            control.clone(),
            format!("{replica_id}.{}", random_hex()),
            local.clone(),
            BusMetrics {
                notices: metrics.cluster_notices.clone(),
                listener_up: metrics.cluster_listener_up.clone(),
                resyncs: metrics.cluster_resyncs.clone(),
            },
        );
        let (stored_schema, _) = control.schema_versions()?;
        metrics.control_schema_version.set(stored_schema);
        let backend = match control.kind() {
            crate::control::ControlKind::Sqlite => "sqlite",
            crate::control::ControlKind::Postgres => "postgres",
        };
        metrics.replica_info.with_label_values(&[replica_id.as_str(), env!("CARGO_PKG_VERSION"), backend]).set(1);
        tracing::info!(replica = %replica_id, control = backend, control_schema = stored_schema, "control plane ready");
        Ok(Self(Arc::new(AppState {
            cfg,
            clock,
            control,
            auth,
            repos,
            tracer,
            limiter,
            metrics,
            concurrency,
            started: Instant::now(),
            shutdown: tokio::sync::watch::channel(false).0,
            bindings: Mutex::new(None),
            local,
            job_rescan: Mutex::new(Instant::now()),
            bus,
            replica_id,
            draining: AtomicBool::new(false),
        })))
    }

    // ---- pipeline -----------------------------------------------------------

    /// Runs one operation through the pipeline: concurrency gate →
    /// authentication → deadline → metrics.
    pub async fn dispatch<T, Fut>(&self, ing: Ingress, op: &'static str, f: impl FnOnce(Call) -> Fut) -> ApiResult<T>
    where
        Fut: Future<Output = ApiResult<T>>,
    {
        use tracing::Instrument as _;
        let started = Instant::now();
        let span = self.tracer.as_ref().and_then(|t| t.begin(ing.traceparent.as_deref()));
        // Every log line written while serving this request — including the cause of an `internal` error,
        // which the response deliberately does not reveal — carries the request id the client was given.
        let log_span = tracing::info_span!("request", op, request_id = %ing.request_id, transport = ing.transport);
        let result =
            self.dispatch_inner(&ing, op, f).instrument(log_span).await.map_err(|e| e.with_request_id(&ing.request_id));
        let code = match &result {
            Ok(_) => "ok",
            Err(e) => e.code().as_str(),
        };
        if let (Some(tracer), Some(span)) = (&self.tracer, span) {
            let mut attrs = vec![
                ("agwp.operation", crate::trace::Attr::Str(op.into())),
                ("agwp.transport", crate::trace::Attr::Str(ing.transport.into())),
                ("agwp.result", crate::trace::Attr::Str(code.into())),
                ("agwp.request_id", crate::trace::Attr::Str(ing.request_id.clone())),
            ];
            if let Some(d) = ing.deadline_ms {
                attrs.push(("agwp.deadline_ms", crate::trace::Attr::Int(d.min(i64::MAX as u64) as i64)));
            }
            // Only server faults mark the span as failed; a caller's 404 or 409 is a normal answer.
            let server_fault = matches!(code, "internal" | "unavailable" | "deadline_exceeded" | "unimplemented");
            tracer.finish(span, op, attrs, server_fault.then(|| code.to_string()));
        }
        self.metrics.requests.with_label_values(&[ing.transport, op, code]).inc();
        self.metrics.latency.with_label_values(&[ing.transport, op]).observe(started.elapsed().as_secs_f64());
        if let Err(e) = &result {
            tracing::debug!(op, code, request_id = %ing.request_id, message = %e.0.message, "request failed");
        }
        result
    }

    async fn dispatch_inner<T, Fut>(&self, ing: &Ingress, op: &'static str, f: impl FnOnce(Call) -> Fut) -> ApiResult<T>
    where
        Fut: Future<Output = ApiResult<T>>,
    {
        let Ok(_permit) = self.concurrency.try_acquire() else {
            self.metrics.shed.inc();
            return Err(ApiError::new(ag_protocol::ErrorCode::Unavailable, "the server is saturated; retry shortly")
                .with_retry_after(250));
        };
        self.metrics.inflight.inc();
        let _guard = scopeguard(|| self.metrics.inflight.dec());

        let bearer = ing.bearer.as_deref().ok_or_else(|| {
            self.metrics.auth_failures.with_label_values(&["missing"]).inc();
            ApiError::new(ag_protocol::ErrorCode::Unauthenticated, "missing, invalid or expired credentials")
        })?;
        let principal = self.auth.authenticate(bearer).await.inspect_err(|_| {
            self.metrics.auth_failures.with_label_values(&["invalid"]).inc();
        })?;
        let deadline_ms = ing
            .deadline_ms
            .or((self.cfg.limits.default_deadline_ms > 0).then_some(self.cfg.limits.default_deadline_ms));
        let deadline = deadline_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
        let call = Call {
            principal: Arc::new(principal),
            run: ing.run.clone(),
            request_id: ing.request_id.clone(),
            op,
            transport: ing.transport,
            deadline,
        };
        // One token per request from the caller's bucket, for every operation — repository-scoped or not.
        self.rate_limit(&call)?;
        let fut = f(call);
        match deadline_ms {
            Some(ms) => tokio::time::timeout(Duration::from_millis(ms), fut).await.map_err(|_| {
                ApiError::new(
                    ag_protocol::ErrorCode::DeadlineExceeded,
                    "the deadline expired; the operation may still complete — ask for its outcome before retrying",
                )
            })?,
            None => fut.await,
        }
    }

    // ---- authorization ------------------------------------------------------------

    /// Role bindings, cached for at most `control.bindings_ttl_ms` (and dropped at once when this or another
    /// replica changes a binding).
    pub async fn bindings(&self) -> ApiResult<Arc<Vec<Binding>>> {
        let epoch = self.local.bindings_epoch.load(Ordering::Acquire);
        let ttl = Duration::from_millis(self.cfg.control.bindings_ttl_ms);
        if let Some((at, e, b)) = &*self.bindings.lock().unwrap_or_else(|e| e.into_inner())
            && *e == epoch
            && at.elapsed() < ttl
        {
            return Ok(b.clone());
        }
        let control = self.control.clone();
        let loaded =
            tokio::task::spawn_blocking(move || control.all_bindings()).await.map_err(ApiError::internal)??;
        let loaded = Arc::new(loaded);
        *self.bindings.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), epoch, loaded.clone()));
        Ok(loaded)
    }

    /// Call after changing role bindings so the change is effective at once — here, and on the other replicas.
    pub fn invalidate_bindings(&self) {
        self.local.invalidate_bindings();
        self.bus.publish(Signal::Bindings);
    }

    /// Checks `cap` on `repo` (and optionally `stream`) for the caller.
    pub async fn authorize(&self, call: &Call, repo: &str, cap: Cap, stream: Option<&str>) -> ApiResult<()> {
        let bindings = self.bindings().await?;
        Authorizer::check(&call.principal, &bindings, repo, cap, stream)
    }

    /// Checks a capability that is not tied to a repository.
    pub async fn authorize_global(&self, call: &Call, cap: Cap) -> ApiResult<()> {
        let bindings = self.bindings().await?;
        Authorizer::check_global(&call.principal, &bindings, cap)
    }

    /// Takes one token from the caller's bucket (`limits.requests_per_second`). Runs once per request, before
    /// the operation, so every operation is covered.
    pub fn rate_limit(&self, call: &Call) -> ApiResult<()> {
        let key = format!("cred:{}", call.principal.credential_id);
        self.limiter.check(&key, None).inspect_err(|_| self.metrics.rate_limited.inc())
    }

    /// A repository's own request quota, if it has one: a separate bucket per (caller, repository).
    pub fn repo_rate_limit(&self, call: &Call, repo: &str, quota: Option<&ag_protocol::v1::Quota>) -> ApiResult<()> {
        let Some(q) = quota.filter(|q| q.requests_per_second > 0) else { return Ok(()) };
        let key = format!("cred:{}|repo:{repo}", call.principal.credential_id);
        self.limiter
            .check(&key, Some((q.requests_per_second, q.burst)))
            .inspect_err(|_| self.metrics.rate_limited.inc())
    }

    // ---- repository access --------------------------------------------------------

    /// A repository's registry row (off the async threads: the control database may be a network service).
    pub async fn repo_row(&self, name: &str) -> ApiResult<Option<crate::control::RepoRow>> {
        let (control, name) = (self.control.clone(), name.to_string());
        tokio::task::spawn_blocking(move || control.repo(&name)).await.map_err(ApiError::internal)?
    }

    /// `stream` if given, otherwise the repository's default stream (for reads, where "the stream" is obvious).
    pub async fn stream_or_default(&self, repo: &str, stream: &str) -> ApiResult<String> {
        if !stream.is_empty() {
            return Ok(stream.to_string());
        }
        let row = self.repo_row(repo).await?;
        // An unknown repository is reported by the authorization that follows, without revealing anything.
        Ok(row.map(|r| r.default_stream).unwrap_or_else(|| self.cfg.repos.default_stream.clone()))
    }

    /// Authorizes, rate limits and runs `f` on a blocking thread with the
    /// repository and the effective acting identity.
    pub async fn with_repo<T, F>(&self, call: &Call, repo: &str, cap: Cap, stream: Option<&str>, f: F) -> ApiResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Repository, &Identity) -> ApiResult<T> + Send + 'static,
    {
        self.authorize(call, repo, cap, stream).await?;
        let row = self.repo_row(repo).await?;
        let quota = row.as_ref().map(|r| r.quota);
        self.repo_rate_limit(call, repo, quota.as_ref())?;
        let repos = self.0.clone();
        let name = repo.to_string();
        let call2 = call.clone();
        let log_span = tracing::Span::current();
        tokio::task::spawn_blocking(move || {
            let _in_request = log_span.enter();
            let handle = repos.repos.handle(&name)?;
            repos.metrics.open_repos.set(repos.repos.open_count() as i64);
            let actor = effective_actor(&call2, &handle)?;
            f(&handle, &actor)
        })
        .await
        .map_err(ApiError::internal)?
    }

    /// Like [`App::with_repo`] and wakes event subscribers afterwards.
    pub async fn with_repo_mut<T, F>(
        &self,
        call: &Call,
        repo: &str,
        cap: Cap,
        stream: Option<&str>,
        f: F,
    ) -> ApiResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Repository, &Identity) -> ApiResult<T> + Send + 'static,
    {
        let out = self.with_repo(call, repo, cap, stream, f).await;
        self.notify(repo);
        out
    }

    // ---- events -----------------------------------------------------------------------

    /// Tells long-lived streams (event subscriptions) and job long-polls to end.
    pub fn begin_shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    /// Tells the other replicas that `repo` may have claimable jobs (and wakes local claimers).
    pub(crate) fn announce_jobs(&self, repo: &str) {
        self.local.note_jobs(repo);
        self.bus.publish(Signal::Jobs(repo.to_string()));
    }

    /// Whether shutdown has begun.
    pub fn is_shutting_down(&self) -> bool {
        *self.shutdown.borrow()
    }

    /// Starts draining: `/readyz` reports "not ready" from now on, so a load balancer stops sending new
    /// requests while this replica keeps serving the ones it has.
    pub fn begin_drain(&self) {
        self.draining.store(true, Ordering::Release);
        self.metrics.draining.set(1);
    }

    /// Whether the replica is draining.
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::Acquire)
    }

    /// Why this replica should not receive new requests, if it should not: draining, or the control database
    /// does not answer within two seconds. (`/healthz` is the separate, database-free liveness check.)
    pub async fn not_ready_reason(&self) -> Option<&'static str> {
        if self.is_draining() {
            return Some("draining");
        }
        let control = self.control.clone();
        match tokio::time::timeout(Duration::from_secs(2), tokio::task::spawn_blocking(move || control.ping())).await {
            Ok(Ok(Ok(()))) => None,
            _ => Some("control database unavailable"),
        }
    }

    /// A receiver that reports `true` once shutdown began.
    pub fn shutdown_signal(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    /// The wake-up handle of a repository's event feed.
    pub fn notifier(&self, repo: &str) -> Arc<Notify> {
        self.local.notifier(repo)
    }

    /// Wakes subscribers of `repo`, on this replica and (best effort) on the others.
    pub fn notify(&self, repo: &str) {
        self.local.wake_repo(repo);
        self.bus.publish(Signal::Events(repo.to_string()));
    }
}

/// Resolves the identity requests act as: the principal itself, or for an
/// agent presenting `ag-run`, `agent:<name>#<run>` after checking that the run
/// exists, belongs to that agent and is still running.
fn effective_actor(call: &Call, repo: &Repository) -> ApiResult<Identity> {
    let Some(run_id) = call.run.as_deref().filter(|r| !r.is_empty()) else {
        return Ok(call.principal.identity.clone());
    };
    let base = call.principal.identity.principal();
    if base.kind() != IdentityKind::Agent {
        return Err(ApiError::invalid("only agent identities may act as a run"));
    }
    let run = repo
        .meta()
        .read(|tx| tx.run(run_id))?
        .ok_or_else(|| ApiError::not_found(format!("run '{run_id}' not found")))?;
    let agent_identity = format!("agent:{}", run.agent);
    if agent_identity != base.as_str() {
        return Err(ApiError::denied("this run belongs to a different agent"));
    }
    if run.status != "running" {
        return Err(ApiError::new(
            ag_protocol::ErrorCode::FailedPrecondition,
            format!("run '{run_id}' is {}", run.status),
        ));
    }
    Ok(Identity::new(format!("{}#{}", base.as_str(), run_id))?)
}

struct ScopeGuard<F: FnMut()>(F);

impl<F: FnMut()> Drop for ScopeGuard<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

fn scopeguard<F: FnMut()>(f: F) -> ScopeGuard<F> {
    ScopeGuard(f)
}
