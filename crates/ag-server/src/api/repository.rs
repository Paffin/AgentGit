//! Repository operations: create, describe, list, configure.

use ag_protocol::v1;
use sha2::{Digest, Sha256};

use super::{page_size, require};
use crate::app::{App, Call};
use crate::authz::{Authorizer, Cap};
use crate::control::RepoRow;
use crate::error::{ApiError, ApiResult};
use crate::repos::parse_repo_name;

impl App {
    /// The quota a new repository starts with: the `[limits]` defaults. (Create, `ag-server admin create-repo`
    /// and the API all use this one.)
    pub fn default_quota(&self) -> v1::Quota {
        let l = &self.cfg.limits;
        v1::Quota {
            max_total_object_bytes: l.max_total_object_bytes,
            max_object_bytes: l.repo_max_object_bytes,
            max_streams: l.repo_max_streams,
            max_open_proposals: l.max_open_proposals,
            requests_per_second: l.repo_requests_per_second,
            burst: l.repo_burst,
        }
    }

    /// The wire form of a registry row. The storage backends come from the repository's own stored
    /// configuration, so create and list report what describe reports; the counts need the repository's metadata
    /// ([`App::fill_counts`]) and the durability text the open object store (describe only).
    fn repo_to_wire(&self, r: &RepoRow) -> v1::Repo {
        let (meta_backend, object_backend) = self.repos.backends_of(r);
        v1::Repo {
            name: r.name.clone(),
            namespace: r.namespace.clone(),
            default_stream: r.default_stream.clone(),
            description: r.description.clone(),
            created_at_ms: r.created_at_ms,
            created_by: r.created_by.clone(),
            meta_backend: meta_backend.into(),
            object_backend: object_backend.into(),
            durability: String::new(),
            stream_count: 0,
            changeset_count: 0,
            quota: Some(r.quota),
            counts_error: String::new(),
        }
    }

    /// Fills `stream_count` and `changeset_count` from the repository's own metadata (blocking). A repository
    /// that cannot be opened or read keeps both at 0 and says so in `counts_error` - what went wrong is in the
    /// server log, not in the response, which any reader of the repository sees.
    fn fill_counts(&self, wire: &mut v1::Repo) {
        let counted = self.repos.handle(&wire.name).map_err(|e| e.0.message).and_then(|h| {
            h.meta().read(|tx| Ok((tx.count_streams()?, tx.count_changesets()?))).map_err(|e| {
                tracing::error!(repo = %wire.name, error = %e, "cannot count the streams and changesets");
                "the repository's metadata could not be read (see the server log)".to_string()
            })
        });
        match counted {
            Ok((streams, changesets)) => {
                wire.stream_count = u32::try_from(streams).unwrap_or(u32::MAX);
                wire.changeset_count = changesets;
                wire.counts_error.clear();
            }
            Err(why) => {
                wire.stream_count = 0;
                wire.changeset_count = 0;
                wire.counts_error = why;
            }
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
            let mut wire = app.repo_to_wire(&row);
            app.fill_counts(&mut wire);
            app.control.record_idempotent(
                &scoped_key,
                &request_hash,
                &serde_json::to_string(&wire).map_err(ApiError::internal)?,
            )?;
            Ok(row)
        })
        .await
        .map_err(ApiError::internal)??;
        // The answer is read from the repository now, whether this call created it or an earlier one with the
        // same key did.
        let app = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut wire = app.repo_to_wire(&row);
            app.fill_counts(&mut wire);
            wire
        })
        .await
        .map_err(ApiError::internal)
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
        let mut wire = self.repo_to_wire(&row);
        let (streams, changesets, durability, backends) = self
            .with_repo(call, &name, Cap::RepoRead, None, |repo, _| {
                let (streams, changesets) = repo.meta().read(|tx| Ok((tx.count_streams()?, tx.count_changesets()?)))?;
                let streams = u32::try_from(streams).unwrap_or(u32::MAX);
                let d = repo.objects().durability();
                let backends = crate::repos::config_backends(repo.config());
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
        let app = self.clone();
        let (repos, next) = tokio::task::spawn_blocking(move || -> ApiResult<(Vec<v1::Repo>, String)> {
            let mut out: Vec<RepoRow> = Vec::new();
            let mut cursor = after;
            loop {
                let page = control.repos_page(&prefix, &cursor, 200)?;
                for r in page.rows {
                    if Authorizer::check(&principal, &bindings, &r.name, Cap::RepoRead, None).is_ok() {
                        out.push(r);
                        if out.len() > size {
                            break;
                        }
                    }
                }
                // Resume after the last name the page returned, including rows that were skipped.
                match page.last {
                    Some(last) if out.len() <= size && !page.exhausted => cursor = last,
                    _ => break,
                }
            }
            let next = if out.len() > size {
                out.truncate(size);
                out.last().map(|r| base64_encode(&r.name)).unwrap_or_default()
            } else {
                String::new()
            };
            // One metadata read per repository of the page - the price of exact counts, bounded by the page size.
            let wires = out
                .iter()
                .map(|r| {
                    let mut w = app.repo_to_wire(r);
                    app.fill_counts(&mut w);
                    w
                })
                .collect();
            Ok((wires, next))
        })
        .await
        .map_err(ApiError::internal)??;
        Ok(v1::ListReposResponse { repos, next_page_token: next })
    }

    /// Changes repository settings.
    pub async fn configure_repo(&self, call: &Call, req: v1::ConfigureRepoRequest) -> ApiResult<v1::Repo> {
        let name = require(&req.repo, "repo")?.to_string();
        self.authorize(call, &name, Cap::RepoConfigure, None).await?;
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
