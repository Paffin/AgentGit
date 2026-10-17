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
