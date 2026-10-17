//! The AGWP operations, transport independent.
//!
//! Every operation is an `async fn` on [`crate::app::App`] taking the
//! authenticated [`crate::app::Call`] and a protocol request message and
//! returning a protocol response message. The HTTP/JSON and gRPC layers are
//! thin adapters over these functions.

use std::str::FromStr;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::error::{ApiError, ApiResult};

mod admin;
mod agents;
mod convert;
mod events;
mod history;
mod integration;
mod jobs;
mod objects;
mod proposals;
mod repository;
mod session;
mod streams;
mod tree;
mod workspaces;

pub use events::EventBatch;
pub use objects::ObjectSink;

/// Serializes keyed creations so two requests with the same key cannot both create.
static KEYED_CREATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Runs `create` at most once per `(scope, key)` in a repository, recording the response.
///
/// A repeat of the same request returns the recorded response (what the first call answered, not a fresh
/// view of the object); the same key with a different request is `idempotency_key_reused`. An empty key
/// means "no idempotency": `create` simply runs. The record lives in the repository's own metadata.
pub(crate) fn idempotent_create<T: serde::Serialize + serde::de::DeserializeOwned>(
    repo: &ag_integrate::Repository,
    scope: &str,
    key: &str,
    request: &str,
    create: impl FnOnce() -> ApiResult<T>,
) -> ApiResult<T> {
    use sha2::{Digest, Sha256};
    if key.is_empty() {
        return create();
    }
    let _serial = KEYED_CREATE.lock().unwrap_or_else(|e| e.into_inner());
    let slot = format!("idem:{scope}:{key}");
    let hash = hex::encode(Sha256::digest(request.as_bytes()));
    if let Some(stored) = repo.meta().read(|tx| tx.meta_get(&slot))?
        && let Some((h, json)) = stored.split_once('\n')
    {
        if h != hash {
            return Err(ApiError::new(
                ag_protocol::ErrorCode::IdempotencyKeyReused,
                format!("idempotency key '{key}' was already used for a different request"),
            ));
        }
        return serde_json::from_str(json).map_err(ApiError::internal);
    }
    let out = create()?;
    let json = serde_json::to_string(&out).map_err(ApiError::internal)?;
    repo.meta().write(|tx| tx.meta_set(&slot, &format!("{hash}\n{json}")))?;
    Ok(out)
}

/// Parses a typed identifier, naming what it was in the error.
pub(crate) fn parse_id<T: FromStr>(s: &str, what: &str) -> ApiResult<T> {
    s.parse().map_err(|_| ApiError::invalid(format!("'{s}' is not a valid {what}")))
}

/// Clamps a requested page size.
pub(crate) fn page_size(requested: u32) -> usize {
    let max = ag_protocol::limits::MAX_PAGE_SIZE;
    let n = if requested == 0 { ag_protocol::limits::DEFAULT_PAGE_SIZE } else { requested };
    n.min(max) as usize
}

/// Page tokens are opaque to clients: an offset, tagged so a token from one
/// kind of listing is not mistaken for another.
pub(crate) fn encode_offset(kind: &str, offset: usize) -> String {
    URL_SAFE_NO_PAD.encode(format!("{kind}:{offset}"))
}

pub(crate) fn decode_offset(kind: &str, token: &str) -> ApiResult<usize> {
    if token.is_empty() {
        return Ok(0);
    }
    let bad = || ApiError::invalid("invalid page_token");
    let raw = URL_SAFE_NO_PAD.decode(token).map_err(|_| bad())?;
    let raw = String::from_utf8(raw).map_err(|_| bad())?;
    let (k, n) = raw.split_once(':').ok_or_else(bad)?;
    if k != kind {
        return Err(bad());
    }
    n.parse().map_err(|_| bad())
}

/// Slices `items` for the page described by `size` and `token`, returning the
/// page and the next token.
pub(crate) fn paginate<T>(kind: &str, items: Vec<T>, size: u32, token: &str) -> ApiResult<(Vec<T>, String)> {
    let size = page_size(size);
    let offset = decode_offset(kind, token)?;
    let total = items.len();
    let page: Vec<T> = items.into_iter().skip(offset).take(size).collect();
    let next = if offset + page.len() < total { encode_offset(kind, offset + page.len()) } else { String::new() };
    Ok((page, next))
}

/// Requires a non-empty string field.
pub(crate) fn require<'a>(value: &'a str, field: &str) -> ApiResult<&'a str> {
    if value.is_empty() { Err(ApiError::invalid(format!("'{field}' is required"))) } else { Ok(value) }
}

/// Resolves a revision expression; an empty one means the repository's default stream
/// (a server-side repository has no working copy, so `@` would be meaningless).
pub(crate) fn resolve(repo: &ag_integrate::Repository, rev: &str) -> ApiResult<ag_types::ChangesetId> {
    let rev = if rev.is_empty() { repo.default_stream().as_str() } else { rev };
    Ok(repo.resolve_rev(rev)?)
}
