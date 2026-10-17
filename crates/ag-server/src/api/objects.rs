//! Objects: upload, download, presence checks, verification, publication.

use ag_changeset::Changeset;
use ag_object::{ObjectKind, RawObject};
use ag_protocol::v1;
use ag_storage::ObjectStatus;
use ag_types::{ChangesetId, ObjectId};

use super::{convert, parse_id, require};
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::{ApiError, ApiResult};

/// Accumulates a stream of object chunks and stores each completed object.
pub struct ObjectSink {
    app: App,
    call: Call,
    repo: String,
    current: Option<(String, Vec<u8>)>,
    stored: Vec<String>,
    present: Vec<String>,
    bytes: u64,
}

impl ObjectSink {
    /// Feeds one chunk. A chunk with `last` set completes the current object.
    pub async fn push(&mut self, chunk: v1::ObjectChunk) -> ApiResult<()> {
        if chunk.object_id.is_empty() {
            return Err(ApiError::invalid("object chunk without object_id"));
        }
        match &mut self.current {
            Some((id, buf)) if *id == chunk.object_id => buf.extend_from_slice(&chunk.data),
            Some((id, _)) => {
                return Err(ApiError::invalid(format!(
                    "object {id} was not finished before {} began",
                    chunk.object_id
                )));
            }
            None => self.current = Some((chunk.object_id.clone(), chunk.data.clone())),
        }
        let max = self.app.cfg.limits.max_object_bytes;
        if self.current.as_ref().is_some_and(|(_, b)| b.len() as u64 > max) {
            return Err(ApiError::new(
                ag_protocol::ErrorCode::TooLarge,
                format!("object {} exceeds the limit of {max} bytes", chunk.object_id),
            ));
        }
        if chunk.last {
            let (id, buf) = self.current.take().expect("set above");
            let len = buf.len() as u64;
            let newly = self.app.store_object(&self.call, &self.repo, &id, buf).await?;
            self.bytes += len;
            if newly { self.stored.push(id) } else { self.present.push(id) }
        }
        Ok(())
    }

    /// Ends the stream; every object must have been completed.
    pub fn finish(self) -> ApiResult<v1::UploadObjectsResponse> {
        if let Some((id, _)) = self.current {
            return Err(ApiError::invalid(format!("the stream ended in the middle of object {id}")));
        }
        Ok(v1::UploadObjectsResponse { stored: self.stored, already_present: self.present, bytes: self.bytes })
    }
}

impl App {
    /// Starts an upload session (authorizes once for the whole stream).
    pub async fn upload_sink(&self, call: &Call, repo: &str) -> ApiResult<ObjectSink> {
        let repo = require(repo, "repo")?.to_string();
        self.authorize(call, &repo, Cap::ObjectsWrite, None).await?;
        Ok(ObjectSink {
            app: self.clone(),
            call: call.clone(),
            repo,
            current: None,
            stored: vec![],
            present: vec![],
            bytes: 0,
        })
    }

    /// Stores one object envelope after verifying that it hashes to `expected_id`.
    /// Returns whether the object was new.
    pub async fn store_object(&self, call: &Call, repo: &str, expected_id: &str, envelope: Vec<u8>) -> ApiResult<bool> {
        let expected: ObjectId = parse_id(expected_id, "object id")?;
        let size = envelope.len() as u64;
        let quota = self.control.repo(repo)?.map(|r| (r.quota.max_total_object_bytes, r.object_bytes));
        let control = self.control.clone();
        let name = repo.to_string();
        let newly = self
            .with_repo_mut(call, repo, Cap::ObjectsWrite, None, move |r, _| {
                let raw = RawObject::decode_envelope(&envelope)
                    .map_err(|e| ApiError::invalid(format!("bad object envelope: {e}")))?;
                raw.verify(&expected).map_err(|_| {
                    ApiError::new(
                        ag_protocol::ErrorCode::ObjectCorrupt,
                        format!("the uploaded bytes do not hash to {expected}"),
                    )
                })?;
                let existed = r.objects().contains(&expected)?;
                if !existed
                    && let Some((max_total, used)) = quota
                    && max_total > 0
                    && used + size > max_total
                {
                    return Err(ApiError::new(
                        ag_protocol::ErrorCode::QuotaExceeded,
                        format!("this repository's object quota of {max_total} bytes would be exceeded"),
                    ));
                }
                match raw.kind {
                    ObjectKind::Blob => {
                        r.objects().put(&raw)?;
                    }
                    ObjectKind::Changeset => {
                        let cs = Changeset::from_raw(&raw, r.limits()).map_err(|e| ApiError::invalid(e.to_string()))?;
                        r.put_changeset(&cs)?;
                    }
                    ObjectKind::Tree => {
                        return Err(ApiError::invalid("trees are derived by the server and cannot be uploaded"));
                    }
                }
                if !existed {
                    control.add_object_bytes(&name, size as i64)?;
                }
                Ok(!existed)
            })
            .await?;
        self.metrics.object_bytes_in.inc_by(size);
        Ok(newly)
    }

