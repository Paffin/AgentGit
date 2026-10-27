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
use crate::quota::{Budget, MeterGuard, MeteredStore, ObjectMeter};
use crate::repos::RepoManager;

/// The identity under which the server's own periodic run reaper (`agents.reap_interval_secs`) is audited.
pub const REAPER_IDENTITY: &str = "system:reaper";
/// The control-database lease that makes the periodic run reaper run on one replica at a time.
pub const RUN_REAPER_LEASE: &str = "run-reaper";

/// The deadline given to long-lived streams (event subscriptions): effectively none. It is a marker, not a time
/// anyone waits for, and it is left out of the trace attributes.
pub const NO_DEADLINE_MS: u64 = u64::MAX / 4;

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
    /// W3C `traceparent`: continues the caller's trace (exported with OTLP) and puts its trace id on the
    /// request's log lines.
    pub traceparent: Option<String>,
    /// Address of the caller: the peer of the connection or, behind a trusted proxy (`server.trusted_proxies`), the
    /// client that proxy reports. Appears on the request's log lines, including failed authentications, and keys
    /// the failed-authentication limit. [`App::dispatch`] resolves it from the raw peer.
    pub remote: Option<String>,
    /// The `Forwarded` header, read only when the peer is a trusted proxy.
    pub forwarded: Option<String>,
    /// The `X-Forwarded-For` header, read only when the peer is a trusted proxy.
    pub x_forwarded_for: Option<String>,
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
    /// Failed authentications per client address (`limits.auth_failures_per_second`).
    pub auth_failures: RateLimiter,
    /// Proxies whose `Forwarded` / `X-Forwarded-For` headers are believed (`server.trusted_proxies`).
    pub proxies: crate::proxy::TrustedProxies,
    /// Prometheus metrics.
    pub metrics: Metrics,
    /// What the last health scan found (`repos.health_scan_interval_secs`).
    pub(crate) health: Mutex<Option<crate::health::Health>>,
    /// Object bytes held by uploads in progress (`limits.max_inflight_object_bytes`).
    pub inflight: Arc<crate::inflight::InflightBytes>,
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
    /// Names this process (not just this replica id) as the holder of background-work leases in the control database.
    pub lease_holder: String,
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

/// The rate-limit bucket of a caller: its credential - except for OIDC sign-ins, where the "credential" is a
/// short-lived token whose id changes with every login, so the bucket belongs to the identity instead (signing in
/// again must not hand out a fresh burst).
fn bucket_key(p: &Principal) -> String {
    match p.method {
        crate::auth::AuthMethod::Oidc => format!("id:{}", p.identity.principal()),
        _ => format!("cred:{}", p.credential_id),
    }
}

/// Parses `name=value,name2=value2` (the `OTEL_EXPORTER_OTLP_HEADERS` format). An entry that cannot be used is
/// reported by position and name - never by value, which may be a secret - and skipped.
fn parse_headers(s: &str) -> Vec<(String, String)> {
    s.split(',')
        .enumerate()
        .filter_map(|(i, kv)| {
            let parsed = kv.split_once('=').and_then(|(k, v)| {
                let (k, v) = (k.trim(), v.trim());
                (!k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') && !v.is_empty())
                    .then(|| (k.to_ascii_lowercase(), v.to_string()))
            });
            if parsed.is_none() && !kv.trim().is_empty() {
                let name = kv.split_once('=').map(|(k, _)| k.trim()).unwrap_or("(no '=')");
                tracing::warn!(entry = i, name, "ignoring a malformed entry of the OTLP headers variable");
            }
            parsed
        })
        .collect()
}

/// Creates the data directory. When this call creates it, it is `0700` on Unix: the control database in it holds the
/// hash of every credential. A directory that already exists is left as it is, with a warning when users outside the
/// owner's group can enter it.
fn prepare_data_dir(dir: &std::path::Path) -> std::io::Result<()> {
    if !dir.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(dir)?;
    }
    if let Some(why) = data_dir_exposure(dir) {
        tracing::warn!("{why}");
    }
    Ok(())
}

