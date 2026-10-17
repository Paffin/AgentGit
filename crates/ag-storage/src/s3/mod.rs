//! An S3-compatible [`ObjectStore`] (Amazon S3, MinIO, Ceph RGW, Cloudflare R2,
//! …) over a blocking HTTP client, with hand-rolled AWS Signature V4.
//!
//! # Layout and guarantees
//!
//! Objects live at `<prefix>/objects/<2 hex>/<62 hex>`, the same fan-out as the
//! filesystem store, holding the same envelope. The content id is recomputed on
//! **every** read, so a damaged or tampered object is an error, never data.
//!
//! * **`put` is idempotent and self-healing.** It creates the object with a
//!   conditional `PUT` (`If-None-Match: *`). If one exists (HTTP 412) it is read
//!   back and verified; a healthy one has its age refreshed with a server-side
//!   `CopyObject` onto itself (so `LastModified` moves and a garbage collector
//!   keeps it), a damaged one is overwritten. Services that ignore the
//!   conditional header just overwrite identical bytes, which is equivalent.
//! * **Garbage collection is race-free** through an [`ObjectLock`]: `put` holds
//!   it shared, `delete_if_older_than` holds it exclusive across its whole
//!   check-then-delete. S3 itself offers no such atomicity.
//! * **Transient failures are retried** with exponential backoff (connection
//!   errors, timeouts, 408/409/429/5xx); anything else that is not a success is
//!   an error. A `put` that succeeded but whose answer was lost is simply
//!   repeated and ends in the 412 path.
//! * **Size.** Objects are sent in a single request (no multipart upload), so
//!   the largest object is bounded by the blob limit of the repository (256 MiB
//!   by default; S3 allows 5 GiB per single `PUT`) and is held in memory while
//!   it is transferred.
//! * **Time.** An object's age is the server's `LastModified`, with the
//!   resolution the service provides (one second on Amazon S3). Garbage
//!   collection grace periods are hours, so clock differences between this
//!   machine and the service do not matter in practice.

mod http;
mod sigv4;
#[cfg(test)]
mod tests;
mod xml;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ag_object::RawObject;
use ag_types::{Limits, ObjectId, Timestamp};

use self::http::{Client, Request};
pub use self::sigv4::Credentials;
use crate::error::StoreError;
use crate::lock::{LockGuard, ObjectLock};
use crate::object_store::{Durability, ObjectInfo, ObjectStatus, ObjectStore, check_size};

/// Where and how to reach the bucket.
///
/// Credentials live only in memory here; a repository's configuration file
/// names the environment variables to read them from and never contains them.
#[derive(Clone, Debug)]
pub struct S3Config {
    /// Service endpoint, e.g. `https://s3.eu-west-1.amazonaws.com` or `http://127.0.0.1:9000`.
    pub endpoint: String,
    /// Signing region (`us-east-1` for most S3-compatible servers).
    pub region: String,
    /// Bucket name. The bucket must already exist.
    pub bucket: String,
    /// Key prefix: several repositories can share one bucket by using different prefixes.
    pub prefix: String,
    /// `true`: `https://host/bucket/key`; `false`: `https://bucket.host/key`.
    pub path_style: bool,
    /// Access credentials.
    pub credentials: Credentials,
    /// PEM file with the CA certificate(s) to trust instead of the built-in web roots
    /// (private CAs, self-signed test servers).
    pub ca_bundle: Option<PathBuf>,
    /// Limits applied to objects (the blob size limit).
    pub limits: Limits,
    /// Retries after the first attempt for transient failures.
    pub max_retries: u32,
    /// Deadline of one request, including transferring its body.
    pub request_timeout: Duration,
    /// Keys requested per `ListObjectsV2` page (the service may return fewer).
    pub list_page_size: u32,
}

impl S3Config {
    /// A configuration with defaults: no prefix, 5 retries, 5 minute request deadline,
    /// path-style addressing unless the endpoint is Amazon's.
    pub fn new(
        endpoint: impl Into<String>,
        region: impl Into<String>,
        bucket: impl Into<String>,
        credentials: Credentials,
    ) -> Self {
        let endpoint = endpoint.into();
        let amazon = endpoint.contains("amazonaws.com");
        Self {
            endpoint,
            region: region.into(),
            bucket: bucket.into(),
            prefix: String::new(),
            path_style: !amazon,
            credentials,
            ca_bundle: None,
            limits: Limits::default(),
            max_retries: 5,
            request_timeout: Duration::from_secs(300),
            list_page_size: 1000,
        }
    }
}