    /// The stored envelope of an object (verified on read).
    pub async fn get_object(&self, call: &Call, req: v1::GetObjectRequest) -> ApiResult<Vec<u8>> {
        let id: ObjectId = parse_id(&req.object_id, "object id")?;
        let bytes = self
            .with_repo(call, require(&req.repo, "repo")?, Cap::ObjectsRead, None, move |r, _| {
                let raw = r.objects().get(&id)?.ok_or_else(|| ApiError::not_found(format!("object {id} not found")))?;
                Ok(raw.encode_envelope())
            })
            .await?;
        self.metrics.object_bytes_out.inc_by(bytes.len() as u64);
        Ok(bytes)
    }

    /// Which of the given objects does the repository not have?
    pub async fn missing_objects(
        &self,
        call: &Call,
        req: v1::MissingObjectsRequest,
    ) -> ApiResult<v1::MissingObjectsResponse> {
        if req.object_ids.len() > ag_protocol::limits::MAX_BATCH as usize {
            return Err(ApiError::new(ag_protocol::ErrorCode::TooLarge, "too many object ids in one request"));
        }
        let ids = req.object_ids.iter().map(|s| parse_id::<ObjectId>(s, "object id")).collect::<ApiResult<Vec<_>>>()?;
        self.with_repo(call, require(&req.repo, "repo")?, Cap::ObjectsRead, None, move |r, _| {
            let mut missing = Vec::new();
            for id in ids {
                if !r.objects().contains(&id)? {
                    missing.push(id.to_string());
                }
            }
            Ok(v1::MissingObjectsResponse { missing })
        })
        .await
    }

    /// Re-reads and hashes the given objects.
    pub async fn verify_objects(
        &self,
        call: &Call,
        req: v1::VerifyObjectsRequest,
    ) -> ApiResult<v1::VerifyObjectsResponse> {
        if req.object_ids.len() > 1000 {
            return Err(ApiError::new(ag_protocol::ErrorCode::TooLarge, "verify at most 1000 objects per request"));
        }
        let ids = req.object_ids.iter().map(|s| parse_id::<ObjectId>(s, "object id")).collect::<ApiResult<Vec<_>>>()?;
        self.with_repo(call, require(&req.repo, "repo")?, Cap::ObjectsRead, None, move |r, _| {
            let mut objects = Vec::new();
            for id in ids {
                let (status, size, detail) = match r.objects().verify(&id)? {
                    ObjectStatus::Missing => ("missing", 0, String::new()),
                    ObjectStatus::Ok { size } => ("ok", size, String::new()),
                    ObjectStatus::Corrupt(why) => ("corrupt", 0, why),
                };
                objects.push(v1::ObjectStatus { object_id: id.to_string(), status: status.into(), size, detail });
            }
            Ok(v1::VerifyObjectsResponse { objects })
        })
        .await
    }

    /// Publishes a head and its unpublished ancestors (idempotent).
    pub async fn publish(&self, call: &Call, req: v1::PublishRequest) -> ApiResult<v1::Publication> {
        let head: ChangesetId = parse_id(&req.head, "changeset id")?;
        let key = (!req.idempotency_key.is_empty()).then(|| req.idempotency_key.clone());
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::Publish, None, move |r, actor| {
            let res = r.publish(&head, actor, key.as_deref())?;
            Ok(v1::Publication {
                operation_id: res.operation_id,
                state: res.state.to_string(),
                head: res.head.to_string(),
                new_changesets: res.new_changesets as u32,
                new_blobs: res.new_blobs as u32,
                reason: res.reason.unwrap_or_default(),
            })
        })
        .await
    }

    /// The recorded outcome of an operation, by operation id or idempotency key.
    pub async fn get_operation(&self, call: &Call, req: v1::GetOperationRequest) -> ApiResult<v1::Operation> {
        self.with_repo(call, require(&req.repo, "repo")?, Cap::OperationRead, None, move |r, _| {
            Ok(convert::operation(&r.operation_row(&req.operation_id, &req.idempotency_key)?))
        })
        .await
    }
}