/// Why `dir` is too open, if it is: any access for "other" users (Unix permission bits). Group access is not
/// flagged, because a shared group on a volume (Kubernetes `fsGroup`) is a deliberate setup.
fn data_dir_exposure(dir: &std::path::Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir).ok()?.permissions().mode() & 0o777;
        if mode & 0o007 != 0 {
            return Some(format!(
                "the data directory {} can be entered by every user of this host (mode {mode:03o}); it holds the control \
                 database with the hash of every credential: chmod 700 it (750 for a shared group)",
                dir.display()
            ));
        }
    }
    let _ = dir;
    None
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
        prepare_data_dir(&cfg.server.data_dir).map_err(ApiError::internal)?;
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
            crate::repos::HostedOptions {
                trees: ag_storage::DecodedTrees::new(usize::try_from(cfg.repos.tree_cache_bytes).unwrap_or(usize::MAX)),
                contention: ag_integrate::ContentionPolicy {
                    attempts: cfg.repos.integrate_attempts,
                    max_backoff_ms: cfg.repos.integrate_backoff_ms,
                },
            },
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
        let auth_failures = RateLimiter::new(cfg.limits.auth_failures_per_second, cfg.limits.auth_failure_burst);
        let proxies = crate::proxy::TrustedProxies::parse(&cfg.server.trusted_proxies)
            .map_err(|e| ApiError::internal(format!("server.trusted_proxies: {e}")))?;
        let concurrency = Semaphore::new(cfg.limits.max_concurrent_requests);
        let inflight = crate::inflight::InflightBytes::new(
            cfg.limits.max_inflight_object_bytes,
            metrics.inflight_object_bytes.clone(),
            metrics.object_budget_refused.clone(),
        );
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
            auth_failures,
            proxies,
            metrics,
            inflight,
            health: Mutex::new(None),
            concurrency,
            started: Instant::now(),
            shutdown: tokio::sync::watch::channel(false).0,
            bindings: Mutex::new(None),
            local,
            job_rescan: Mutex::new(Instant::now()),
            bus,
            lease_holder: format!("{replica_id}.{}", random_hex()),
            replica_id,
            draining: AtomicBool::new(false),
        })))
    }

    // ---- pipeline -----------------------------------------------------------

    /// Runs one operation through the pipeline: concurrency gate →
    /// authentication → deadline → metrics.
    pub async fn dispatch<T, Fut>(
        &self,
        mut ing: Ingress,
        op: &'static str,
        f: impl FnOnce(Call) -> Fut,
    ) -> ApiResult<T>
    where
        Fut: Future<Output = ApiResult<T>>,
    {
        use tracing::Instrument as _;
        // The caller's address: the peer, or what a trusted proxy says (forwarding headers from anyone else are
        // ignored). From here on `remote` is the one address used in logs and limits.
        if let Some(peer) = ing.remote.as_deref().and_then(|r| r.parse().ok()) {
            ing.remote = self.proxies.client(Some(peer), ing.forwarded.as_deref(), ing.x_forwarded_for.as_deref());
        }
        let started = Instant::now();
        let span = self.tracer.as_ref().and_then(|t| t.begin(ing.traceparent.as_deref()));
        // Every log line written while serving this request — including the cause of an `internal` error,
        // which the response deliberately does not reveal — carries the request id the client was given.
        let log_span = tracing::info_span!(
            "request",
            op,
            request_id = %ing.request_id,
            transport = ing.transport,
            remote = ing.remote.as_deref().unwrap_or("-"),
            trace_id = tracing::field::Empty,
        );
        // The trace id this request belongs to: the one the exporter assigned, else the caller's own.
        let trace_id = match &span {
            Some(ctx) => Some(hex::encode(ctx.trace_id)),
            None => ing.traceparent.as_deref().and_then(crate::trace::trace_id_of),
        };
        if let Some(id) = &trace_id {
            log_span.record("trace_id", id.as_str());
        }
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
            if let Some(d) = ing.deadline_ms.filter(|d| *d < NO_DEADLINE_MS) {
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

        // A client address that keeps failing to authenticate is turned away before its credential is looked at
        // (`limits.auth_failures_per_second`); the key is the host of `remote`, so behind a trusted proxy it is the
        // client, not the proxy, that is counted.
        let failure_key = ing.remote.as_deref().map(|r| format!("ip:{}", crate::proxy::host_of(r)));
        if let Some(key) = &failure_key {
            self.auth_failures.peek(key).inspect_err(|_| {
                self.metrics.auth_failures.with_label_values(&["throttled"]).inc();
                tracing::warn!(remote = ing.remote.as_deref().unwrap_or("-"), op, "too many failed authentications");
            })?;
        }
        let bearer = ing.bearer.as_deref().ok_or_else(|| {
            if let Some(key) = &failure_key {
                self.auth_failures.spend(key);
            }
            self.metrics.auth_failures.with_label_values(&["missing"]).inc();
            tracing::warn!(remote = ing.remote.as_deref().unwrap_or("-"), op, "request without credentials");
            ApiError::new(ag_protocol::ErrorCode::Unauthenticated, "missing, invalid or expired credentials")
        })?;
        let principal = self.auth.authenticate(bearer).await.inspect_err(|e| {
            if let Some(key) = &failure_key {
                self.auth_failures.spend(key);
            }
            self.metrics.auth_failures.with_label_values(&["invalid"]).inc();
            tracing::warn!(
                remote = ing.remote.as_deref().unwrap_or("-"),
                op,
                code = e.code().as_str(),
                "authentication failed"
            );
        })?;
        // A credential bound to a run is good for as long as the run is running (ADR-0010): checked for every
        // operation, before it does anything, so that no code path has to remember to.
        self.ensure_bound_run_alive(&principal).await.inspect_err(|e| {
            self.metrics.auth_failures.with_label_values(&["run_ended"]).inc();
            tracing::warn!(
                remote = ing.remote.as_deref().unwrap_or("-"),
                op,
                credential = %principal.credential_id,
                code = e.code().as_str(),
                "a run-bound credential was refused"
            );
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

    // ---- run-bound credentials -------------------------------------------------------

    /// For a credential bound to a run: the run must still be running and must belong to the credential's
    /// agent. A finished, failed or abandoned run (or one that no longer exists) ends the credential, whether or
    /// not it was revoked yet; the answer is `unauthenticated`, as for any dead credential. Other credentials
    /// pass without a lookup.
    pub async fn ensure_bound_run_alive(&self, principal: &Principal) -> ApiResult<()> {
        let Some((repo, run_id)) = principal.scope.bound_run() else { return Ok(()) };
        let (repos, repo, run_id) = (self.0.clone(), repo.to_string(), run_id.to_string());
        let agent = principal.identity.principal();
        tokio::task::spawn_blocking(move || {
            let ended = || {
                ApiError::new(
                    ag_protocol::ErrorCode::Unauthenticated,
                    "the run this credential is bound to is no longer running, so the credential is no longer valid",
                )
            };
            let handle = match repos.repos.handle(&repo) {
                Ok(h) => h,
                Err(e) if e.code() == ag_protocol::ErrorCode::NotFound => return Err(ended()),
                Err(e) => return Err(e),
            };
            match handle.meta().read(|tx| tx.run(&run_id))? {
                Some(run) if run.status == "running" && format!("agent:{}", run.agent) == agent.as_str() => Ok(()),
                _ => Err(ended()),
            }
        })
        .await
        .map_err(ApiError::internal)?
    }

    /// Revokes the credentials bound to a run that has ended; returns how many. A fault here is logged and
    /// counted as zero: the credentials are dead anyway ([`App::ensure_bound_run_alive`]), this keeps their
    /// listing and any open event stream honest.
    pub async fn revoke_run_credentials(&self, repo: &str, run_id: &str) -> u64 {
        let (control, repo, run_id, now) =
            (self.control.clone(), repo.to_string(), run_id.to_string(), self.clock.now().as_millis());
        let revoked = tokio::task::spawn_blocking(move || {
            control.revoke_run_credentials(&repo, &run_id, now).inspect_err(|e| {
                tracing::error!(%repo, %run_id, error = %e.0.message, "could not revoke the credentials of an ended run")
            })
        })
        .await;
        match revoked {
            Ok(Ok(n)) => n,
            _ => 0,
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
        self.limiter.check(&bucket_key(&call.principal), None).inspect_err(|_| self.metrics.rate_limited.inc())
    }

    /// A repository's own request quota, if it has one: a separate bucket per (caller, repository).
    pub fn repo_rate_limit(&self, call: &Call, repo: &str, quota: Option<&ag_protocol::v1::Quota>) -> ApiResult<()> {
        let Some(q) = quota.filter(|q| q.requests_per_second > 0) else { return Ok(()) };
        let key = format!("{}|repo:{repo}", bucket_key(&call.principal));
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
        let budget = row.as_ref().map(Budget::of);
        let repos = self.0.clone();
        let name = repo.to_string();
        let call2 = call.clone();
        let log_span = tracing::Span::current();
        tokio::task::spawn_blocking(move || {
            let _in_request = log_span.enter();
            let handle = repos.repos.handle(&name)?;
            repos.metrics.open_repos.set(repos.repos.open_count() as i64);
            let actor = effective_actor(&call2, &handle)?;
            // Whatever the call writes into the repository — an upload, a workspace edit, a merge — is counted
            // against the repository's quota by the view's object store, not by each endpoint.
            let Some(budget) = budget else { return f(&handle, &actor) };
            let meter = ObjectMeter::new(repos.control.clone(), budget);
            let _settle = MeterGuard(meter.clone());
            let view = handle.with_object_store(|inner| Arc::new(MeteredStore::new(inner, meter.clone())));
            f(&view, &actor).map_err(|e| meter.violation().unwrap_or(e))
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
///
/// A credential bound to a run (ADR-0010) always acts as that run - with or without `ag-run` - and as no other:
/// naming another run is refused, so two runs of one agent cannot act for each other.
fn effective_actor(call: &Call, repo: &Repository) -> ApiResult<Identity> {
    let bound = call.principal.scope.bound_run().map(|(_, run)| run);
    let requested = call.run.as_deref().filter(|r| !r.is_empty());
    if let (Some(bound), Some(asked)) = (bound, requested)
        && bound != asked
    {
        return Err(ApiError::denied(format!(
            "this credential is bound to run '{bound}'; it may not act as run '{asked}'"
        )));
    }
    let Some(run_id) = bound.or(requested) else {
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
        // The end of a bound run is the end of its credential; `ensure_bound_run_alive` normally answers first.
        let code = if bound.is_some() {
            ag_protocol::ErrorCode::Unauthenticated
        } else {
            ag_protocol::ErrorCode::FailedPrecondition
        };
        return Err(ApiError::new(code, format!("run '{run_id}' is {}", run.status)));
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

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    const TOKEN: &str = "unit-test-bootstrap-token-0123456789";

    #[cfg(unix)]
    #[test]
    fn the_data_directory_is_created_private_and_an_open_one_is_called_out() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let base = tempfile::tempdir().unwrap();

        // created by the server: 0700, parents included, nothing to warn about
        let fresh = base.path().join("a/b/data");
        prepare_data_dir(&fresh).unwrap();
        assert_eq!(mode(&fresh), 0o700);
        assert_eq!(data_dir_exposure(&fresh), None);

        // an existing directory is not touched; "other" access is what gets the warning
        let existing = base.path().join("existing");
        std::fs::create_dir(&existing).unwrap();
        for (bits, flagged) in [(0o755, true), (0o701, true), (0o770, false), (0o750, false), (0o700, false)] {
            std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(bits)).unwrap();
            prepare_data_dir(&existing).unwrap();
            assert_eq!(mode(&existing), bits, "the directory is left as the operator made it");
            let why = data_dir_exposure(&existing);
            assert_eq!(why.is_some(), flagged, "{bits:o}: {why:?}");
            if let Some(why) = why {
                assert!(why.contains(&format!("{bits:03o}")) && why.contains("chmod 700"), "{why}");
            }
        }
        // a path that cannot be created is an error, not a panic
        let file = base.path().join("a-file");
        std::fs::write(&file, b"x").unwrap();
        assert!(prepare_data_dir(&file.join("below-a-file")).is_err());
    }

    /// Collects what the `tracing` subscriber writes.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    std::thread_local! {
        static LOG_SINK: std::cell::RefCell<Option<Captured>> = const { std::cell::RefCell::new(None) };
    }

    struct CaptureGuard(Option<Captured>);

    impl Drop for CaptureGuard {
        fn drop(&mut self) {
            LOG_SINK.with(|sink| sink.replace(self.0.take()));
        }
    }

    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Captured {
        fn install(&self) -> CaptureGuard {
            // Scoped subscribers race with other tests through tracing's global interest cache. Keep one
            // subscriber alive for the entire test process and route its output to the calling thread.
            static INSTALL: std::sync::Once = std::sync::Once::new();
            INSTALL.call_once(|| {
                let subscriber = tracing_subscriber::fmt()
                    .with_writer(|| LOG_SINK.with(|sink| sink.borrow().clone().unwrap_or_default()))
                    .with_ansi(false)
                    .with_max_level(tracing::Level::INFO)
                    .finish();
                tracing::subscriber::set_global_default(subscriber).expect("install test log subscriber");
            });
            CaptureGuard(LOG_SINK.with(|sink| sink.replace(Some(self.clone()))))
        }

        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    fn app(dir: &tempfile::TempDir) -> App {
        let mut cfg = ServerConfig::default();
        cfg.server.data_dir = dir.path().to_path_buf();
        App::with_options(cfg, Arc::new(SystemClock), Some(TOKEN.to_string())).unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn log_lines_of_a_request_carry_its_trace_id_and_the_peer_address() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(&dir);
        let captured = Captured::default();
        let _installed = captured.install();

        let ing = Ingress {
            transport: "http",
            bearer: Some(TOKEN.into()),
            request_id: "req_unit".into(),
            // Not sampled, no exporter configured: the trace id still reaches the log.
            traceparent: Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-00".into()),
            remote: Some("203.0.113.9:4711".into()),
            ..Ingress::default()
        };
        app.dispatch(ing, "test.op", |_call| async {
            tracing::info!("inside the operation");
            Ok(())
        })
        .await
        .unwrap();
        let line = captured.text();
        assert!(line.contains("inside the operation"), "{line}");
        assert!(line.contains(r#"trace_id="0af7651916cd43dd8448eb211c80319c""#), "{line}");
        assert!(line.contains(r#"remote="203.0.113.9:4711""#), "{line}");
        assert!(line.contains("request_id=req_unit"), "{line}");

        // A failed authentication says where it came from.
        let bad = Ingress {
            transport: "grpc",
            bearer: Some("not-a-credential".into()),
            request_id: "req_bad".into(),
            remote: Some("198.51.100.7:1".into()),
            ..Ingress::default()
        };
        assert!(app.dispatch(bad, "test.op", |_call| async { Ok(()) }).await.is_err());
        let log = captured.text();
        let failure = log.lines().find(|l| l.contains("authentication failed")).expect(&log);
        assert!(failure.contains(r#"remote="198.51.100.7:1""#) && failure.contains("unauthenticated"), "{failure}");
        let missing = Ingress { remote: Some("198.51.100.8:2".into()), ..Ingress::default() };
        assert!(app.dispatch(missing, "test.op", |_call| async { Ok(()) }).await.is_err());
        assert!(captured.text().contains("198.51.100.8:2"));
    }

    fn proxied_app(dir: &tempfile::TempDir, trusted: &[&str], per_second: u32, burst: u32) -> App {
        let mut cfg = ServerConfig::default();
        cfg.server.data_dir = dir.path().to_path_buf();
        cfg.server.trusted_proxies = trusted.iter().map(|s| s.to_string()).collect();
        cfg.limits.auth_failures_per_second = per_second;
        cfg.limits.auth_failure_burst = burst;
        App::with_options(cfg, Arc::new(SystemClock), Some(TOKEN.to_string())).unwrap()
    }

    fn bad_login(peer: &str, forwarded_for: &str) -> Ingress {
        Ingress {
            transport: "http",
            bearer: Some("not-a-credential".into()),
            remote: Some(peer.into()),
            x_forwarded_for: (!forwarded_for.is_empty()).then(|| forwarded_for.into()),
            ..Ingress::default()
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn behind_a_trusted_proxy_the_client_address_is_logged_and_anyone_elses_header_is_not_believed() {
        let dir = tempfile::tempdir().unwrap();
        let app = proxied_app(&dir, &["10.0.0.0/8"], 0, 20);
        let captured = Captured::default();
        let _installed = captured.install();

        // From the proxy: the client it names.
        assert!(app.dispatch(bad_login("10.0.0.5:3333", "198.51.100.7"), "t", |_c| async { Ok(()) }).await.is_err());
        let log = captured.text();
        let line = log.lines().find(|l| l.contains("authentication failed")).expect(&log);
        assert!(line.contains(r#"remote="198.51.100.7""#), "{line}");
        // From anyone else: the header is a claim, the peer is the fact.
        assert!(app.dispatch(bad_login("203.0.113.9:4711", "198.51.100.7"), "t", |_c| async { Ok(()) }).await.is_err());
        let log = captured.text();
        assert!(log.contains(r#"remote="203.0.113.9:4711""#), "{log}");
        assert!(
            log.lines().filter(|l| l.contains("203.0.113.9")).all(|l| !l.contains("198.51.100.7")),
            "the spoofed address must not appear on the lines of the direct peer: {log}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn failed_logins_are_counted_per_client_address_and_the_header_decides_only_behind_a_trusted_proxy() {
        let dir = tempfile::tempdir().unwrap();
        // The proxy 10.0.0.5 is trusted: two clients behind it have a bucket each (burst 2, almost no refill).
        let app = proxied_app(&dir, &["10.0.0.5"], 1, 2);
        let fail = |peer: &'static str, xff: &'static str| {
            let app = app.clone();
            async move { app.dispatch(bad_login(peer, xff), "t", |_c| async { Ok(()) }).await.unwrap_err() }
        };
        for _ in 0..2 {
            assert_eq!(fail("10.0.0.5:1", "198.51.100.1").await.code(), ag_protocol::ErrorCode::Unauthenticated);
        }
        let throttled = fail("10.0.0.5:1", "198.51.100.1").await;
        assert_eq!(throttled.code(), ag_protocol::ErrorCode::RateLimited);
        assert!(throttled.0.retry_after_ms > 0, "{throttled:?}");
        // A second client behind the same proxy is unaffected.
        assert_eq!(fail("10.0.0.5:2", "198.51.100.2").await.code(), ag_protocol::ErrorCode::Unauthenticated);
        // A good credential from the throttled address is refused too: it is turned away before it is looked at.
        let good = Ingress {
            transport: "http",
            bearer: Some(TOKEN.into()),
            remote: Some("10.0.0.5:1".into()),
            x_forwarded_for: Some("198.51.100.1".into()),
            ..Ingress::default()
        };
        assert_eq!(
            app.dispatch(good, "t", |_c| async { Ok(()) }).await.unwrap_err().code(),
            ag_protocol::ErrorCode::RateLimited
        );

        // Nobody trusted: changing the header does not buy a new bucket - the peer is counted.
        let app2 = proxied_app(&dir, &[], 1, 2);
        let mut codes = Vec::new();
        for n in 0..4 {
            let xff = format!("198.51.100.{n}");
            let err = app2.dispatch(bad_login("203.0.113.9:4711", &xff), "t", |_c| async { Ok(()) }).await.unwrap_err();
            codes.push(err.code());
        }
        use ag_protocol::ErrorCode::{RateLimited, Unauthenticated};
        assert_eq!(codes, [Unauthenticated, Unauthenticated, RateLimited, RateLimited]);
        // Another peer, the same header: its own bucket.
        let other = app2.dispatch(bad_login("203.0.113.10:1", "198.51.100.0"), "t", |_c| async { Ok(()) }).await;
        assert_eq!(other.unwrap_err().code(), Unauthenticated);
    }

    #[test]
    fn oidc_sign_ins_share_one_rate_limit_bucket_per_identity() {
        let mut token = Principal::for_tests("user:dana");
        token.credential_id = "cred_1".into();
        let mut other = token.clone();
        other.credential_id = "cred_2".into();
        assert_ne!(bucket_key(&token), bucket_key(&other), "tokens of ours are limited one by one");
        token.method = crate::auth::AuthMethod::Oidc;
        other.method = crate::auth::AuthMethod::Oidc;
        assert_eq!(bucket_key(&token), bucket_key(&other), "a new OIDC login is not a new bucket");
        other.identity = ag_types::Identity::new("user:erin").unwrap();
        assert_ne!(bucket_key(&token), bucket_key(&other));
    }

    #[test]
    fn collector_headers_keep_the_good_entries_and_report_the_bad_ones_by_name_only() {
        let captured = Captured::default();
        let _installed = captured.install();
        let parsed = parse_headers("Authorization=Bearer abc, bad name=secret-value,=nothing,noequals,x-key=v");
        assert_eq!(
            parsed,
            vec![("authorization".to_string(), "Bearer abc".to_string()), ("x-key".to_string(), "v".to_string())]
        );
        let log = captured.text();
        assert_eq!(log.matches("malformed entry").count(), 3, "{log}");
        assert!(log.contains("bad name"), "the name of a bad entry is reported: {log}");
        assert!(!log.contains("secret-value") && !log.contains("nothing"), "values never are: {log}");
    }
}
