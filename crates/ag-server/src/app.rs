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
    /// Request rate limiting.
    pub limiter: RateLimiter,
    /// Prometheus metrics.
    pub metrics: Metrics,
    /// Bounds concurrent requests; excess is shed.
    pub concurrency: Semaphore,
    /// When the process started.
    pub started: Instant,
    bindings: Mutex<Option<BindingsCache>>,
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
        );
        let limiter = RateLimiter::new(cfg.limits.requests_per_second, cfg.limits.burst);
        let concurrency = Semaphore::new(cfg.limits.max_concurrent_requests);
        Ok(Self(Arc::new(AppState {
            cfg,
            clock,
            control,
            auth,
            repos,
            limiter,
            metrics: Metrics::new(),
            concurrency,
            started: Instant::now(),
            bindings: Mutex::new(None),
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
        let result = self.dispatch_inner(&ing, op, f).await.map_err(|e| e.with_request_id(&ing.request_id));
        let code = match &result {
            Ok(_) => "ok",
            Err(e) => e.code().as_str(),
        };
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

    /// Rate limits the caller (and the repository's own quota).
    pub fn rate_limit(&self, call: &Call, quota: Option<&ag_protocol::v1::Quota>) -> ApiResult<()> {
        let key = format!("cred:{}", call.principal.credential_id);
        let over = quota.filter(|q| q.requests_per_second > 0).map(|q| (q.requests_per_second, q.burst));
        self.limiter.check(&key, over).inspect_err(|_| self.metrics.rate_limited.inc())
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
        self.rate_limit(call, quota.as_ref())?;
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