type Hook = Arc<dyn Fn(&str) + Send + Sync>;

/// The S3 object store. See the [module documentation](self).
pub struct S3ObjectStore {
    client: Client,
    prefix: String,
    bucket: String,
    limits: Limits,
    lock: Arc<dyn ObjectLock>,
    page_size: u32,
    hook: Mutex<Option<Hook>>,
}

enum Created {
    Yes,
    AlreadyThere,
}

fn remote(message: impl Into<String>) -> StoreError {
    StoreError::Remote { status: None, message: message.into() }
}

impl S3ObjectStore {
    /// Connects, checking that the bucket is reachable with these credentials.
    ///
    /// `lock` must span every process that writes to or collects garbage from
    /// this prefix; see [`crate::lock`].
    pub fn open(cfg: S3Config, lock: Arc<dyn ObjectLock>) -> Result<Self, StoreError> {
        let prefix = cfg.prefix.trim_matches('/').to_string();
        if prefix.contains("//") || prefix.chars().any(char::is_control) || prefix.len() > 512 {
            return Err(remote(format!("invalid S3 key prefix '{}'", cfg.prefix)));
        }
        let prefix = if prefix.is_empty() { prefix } else { format!("{prefix}/") };
        let store = Self {
            client: Client::new(&cfg)?,
            prefix,
            bucket: cfg.bucket.clone(),
            limits: cfg.limits,
            lock,
            page_size: cfg.list_page_size.clamp(1, 1000),
            hook: Mutex::new(None),
        };
        // Fail now, with an actionable message, if the endpoint, bucket or credentials are wrong.
        let probe = Request::new("GET", None)
            .query("list-type", "2")
            .query("max-keys", "1")
            .query("prefix", store.objects_prefix());
        store.client.send("open", &probe, &[])?;
        Ok(store)
    }

    fn objects_prefix(&self) -> String {
        format!("{}objects/", self.prefix)
    }

    fn key_of(&self, id: &ObjectId) -> String {
        let hex = id.to_hex();
        format!("{}{}/{}", self.objects_prefix(), &hex[..2], &hex[2..])
    }

    fn id_of(&self, key: &str) -> Option<ObjectId> {
        let rest = key.strip_prefix(&self.objects_prefix())?;
        let (shard, tail) = rest.split_once('/')?;
        if shard.len() != 2 || tail.len() != 62 {
            return None;
        }
        format!("{shard}{tail}").parse().ok()
    }

    fn fire(&self, point: &str) {
        let hook = self.hook.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(h) = hook {
            h(point);
        }
    }

    /// Installs a callback invoked at named points of `put` (`put.locked`) and
    /// `delete_if_older_than` (`delete.checked`: after the age check, before the
    /// delete). Lets a test hold one operation at the exact moment a racing one
    /// would interleave. Never used outside tests.
    #[doc(hidden)]
    pub fn set_test_hook(&self, hook: Option<Hook>) {
        *self.hook.lock().unwrap_or_else(|e| e.into_inner()) = hook;
    }

    /// Overwrites the stored bytes of an object with `bytes` (damage simulation). Tests only.
    #[doc(hidden)]
    pub fn corrupt_for_test(&self, id: &ObjectId, bytes: &[u8]) -> Result<(), StoreError> {
        self.overwrite(&self.key_of(id), bytes)
    }

    /// Deletes every object under this store's prefix. Test clean-up only.
    #[doc(hidden)]
    pub fn purge_for_test(&self) -> Result<usize, StoreError> {
        let all = self.list()?;
        for o in &all {
            self.client.send("delete", &Request::new("DELETE", Some(&self.key_of(&o.id))), &[404])?;
        }
        Ok(all.len())
    }

