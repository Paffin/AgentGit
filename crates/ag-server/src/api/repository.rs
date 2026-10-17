//! Repository operations: create, describe, list, configure.

use ag_protocol::v1;
use sha2::{Digest, Sha256};

use super::{page_size, require};
use crate::app::{App, Call};
use crate::authz::{Authorizer, Cap};
use crate::control::RepoRow;
use crate::error::{ApiError, ApiResult};
use crate::repos::parse_repo_name;

fn row_to_wire(r: &RepoRow) -> v1::Repo {
    v1::Repo {
        name: r.name.clone(),
        namespace: r.namespace.clone(),
        default_stream: r.default_stream.clone(),
        description: r.description.clone(),
        created_at_ms: r.created_at_ms,
        created_by: r.created_by.clone(),
        meta_backend: "sqlite".into(),
        object_backend: "fs".into(),
        durability: String::new(),
        stream_count: 0,
        changeset_count: 0,
        quota: Some(r.quota),
    }
}

impl App {
    fn default_quota(&self) -> v1::Quota {
        v1::Quota {
            max_total_object_bytes: self.cfg.limits.max_total_object_bytes,
            max_object_bytes: 0,
            max_streams: 0,
            max_open_proposals: self.cfg.limits.max_open_proposals,
            requests_per_second: 0,
            burst: 0,
        }
    }

    /// Creates a repository.
    pub async fn create_repo(&self, call: &Call, req: v1::CreateRepoRequest) -> ApiResult<v1::Repo> {
        let name = require(&req.name, "name")?.to_string();
        parse_repo_name(&name)?;
        // A caller without any role on the new name must not learn whether it exists.
        self.authorize(call, &name, Cap::RepoCreate, None).await.map_err(|e| {
            if e.code() == ag_protocol::ErrorCode::NotFound {
                ApiError::denied(format!("this identity may not create repositories named '{name}'"))
            } else {
                e
            }
        })?;
        self.rate_limit(call, None)?;
        let stream = if req.default_stream.is_empty() {
            self.cfg.repos.default_stream.clone()
        } else {
            req.default_stream.clone()
        };
        let request_hash = hex::encode(Sha256::digest(format!("create_repo\0{name}\0{stream}\0{}", req.description)));
        let key = req.idempotency_key.clone();
        let app = self.clone();
        let actor = call.principal.identity.clone();
        let quota = self.default_quota();
        let description = req.description.clone();
        let row = tokio::task::spawn_blocking(move || -> ApiResult<RepoRow> {
            let scoped_key = if key.is_empty() { String::new() } else { format!("repo.create:{key}") };
            if let Some(prev) = app.control.idempotent(&scoped_key, &request_hash)? {
                let stored: v1::Repo = serde_json::from_str(&prev).map_err(ApiError::internal)?;
                if let Some(row) = app.control.repo(&stored.name)? {
                    return Ok(row);
                }
            }
            let row = app.repos.create(&name, &stream, &description, &actor, quota)?;
            app.control.record_idempotent(
                &scoped_key,
                &request_hash,
                &serde_json::to_string(&row_to_wire(&row)).map_err(ApiError::internal)?,
            )?;
            Ok(row)
        })
        .await
        .map_err(ApiError::internal)??;
        Ok(row_to_wire(&row))
    }

    /// Describes a repository.
    pub async fn describe_repo(&self, call: &Call, req: v1::GetRepoRequest) -> ApiResult<v1::Repo> {
        let name = require(&req.repo, "repo")?.to_string();
        let row = {
            let control = self.control.clone();
            let n = name.clone();
            self.authorize(call, &name, Cap::RepoRead, None).await?;
            tokio::task::spawn_blocking(move || control.repo(&n)).await.map_err(ApiError::internal)??
        }
        .ok_or_else(|| ApiError::not_found(format!("repository '{name}' not found")))?;
        let mut wire = row_to_wire(&row);
        let (streams, changesets, durability, backends) = self
            .with_repo(call, &name, Cap::RepoRead, None, |repo, _| {
                let streams = repo.meta().read(|tx| tx.list_streams())?.len() as u32;
                let changesets = repo.meta().read(|tx| tx.count_changesets())?;
                let d = repo.objects().durability();
                let storage = repo.config().storage();
                let backends = (
                    if matches!(storage.meta, ag_integrate::MetaConfig::Postgres { .. }) {
                        "postgres"
                    } else {
                        "sqlite"
                    },
                    if matches!(storage.objects, ag_integrate::ObjectsConfig::S3 { .. }) { "s3" } else { "fs" },
                );
                Ok((
                    streams,
                    changesets,
                    format!(
                        "{}: survives {}; does not survive {}",
                        d.name,
                        d.survives.join(", "),
                        d.does_not_survive.join(", ")
                    ),
                    backends,
                ))
            })
            .await?;
        wire.stream_count = streams;
        wire.changeset_count = changesets;
        wire.durability = durability;
        wire.meta_backend = backends.0.into();
        wire.object_backend = backends.1.into();
        Ok(wire)
    }

    /// Lists the repositories the caller may read.
    pub async fn list_repos(&self, call: &Call, req: v1::ListReposRequest) -> ApiResult<v1::ListReposResponse> {
        let bindings = self.bindings().await?;
        let size = page_size(req.page_size);
        let after = if req.page_token.is_empty() { String::new() } else { base64_decode(&req.page_token)? };
        let control = self.control.clone();
        let prefix = req.prefix.clone();
        let principal = call.principal.clone();
        let (repos, next) = tokio::task::spawn_blocking(move || -> ApiResult<(Vec<RepoRow>, String)> {
            let mut out: Vec<RepoRow> = Vec::new();
            let mut cursor = after;
            loop {
                let batch = control.repos(&prefix, &cursor, 200)?;
                let exhausted = batch.len() < 200;
                for r in batch {
                    cursor = r.name.clone();
                    if Authorizer::check(&principal, &bindings, &r.name, Cap::RepoRead, None).is_ok() {
                        out.push(r);
                        if out.len() > size {
                            break;
                        }
                    }
                }
                if out.len() > size || exhausted {
                    break;
                }
            }
            let next = if out.len() > size {
                out.truncate(size);
                out.last().map(|r| base64_encode(&r.name)).unwrap_or_default()
            } else {
                String::new()
            };
            Ok((out, next))
        })
        .await
        .map_err(ApiError::internal)??;
        Ok(v1::ListReposResponse { repos: repos.iter().map(row_to_wire).collect(), next_page_token: next })
    }

    /// Changes repository settings.
    pub async fn configure_repo(&self, call: &Call, req: v1::ConfigureRepoRequest) -> ApiResult<v1::Repo> {
        let name = require(&req.repo, "repo")?.to_string();
        self.authorize(call, &name, Cap::RepoConfigure, None).await?;
        self.rate_limit(call, None)?;
        let control = self.control.clone();
        let n = name.clone();
        let (description, quota) = (req.description.clone(), req.quota);
        tokio::task::spawn_blocking(move || -> ApiResult<()> {
            if control.repo(&n)?.is_none() {
                return Err(ApiError::not_found(format!("repository '{n}' not found")));
            }
            control.update_repo(&n, description.as_deref(), quota.as_ref())
        })
        .await
        .map_err(ApiError::internal)??;
        self.describe_repo(call, v1::GetRepoRequest { repo: name }).await
    }
}

fn base64_encode(s: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s)
}

fn base64_decode(s: &str) -> ApiResult<String> {
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| ApiError::invalid("invalid page_token"))?;
    String::from_utf8(raw).map_err(|_| ApiError::invalid("invalid page_token"))
}
