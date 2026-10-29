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
mod comment_drafts;
mod convert;
mod events;
mod file_drafts;
mod history;
mod integration;
mod issues;
mod jobs;
mod objects;
mod proposals;
mod repository;
mod review_drafts;
mod review_threads;
mod run_output;
mod search;
mod session;
mod streams;
mod tree;
mod workspaces;

pub use events::EventBatch;
pub use objects::ObjectSink;

/// A keyed creation whose first answer has not been recorded yet.
///
/// The record belongs in the transaction that creates the object ([`Claim::record`], called from the engine's
/// `*_with` functions): either the object and its idempotency record are both committed or neither is, so a
/// crash or a write error can never leave an object that a retry with the same key would create again - on this
/// replica or another.
pub(crate) struct Claim {
    slot: String,
    hash: String,
    keyed: bool,
    idem: Idem,
}

/// How long idempotency records live (`repos.idempotency_ttl_secs`) and what time it is.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Idem {
    /// Lifetime of a record in milliseconds (0 = forever).
    pub ttl_ms: i64,
    /// Now, in milliseconds.
    pub now_ms: i64,
}

/// The key under which a record's creation time is kept (zero-padded, so that text order is time order).
pub(crate) const IDEM_TIME_PREFIX: &str = "idemt:";

fn time_slot(slot: &str) -> String {
    format!("{IDEM_TIME_PREFIX}{}", slot.strip_prefix("idem:").unwrap_or(slot))
}

pub(crate) fn padded_ms(ms: i64) -> String {
    format!("{:020}", ms.max(0))
}

impl Idem {
    /// Whether the record in `slot` has outlived its lifetime. A record without a time (written before records
    /// carried one) is kept.
    fn expired(&self, tx: &ag_storage::meta::Tx<'_>, slot: &str) -> Result<bool, ag_storage::meta::MetaError> {
        if self.ttl_ms <= 0 {
            return Ok(false);
        }
        let created = tx.meta_get(&time_slot(slot))?.and_then(|t| t.parse::<i64>().ok());
        Ok(created.is_some_and(|c| c.saturating_add(self.ttl_ms) <= self.now_ms))
    }
}

impl Claim {
    /// Checks for a concurrent winner before evaluating mutable preconditions.
    pub(crate) fn ensure_available(&self, tx: &ag_storage::meta::Tx<'_>) -> Result<(), ag_storage::meta::MetaError> {
        if self.keyed && tx.meta_get(&self.slot)?.is_some() && !self.idem.expired(tx, &self.slot)? {
            return Err(ag_storage::meta::MetaError::AlreadyExists(format!("idempotency record {}", self.slot)));
        }
        Ok(())
    }

    /// Records `response` as the answer to this key. Call it inside the creating transaction. If somebody else
    /// recorded the key first, the creation is rolled back with `AlreadyExists` and [`keyed`] replays theirs.
    pub(crate) fn record<T: serde::Serialize>(
        &self,
        tx: &ag_storage::meta::Tx<'_>,
        response: &T,
    ) -> Result<(), ag_storage::meta::MetaError> {
        if !self.keyed {
            return Ok(());
        }
        // A record that has outlived its lifetime is replaced: the key may be used again.
        self.ensure_available(tx)?;
        let json = serde_json::to_string(response)?;
        tx.meta_set(&time_slot(&self.slot), &padded_ms(self.idem.now_ms))?;
        tx.meta_set(&self.slot, &format!("{}\n{json}", self.hash))
    }
}

