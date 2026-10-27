//! Browser-driven migration from a self-contained Git bundle. No network credentials reach the server.
use crate::{
    app::{App, Call},
    authz::Cap,
    error::{ApiError, ApiResult},
    http::ingress_from_headers,
    routes::RouteMeta,
};
use ag_git::{MigrationOptions, MigrationReport};
use ag_integrate::{InitOptions, Repository};
use ag_types::{Identity, Namespace, StreamName};
use axum::{
    Router,
    body::to_bytes,
    extract::{Path, Request, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::path::{Path as FsPath, PathBuf};

const MAX_BUNDLE: usize = 128 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
struct Saved {
    name: String,
    owner: String,
    source_sha256: String,
    report: MigrationReport,
}
#[derive(Serialize)]
struct Plan {
    id: String,
    name: String,
    source_sha256: String,
    report: MigrationReport,
    state: &'static str,
}
#[derive(Deserialize)]
struct Target {
    name: String,
}
#[derive(Deserialize)]
struct Decision {
    #[serde(default)]
    accept_lossy: bool,
}
fn invalid(error: impl std::fmt::Display) -> ApiError {
    ApiError::invalid(error.to_string())
}
fn response(result: ApiResult<Plan>) -> Response {
    match result {
        Ok(plan) => axum::Json(plan).into_response(),
        Err(e) => e.into_response(),
    }
}
fn root(app: &App) -> PathBuf {
    app.cfg.server.data_dir.join("migrations")
}
fn lock(app: &App) -> ApiResult<File> {
    fs::create_dir_all(root(app)).map_err(ApiError::internal)?;
    let file = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(root(app).join("lock"))
        .map_err(ApiError::internal)?;
    file.try_lock().map_err(|_| {
        ApiError::new(ag_protocol::ErrorCode::RateLimited, "Another Git migration is running; retry shortly.")
    })?;
    Ok(file)
}
fn directory(app: &App, id: &str) -> ApiResult<PathBuf> {
    if !id.starts_with("import_") || id.len() != 71 || !id[7..].bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError::invalid("invalid migration id"));
    }
    Ok(root(app).join(id))
}
fn load(app: &App, id: &str) -> ApiResult<Saved> {
    let path = directory(app, id)?.join("plan.json");
    let bytes = fs::read(path).map_err(|_| ApiError::not_found("Migration not found. Upload the bundle again."))?;
    serde_json::from_slice(&bytes).map_err(ApiError::internal)
}
async fn owned(app: &App, call: &Call, id: &str) -> ApiResult<Saved> {
    let saved = load(app, id)?;
    if saved.owner != call.principal.identity.as_str() {
        return Err(ApiError::not_found("Migration not found"));
    }
    app.authorize(call, &saved.name, Cap::RepoCreate, None).await?;
    Ok(saved)
}
fn plan(app: &App, id: String, saved: Saved) -> ApiResult<Plan> {
    let state = if app.control.repo(&saved.name)?.is_some() {
        let marker = app.cfg.server.data_dir.join("repos").join(&saved.name).join(".ag/import/id");
        if fs::read_to_string(marker).ok().as_deref() != Some(&id) {
            return Err(ApiError::new(
                ag_protocol::ErrorCode::AlreadyExists,
                "Another operation created this destination. Choose a different name.",
            ));
        }
        "imported"
    } else {
        "ready"
    };
    Ok(Plan { id, name: saved.name, source_sha256: saved.source_sha256, report: saved.report, state })
}
fn write_plan(path: &FsPath, saved: &Saved) -> ApiResult<()> {
    let bytes = serde_json::to_vec_pretty(saved).map_err(ApiError::internal)?;
    fs::write(path.join("plan.tmp"), bytes).map_err(ApiError::internal)?;
    fs::rename(path.join("plan.tmp"), path.join("plan.json")).map_err(ApiError::internal)
}
fn clean_incomplete(path: &FsPath) -> ApiResult<()> {
    for name in ["mirror.git", "repository"] {
        let dir = path.join(name);
        if dir.exists() {
            fs::remove_dir_all(dir).map_err(ApiError::internal)?;
        }
    }
    Ok(())
}
// Unpublished attempts are temporary. The immutable archive of a published import lives in its repository.
fn prune_entry(entry: &fs::DirEntry) -> ApiResult<bool> {
    let age = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
    if age.is_some_and(|d| d.as_secs() > 86400) {
        fs::remove_dir_all(entry.path()).map_err(ApiError::internal)?;
        return Ok(false);
    }
    Ok(entry.path().join("repository/.ag").is_dir())
}
fn prune_staging(app: &App, current: &str) -> ApiResult<()> {
    let mut pending = 0;
    for entry in fs::read_dir(root(app)).map_err(ApiError::internal)? {
        let entry = entry.map_err(ApiError::internal)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == current || !name.starts_with("import_") || !entry.path().is_dir() {
            continue;
        }
        pending += usize::from(prune_entry(&entry)?);
    }
    if pending >= 8 {
        return Err(ApiError::new(
            ag_protocol::ErrorCode::RateLimited,
            "Eight imports are pending review. Finish them or retry after their 24-hour expiry.",
        ));
    }
    Ok(())
}
fn verify_import(repo: &Repository) -> ApiResult<()> {
    if !repo.fsck().map_err(invalid)?.is_ok() {
        return Err(ApiError::invalid("The migrated repository failed verification"));
    }
    Ok(())
}
fn check_import(repo: &Repository, report: &MigrationReport, quota: ag_protocol::v1::Quota) -> ApiResult<()> {
    if quota.max_streams > 0 && report.branches.len() > quota.max_streams as usize {
        return Err(ApiError::invalid("The migration exceeds this server's stream quota"));
    }
    let objects = repo.objects().list().map_err(invalid)?;
    let total = objects
        .iter()
        .try_fold(0u64, |sum, o| sum.checked_add(o.size))
        .ok_or_else(|| ApiError::invalid("Repository size overflow"))?;
    if quota.max_total_object_bytes > 0 && total > quota.max_total_object_bytes {
        return Err(ApiError::invalid("The migration exceeds the repository's storage quota"));
    }
    if quota.max_object_bytes > 0 && objects.iter().any(|o| o.size > quota.max_object_bytes) {
        return Err(ApiError::invalid("A migrated object exceeds the repository's object limit"));
    }

    Ok(())
}
fn preserve_source(path: &FsPath, id: &str, report: &MigrationReport) -> ApiResult<()> {
    let imported = path.join("repository").join(".ag/import");
    fs::create_dir_all(&imported).map_err(ApiError::internal)?;
    fs::copy(path.join("source.bundle"), imported.join("source.bundle")).map_err(ApiError::internal)?;
    fs::write(imported.join("id"), id).map_err(ApiError::internal)?;
    fs::write(imported.join("report.json"), serde_json::to_vec_pretty(&report).map_err(ApiError::internal)?)
        .map_err(ApiError::internal)?;
    Ok(())
}
fn convert_bundle(path: &FsPath, name: &str, owner: &Identity) -> ApiResult<(Repository, MigrationReport)> {
    let mirror = path.join("mirror.git");
    ag_git::unpack_bundle(&path.join("source.bundle"), &mirror).map_err(invalid)?;
    let default = ag_git::source_default_branch(&mirror).map_err(invalid)?;
    let staged = path.join("repository");
    let repo = Repository::init(
        &staged,
        &InitOptions {
            namespace: Namespace::new(name).map_err(invalid)?,
            default_stream: StreamName::new(default).map_err(invalid)?,
            actor: owner.clone(),
            storage: None,
        },
    )
    .map_err(invalid)?;
    let report = ag_git::migrate(&repo, &MigrationOptions { git_dir: mirror, actor: owner.clone(), allow_lossy: true })
        .map_err(invalid)?;
    Ok((repo, report))
}
fn previous_plan(app: &App, id: &str, name: &str, owner: &Identity, digest: &str) -> ApiResult<Option<Plan>> {
    if let Ok(saved) = load(app, id) {
        if saved.name != name || saved.owner != owner.as_str() || saved.source_sha256 != digest {
            return Err(ApiError::invalid("This migration key was already used for different input."));
        }
        return plan(app, id.into(), saved).map(Some);
    }
    if app.control.repo(name)?.is_some() {
        return Err(ApiError::new(ag_protocol::ErrorCode::AlreadyExists, "Destination repository already exists"));
    }
    Ok(None)
}
fn stage_upload(app: &App, id: &str, path: &FsPath, bytes: &[u8]) -> ApiResult<()> {
    prune_staging(app, id)?;
    fs::create_dir_all(path).map_err(ApiError::internal)?;
    clean_incomplete(path)?;
    fs::write(path.join("source.bundle"), bytes).map_err(ApiError::internal)
}
fn save_result(app: &App, id: &str, path: &FsPath, saved: Saved) -> ApiResult<Plan> {
    write_plan(path, &saved)?;
    plan(app, id.into(), saved)
}
fn prepare(app: &App, id: &str, name: &str, owner: &Identity, bytes: Vec<u8>) -> ApiResult<Plan> {
    let path = directory(app, id)?;
    let digest = hex::encode(Sha256::digest(&bytes));
    if let Some(existing) = previous_plan(app, id, name, owner, &digest)? {
        return Ok(existing);
    }
    stage_upload(app, id, &path, &bytes)?;
    let (repo, report) = convert_bundle(&path, name, owner)?;
    check_import(&repo, &report, app.default_quota())?;
    verify_import(&repo)?;
    preserve_source(&path, id, &report)?;
    let saved = Saved { name: name.into(), owner: owner.to_string(), source_sha256: digest, report };
    save_result(app, id, &path, saved)
}
async fn upload(State(app): State<App>, headers: HeaderMap, request: Request) -> Response {
    let query: Result<Target, _> = serde_urlencoded::from_str(request.uri().query().unwrap_or(""));
    let target = match query {
        Ok(t) => t,
        Err(e) => return invalid(e).into_response(),
    };
    let ing = ingress_from_headers(&headers, None);
    let key = headers
        .get("idempotency-key")
        .and_then(|h| h.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| ing.request_id.clone());
    let a = app.clone();
    response(
        app.dispatch(ing, "migration.prepare", move |call| async move {
            crate::repos::parse_repo_name(&target.name)?;
            a.authorize(&call, &target.name, Cap::RepoCreate, None).await?;
            if a.cfg.repos.storage.is_some() {
                return Err(ApiError::invalid(
                    "Browser Git migration currently requires local SQLite/filesystem storage.",
                ));
            }
            let guard = lock(&a)?;
            let bytes = to_bytes(request.into_body(), MAX_BUNDLE)
                .await
                .map_err(|_| ApiError::invalid("Git bundle exceeds 128 MiB or upload was interrupted"))?;
            let id = format!(
                "import_{}",
                hex::encode(Sha256::digest(format!("{}\0{}\0{}", call.principal.identity, target.name, key)))
            );
            let owner = call.principal.identity.clone();
            tokio::task::spawn_blocking(move || {
                let _guard = guard;
                prepare(&a, &id, &target.name, &owner, bytes.to_vec())
            })
            .await
            .map_err(ApiError::internal)?
        })
        .await,
    )
}
async fn status(State(app): State<App>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    let ing = ingress_from_headers(&headers, None);
    let a = app.clone();
    response(
        app.dispatch(ing, "migration.status", move |call| async move {
            let saved = owned(&a, &call, &id).await?;
            plan(&a, id, saved)
        })
        .await,
    )
}
fn publish(app: &App, id: &str, saved: Saved, owner: &Identity) -> ApiResult<Plan> {
    let _guard = lock(app)?;
    let path = directory(app, id)?;
    app.repos.publish_migration(&path.join("repository"), &saved.name, id, owner, app.default_quota())?;
    for name in ["mirror.git", "source.bundle"] {
        let p = path.join(name);
        if p.is_dir() {
            let _ = fs::remove_dir_all(p);
        } else {
            let _ = fs::remove_file(p);
        }
    }
    plan(app, id.into(), saved)
}
async fn commit(State(app): State<App>, Path(id): Path<String>, headers: HeaderMap, request: Request) -> Response {
    let ing = ingress_from_headers(&headers, None);
    let a = app.clone();
    response(
        app.dispatch(ing, "migration.commit", move |call| async move {
            let saved = owned(&a, &call, &id).await?;
            let bytes = to_bytes(request.into_body(), 1024).await.map_err(invalid)?;
            let decision: Decision = serde_json::from_slice(&bytes).map_err(invalid)?;
            if saved.report.needs_attention() && !decision.accept_lossy {
                return Err(ApiError::invalid(
                    "Review and explicitly accept the reported migration limitations first.",
                ));
            }
            let owner = call.principal.identity.clone();
            tokio::task::spawn_blocking(move || publish(&a, &id, saved, &owner)).await.map_err(ApiError::internal)?
        })
        .await,
    )
}
/// Add the bounded upload, preview and commit routes to the common authenticated router.
pub fn add_routes(router: &mut Router<App>, meta: &mut Vec<RouteMeta>) {
    *router = std::mem::take(router)
        .route("/v1/migrations/git", put(upload))
        .route("/v1/migrations/{id}", get(status))
        .route("/v1/migrations/{id}/commit", post(commit));
    for (method, path, op, summary) in [
        (
            "put",
            "/v1/migrations/git",
            "migration.prepare",
            "Upload a Git bundle and preflight all branches/tags (128 MiB maximum; admin only)",
        ),
        ("get", "/v1/migrations/{id}", "migration.status", "Read a private migration report"),
        (
            "post",
            "/v1/migrations/{id}/commit",
            "migration.commit",
            "Publish a preflighted repository; body: accept_lossy boolean",
        ),
    ] {
        let mut route = RouteMeta::raw(method, path, op, "Migration", summary).response_body("application/json");
        if method == "put" {
            route =
                route.query("name", "string", "New destination, owner/name").request_body("application/octet-stream");
        }
        if method == "post" {
            route = route.request_body("application/json");
        }
        meta.push(route);
    }
}