    fn decode(id: &ObjectId, bytes: &[u8]) -> Result<RawObject, StoreError> {
        let obj =
            RawObject::decode_envelope(bytes).map_err(|e| StoreError::Corrupt { id: *id, reason: e.to_string() })?;
        obj.verify(id).map_err(|e| StoreError::Corrupt { id: *id, reason: e.to_string() })?;
        Ok(obj)
    }

    fn get_bytes(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let r = self.client.send("GET", &Request::new("GET", Some(key)), &[404])?;
        Ok((r.status != 404).then_some(r.body))
    }

    fn put_if_absent(&self, key: &str, envelope: &[u8]) -> Result<Created, StoreError> {
        let req = Request::new("PUT", Some(key))
            .header("content-type", "application/octet-stream")
            .header("if-none-match", "*")
            .body(envelope);
        match self.client.send("PUT", &req, &[412]) {
            Ok(r) if r.status == 412 => Ok(Created::AlreadyThere),
            Ok(_) => Ok(Created::Yes),
            // A service without conditional writes: look first, then write. A race between the
            // look and the write only ever writes identical bytes.
            Err(StoreError::Remote { status: Some(501), .. }) => {
                let head = self.client.send("HEAD", &Request::new("HEAD", Some(key)), &[404])?;
                if head.status == 404 {
                    self.overwrite(key, envelope)?;
                    Ok(Created::Yes)
                } else {
                    Ok(Created::AlreadyThere)
                }
            }
            Err(e) => Err(e),
        }
    }

    fn overwrite(&self, key: &str, envelope: &[u8]) -> Result<(), StoreError> {
        let req = Request::new("PUT", Some(key)).header("content-type", "application/octet-stream").body(envelope);
        self.client.send("PUT", &req, &[]).map(|_| ())
    }

    /// Moves the object's `LastModified` to now without transferring its data:
    /// a server-side copy onto itself with replaced metadata.
    fn refresh(&self, key: &str) -> Result<bool, StoreError> {
        let now_ms = Timestamp::now().as_millis();
        let req = Request::new("PUT", Some(key))
            .header("x-amz-copy-source", self.client.copy_source(key))
            .header("x-amz-metadata-directive", "REPLACE")
            .header("x-amz-meta-ag-refreshed", now_ms.to_string())
            .header("content-type", "application/octet-stream");
        match self.client.send("refresh (CopyObject)", &req, &[404]) {
            Ok(r) => Ok(r.status != 404),
            Err(e) => Err(e),
        }
    }

    /// `LastModified` of an object in Unix milliseconds, `None` if it does not exist.
    fn modified_ms(&self, key: &str) -> Result<Option<i64>, StoreError> {
        let r = self.client.send("HEAD", &Request::new("HEAD", Some(key)), &[404])?;
        if r.status == 404 {
            return Ok(None);
        }
        let lm = r.header("last-modified").ok_or_else(|| remote(format!("HEAD {key}: no Last-Modified header")))?;
        xml::parse_http_date_ms(lm)
            .map(Some)
            .ok_or_else(|| remote(format!("HEAD {key}: unreadable Last-Modified '{lm}'")))
    }
}

impl ObjectStore for S3ObjectStore {
    fn put(&self, obj: &RawObject) -> Result<ObjectId, StoreError> {
        check_size(obj, &self.limits)?;
        let id = obj.id();
        let key = self.key_of(&id);
        let envelope = obj.encode_envelope();
        // Writers share the lock; the collector's delete excludes them (see `delete_if_older_than`).
        let _guard = self.lock.shared()?;
        self.fire("put.locked");
        match self.put_if_absent(&key, &envelope)? {
            Created::Yes => Ok(id),
            Created::AlreadyThere => {
                match self.get_bytes(&key)? {
                    Some(bytes) if Self::decode(&id, &bytes).is_ok() => {
                        // Healthy: refresh its age so a concurrent GC keeps it. (If it vanished
                        // in between, which only an outside actor can cause, recreate it.)
                        if !self.refresh(&key)? {
                            self.overwrite(&key, &envelope)?;
                        }
                    }
                    // Damaged or gone: putting the same content repairs the store.
                    _ => self.overwrite(&key, &envelope)?,
                }
                Ok(id)
            }
        }
    }

    fn get(&self, id: &ObjectId) -> Result<Option<RawObject>, StoreError> {
        match self.get_bytes(&self.key_of(id))? {
            None => Ok(None),
            Some(bytes) => Self::decode(id, &bytes).map(Some),
        }
    }