/// Runs a creation at most once per `(scope, key)` in a repository, recording the response.
///
/// A repeat of the same request returns the recorded response (what the first call answered, not a fresh view of
/// the object); the same key with a different request is `idempotency_key_reused`. An empty key means "no
/// idempotency": `create` simply runs. `create` receives the [`Claim`] and must call [`Claim::record`] in the
/// transaction that creates the object. The record lives in the repository's own metadata, for
/// `repos.idempotency_ttl_secs`; after that the key is free again.
pub(crate) fn keyed<T: serde::Serialize + serde::de::DeserializeOwned>(
    repo: &ag_integrate::Repository,
    idem: Idem,
    scope: &str,
    key: &str,
    request: &str,
    create: impl FnOnce(&Claim) -> ApiResult<T>,
) -> ApiResult<T> {
    let claim = match lookup::<T>(repo, idem, scope, key, request)? {
        Ok(recorded) => return Ok(recorded),
        Err(claim) => claim,
    };
    match create(&claim) {
        // Two requests with one key raced; the other one committed first. Its answer is the answer.
        Err(e) if claim.keyed && e.code() == ag_protocol::ErrorCode::AlreadyExists => {
            match lookup::<T>(repo, idem, scope, key, request)? {
                Ok(recorded) => Ok(recorded),
                Err(_) => Err(e),
            }
        }
        other => other,
    }
}

/// The recorded response for the key, or the claim to create under it.
fn lookup<T: serde::de::DeserializeOwned>(
    repo: &ag_integrate::Repository,
    idem: Idem,
    scope: &str,
    key: &str,
    request: &str,
) -> ApiResult<Result<T, Claim>> {
    use sha2::{Digest, Sha256};
    let slot = format!("idem:{scope}:{key}");
    let hash = hex::encode(Sha256::digest(request.as_bytes()));
    if key.is_empty() {
        return Ok(Err(Claim { slot, hash, keyed: false, idem }));
    }
    let stored = repo.meta().read(|tx| Ok(if idem.expired(tx, &slot)? { None } else { tx.meta_get(&slot)? }))?;
    if let Some(stored) = stored
        && let Some((h, json)) = stored.split_once('\n')
    {
        if h != hash {
            return Err(ApiError::new(
                ag_protocol::ErrorCode::IdempotencyKeyReused,
                format!("idempotency key '{key}' was already used for a different request"),
            ));
        }
        return serde_json::from_str(json).map(Ok).map_err(ApiError::internal);
    }
    Ok(Err(Claim { slot, hash, keyed: true, idem }))
}

/// Whether `actor` may treat something owned by `owner` - a workspace's owner, a proposal's author, a lease's
/// holder, written as an identity - as its own. The same principal does (the agent itself, or another run of the
/// agent, when the owner is not a run); but a run owns what it made: when both name a run, it must be the same
/// run, so that two runs of one agent cannot touch each other's workspaces, proposals and leases (ADR-0010).
pub(crate) fn owns(owner: &str, actor: &ag_types::Identity) -> bool {
    match (owner.contains('#'), actor.as_str().contains('#')) {
        (true, true) => owner == actor.as_str(),
        _ => owner.split('#').next() == Some(actor.principal().as_str()),
    }
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

/// A keyset page token: the sort key of the last row served (`created_at` in ms and the row id), tagged like an
/// offset token. The next page starts strictly after it, whatever happens to the rows before.
pub(crate) fn encode_keyset(kind: &str, at_ms: i64, id: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!("{kind}:{at_ms}:{id}"))
}

/// The key of a [`encode_keyset`] token (`None` for the first page).
pub(crate) fn decode_keyset(kind: &str, token: &str) -> ApiResult<Option<(i64, String)>> {
    if token.is_empty() {
        return Ok(None);
    }
    let bad = || ApiError::invalid("invalid page_token");
    let raw = String::from_utf8(URL_SAFE_NO_PAD.decode(token).map_err(|_| bad())?).map_err(|_| bad())?;
    let mut parts = raw.splitn(3, ':');
    let (k, at, id) = (parts.next(), parts.next(), parts.next());
    match (k, at.and_then(|a| a.parse().ok()), id) {
        (Some(k), Some(at), Some(id)) if k == kind && !id.is_empty() => Ok(Some((at, id.to_string()))),
        _ => Err(bad()),
    }
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
