//! Shared application state and the request pipeline.
//!
//! Both transports build an [`Ingress`] (credentials, deadline, trace id) and
//! call [`App::dispatch`], which authenticates, applies the default deadline,
//! tracks in-flight requests and records metrics around the operation.
//! Repository-scoped operations then go through [`App::with_repo`], which
//! authorizes, rate limits and runs the engine call on a blocking thread.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ag_integrate::Repository;
use ag_storage::{Clock, SystemClock};
use ag_types::{Identity, IdentityKind};
use tokio::sync::{Notify, Semaphore};

use crate::auth::{Authenticator, Principal};
use crate::authz::{Authorizer, Binding, Cap};
use crate::config::ServerConfig;
use crate::control::Control;
use crate::error::{ApiError, ApiResult};
use crate::limits::RateLimiter;
use crate::metrics::Metrics;
use crate::repos::RepoManager;

const BINDINGS_TTL: Duration = Duration::from_secs(5);

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
    /// Repositories that may have queued jobs (a hint, rebuilt by scanning).
    pub(crate) job_repos: Mutex<std::collections::BTreeSet<String>>,
    /// Wakes workers long-polling for jobs.
    pub(crate) job_notify: Notify,
    pub(crate) job_rescan: Mutex<Instant>,
    bindings_epoch: AtomicU64,
    notifiers: Mutex<HashMap<String, Arc<Notify>>>,
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
        let control = Arc::new(Control::open(&cfg.server.data_dir.join("control.db"), clock.clone())?);
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
            job_repos: Mutex::new(std::collections::BTreeSet::new()),
            job_notify: Notify::new(),
            job_rescan: Mutex::new(Instant::now()),
            bindings_epoch: AtomicU64::new(0),
            notifiers: Mutex::new(HashMap::new()),
        })))
    }

    // ---- pipeline -----------------------------------------------------------

    /// Runs one operation through the pipeline: concurrency gate →
    /// authentication → deadline → metrics.
    pub async fn dispatch<T, Fut>(&self, ing: Ingress, op: &'static str, f: impl FnOnce(Call) -> Fut) -> ApiResult<T>
    where
        Fut: Future<Output = ApiResult<T>>,
    {
        let started = Instant::now();
        let span = self.tracer.as_ref().and_then(|t| t.begin(ing.traceparent.as_deref()));
        let result = self.dispatch_inner(&ing, op, f).await.map_err(|e| e.with_request_id(&ing.request_id));
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

    /// Role bindings, cached briefly.
    pub async fn bindings(&self) -> ApiResult<Arc<Vec<Binding>>> {
        let epoch = self.bindings_epoch.load(Ordering::Acquire);
        if let Some((at, e, b)) = &*self.bindings.lock().unwrap_or_else(|e| e.into_inner())
            && *e == epoch
            && at.elapsed() < BINDINGS_TTL
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

    /// Call after changing role bindings so the change is effective at once.
    pub fn invalidate_bindings(&self) {
        self.bindings_epoch.fetch_add(1, Ordering::AcqRel);
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

    /// Authorizes, rate limits and runs `f` on a blocking thread with the
    /// repository and the effective acting identity.
    pub async fn with_repo<T, F>(&self, call: &Call, repo: &str, cap: Cap, stream: Option<&str>, f: F) -> ApiResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Repository, &Identity) -> ApiResult<T> + Send + 'static,
    {
        self.authorize(call, repo, cap, stream).await?;
        let row = {
            let control = self.control.clone();
            let name = repo.to_string();
            tokio::task::spawn_blocking(move || control.repo(&name)).await.map_err(ApiError::internal)??
        };
        let quota = row.as_ref().map(|r| r.quota);
        self.repo_rate_limit(call, repo, quota.as_ref())?;
        let repos = self.0.clone();
        let name = repo.to_string();
        let call2 = call.clone();
        tokio::task::spawn_blocking(move || {
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

    /// Tells long-lived streams (event subscriptions) to end.
    pub fn begin_shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    /// A receiver that reports `true` once shutdown began.
    pub fn shutdown_signal(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    /// The wake-up handle of a repository's event feed.
    pub fn notifier(&self, repo: &str) -> Arc<Notify> {
        self.notifiers.lock().unwrap_or_else(|e| e.into_inner()).entry(repo.to_string()).or_default().clone()
    }

    /// Wakes subscribers of `repo`.
    pub fn notify(&self, repo: &str) {
        if let Some(n) = self.notifiers.lock().unwrap_or_else(|e| e.into_inner()).get(repo) {
            n.notify_waiters();
        }
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