    fn contains(&self, id: &ObjectId) -> Result<bool, StoreError> {
        let r = self.client.send("HEAD", &Request::new("HEAD", Some(&self.key_of(id))), &[404])?;
        Ok(r.status != 404)
    }

    fn verify(&self, id: &ObjectId) -> Result<ObjectStatus, StoreError> {
        match self.get(id) {
            Ok(None) => Ok(ObjectStatus::Missing),
            Ok(Some(o)) => Ok(ObjectStatus::Ok { size: o.payload.len() as u64 }),
            Err(StoreError::Corrupt { reason, .. }) => Ok(ObjectStatus::Corrupt(reason)),
            Err(e) => Err(e),
        }
    }

    fn list(&self) -> Result<Vec<ObjectInfo>, StoreError> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = Request::new("GET", None)
                .query("list-type", "2")
                .query("prefix", self.objects_prefix())
                .query("max-keys", self.page_size.to_string());
            if let Some(t) = &token {
                req = req.query("continuation-token", t.clone());
            }
            let page = self.client.send("list", &req, &[])?.text();
            for c in xml::blocks(&page, "Contents") {
                let Some(key) = xml::first_text(c, "Key") else { continue };
                let Some(id) = self.id_of(&key) else { continue }; // not ours
                let modified = xml::first_text(c, "LastModified")
                    .and_then(|s| xml::parse_iso8601_ms(&s))
                    .ok_or_else(|| remote(format!("list: no usable LastModified for {key}")))?;
                let size = xml::first_text(c, "Size").and_then(|s| s.parse().ok()).unwrap_or(0);
                out.push(ObjectInfo { id, size, modified: Timestamp::from_millis(modified) });
            }
            if xml::first_text(&page, "IsTruncated").as_deref() == Some("true") {
                token = Some(
                    xml::first_text(&page, "NextContinuationToken")
                        .ok_or_else(|| remote("list: truncated page without a continuation token"))?,
                );
            } else {
                break;
            }
        }
        out.sort_by_key(|i| i.id);
        Ok(out)
    }

    fn delete(&self, id: &ObjectId) -> Result<bool, StoreError> {
        let key = self.key_of(id);
        let _guard = self.lock.exclusive()?;
        if self.modified_ms(&key)?.is_none() {
            return Ok(false);
        }
        self.client.send("DELETE", &Request::new("DELETE", Some(&key)), &[404])?;
        Ok(true)
    }

    fn delete_if_older_than(&self, id: &ObjectId, cutoff: Timestamp) -> Result<bool, StoreError> {
        let key = self.key_of(id);
        // Exclusive across the whole check-then-act: a writer that re-puts the object
        // either finished before this (and refreshed the age we are about to read) or
        // starts after the delete (and recreates the object).
        let _guard = self.lock.exclusive()?;
        let Some(modified) = self.modified_ms(&key)? else { return Ok(false) };
        if modified >= cutoff.as_millis() {
            return Ok(false);
        }
        self.fire("delete.checked");
        self.client.send("DELETE", &Request::new("DELETE", Some(&key)), &[404])?;
        Ok(true)
    }

    fn block_gc(&self) -> Result<LockGuard, StoreError> {
        self.lock.shared()
    }

    fn durability(&self) -> Durability {
        Durability {
            name: "s3-compatible",
            survives: &[
                "process crash or loss of this host (a 2xx answer means the service accepted and stored the object)",
                "loss of a disk or node, if the service replicates or erasure-codes: Amazon S3 does, and so does \
                 multi-node MinIO; check what your provider documents",
            ],
            does_not_survive: &[
                "loss of the service's storage when it is a single node or a single disk (single-node MinIO, a test server)",
                "loss of a whole site or region unless the bucket is replicated across sites",
                "deletion or overwrite by anyone with write access to the bucket (turn on bucket versioning or object lock)",
                "anything the provider does not promise: a successful write is not evidence of replication",
            ],
        }
    }
}

impl std::fmt::Debug for S3ObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3ObjectStore").field("bucket", &self.bucket).field("prefix", &self.prefix).finish()
    }
}
