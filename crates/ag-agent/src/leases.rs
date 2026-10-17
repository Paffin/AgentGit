//! Leases and fencing tokens.
//!
//! * A **soft lease** is a signal to other agents ("I am working here"). It
//!   never blocks anyone; overlaps are reported as warnings.
//! * An **exclusive lease** additionally restricts *integration*: while it is
//!   active, a change touching its scope can only be integrated if it is
//!   authored by the holder **and** presents the lease's current fencing token.
//!
//! Fencing tokens are drawn from one strictly increasing counter. A holder
//! whose lease expired and was granted to someone else still holds an old,
//! lower token, so its late writes are refused on the server side no matter
//! what it believes. Expiry never deletes or invalidates the *changes* an
//! agent already proposed; it only stops the stale holder from integrating
//! into a scope someone else now holds.

use ag_crypto::random_id;
use ag_policy::PathMatcher;
use ag_storage::Meta;
use ag_storage::meta::{LeaseRow, MetaError, Tx};
use ag_types::Timestamp;
use serde::Serialize;
use serde_json::json;

use crate::AgentError;

/// Longest allowed lease duration.
pub const MAX_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// Result of acquiring a lease.
#[derive(Clone, Debug, Serialize)]
pub struct Acquired {
    /// The granted lease.
    pub lease: LeaseRow,
    /// Other active leases whose scope may overlap (advisory).
    pub overlaps: Vec<LeaseRow>,
}

/// The literal prefix of a glob (text before the first wildcard).
fn literal_prefix(glob: &str) -> &str {
    let end = glob.find(['*', '?', '[', '{']).unwrap_or(glob.len());
    &glob[..end]
}

/// Conservative overlap test between two globs: true unless their literal
/// prefixes prove they cannot match a common path.
pub fn scopes_may_overlap(a: &str, b: &str) -> bool {
    let (pa, pb) = (literal_prefix(a), literal_prefix(b));
    pa.starts_with(pb) || pb.starts_with(pa)
}

fn principal(identity: &str) -> &str {
    identity.split('#').next().unwrap_or(identity)
}

/// Acquires a lease on `scope` (a path glob) for `holder`.
///
/// Exclusive leases are refused while another holder has an active
/// *exclusive* lease on an overlapping scope.
pub fn acquire(meta: &Meta, scope: &str, holder: &str, exclusive: bool, ttl_ms: i64, task_id: Option<&str>) -> Result<Acquired, AgentError> {
    PathMatcher::new(&[scope.to_string()]).map_err(AgentError::Invalid)?;
    if !(1..=MAX_TTL_MS).contains(&ttl_ms) {
        return Err(AgentError::Invalid(format!("ttl must be between 1 ms and {MAX_TTL_MS} ms")));
    }
    let id = random_id("lease")?;
    let result = meta.write(|tx| {
        let active = tx.leases_active()?;
        let overlaps: Vec<LeaseRow> = active
            .into_iter()
            .filter(|l| principal(&l.holder) != principal(holder) && scopes_may_overlap(&l.scope, scope))
            .collect();
        if exclusive {
            if let Some(blocker) = overlaps.iter().find(|l| l.exclusive) {
                return Err(MetaError::Invalid(format!(
                    "scope '{}' overlaps an exclusive lease on '{}' held by {} until {}",
                    scope, blocker.scope, blocker.holder, blocker.expires_at
                )));
            }
        }
        let token = tx.next_counter("fencing")?;
        let lease = LeaseRow {
            id: id.clone(),
            scope: scope.to_string(),
            holder: holder.to_string(),
            exclusive,
            token,
            acquired_at: tx.now(),
            expires_at: Timestamp::from_millis(tx.now().as_millis() + ttl_ms),
            released_at: None,
            task_id: task_id.map(str::to_string),
        };
        tx.lease_insert(&lease)?;
        tx.audit_append(holder, "lease.acquire", &id, &json!({"scope": scope, "exclusive": exclusive, "token": token}))?;
        Ok(Acquired { lease, overlaps })
    });
    result.map_err(|e| match e {
        MetaError::Invalid(m) => AgentError::Conflict(m),
        other => AgentError::Meta(other),
    })
}

/// Releases a lease. Only its holder (same principal) may release it.
pub fn release(meta: &Meta, lease_id: &str, actor: &str) -> Result<(), AgentError> {
    meta.write(|tx| {
        let l = tx.lease(lease_id)?.ok_or_else(|| MetaError::NotFound(format!("lease '{lease_id}'")))?;
        if principal(&l.holder) != principal(actor) {
            return Err(MetaError::Invalid(format!("lease '{lease_id}' is held by {}", l.holder)));
        }
        tx.lease_release(lease_id)?;
        tx.audit_append(actor, "lease.release", lease_id, &json!({}))?;
        Ok(())
    })
    .map_err(|e| match e {
        MetaError::Invalid(m) => AgentError::Conflict(m),
        MetaError::NotFound(m) => AgentError::NotFound(m),
        other => AgentError::Meta(other),
    })
}

/// Extends an active lease (heartbeat). The fencing token does not change.
pub fn extend(meta: &Meta, lease_id: &str, actor: &str, ttl_ms: i64) -> Result<LeaseRow, AgentError> {
    if !(1..=MAX_TTL_MS).contains(&ttl_ms) {
        return Err(AgentError::Invalid(format!("ttl must be between 1 ms and {MAX_TTL_MS} ms")));
    }
    meta.write(|tx| {
        let l = tx.lease(lease_id)?.ok_or_else(|| MetaError::NotFound(format!("lease '{lease_id}'")))?;
        if principal(&l.holder) != principal(actor) {
            return Err(MetaError::Invalid(format!("lease '{lease_id}' is held by {}", l.holder)));
        }
        tx.lease_extend(lease_id, Timestamp::from_millis(tx.now().as_millis() + ttl_ms))?;
        tx.lease(lease_id)?.ok_or_else(|| MetaError::Corrupt("lease vanished".into()))
    })
    .map_err(|e| match e {
        MetaError::Invalid(m) => AgentError::Conflict(m),
        MetaError::NotFound(m) => AgentError::NotFound(m),
        other => AgentError::Meta(other),
    })
}

/// Why a change cannot be integrated because of an exclusive lease.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LeaseBlocker {
    /// The lease's scope.
    pub scope: String,
    /// Its holder.
    pub holder: String,
    /// `held_by_other` or `stale_or_missing_token`.
    pub reason: String,
}

impl std::fmt::Display for LeaseBlocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "exclusive lease on '{}' held by {} ({})", self.scope, self.holder, self.reason)
    }
}

/// Checks the active exclusive leases against a change. `author` is the
/// proposal author, `tokens` the fencing tokens the author presented.
pub fn lease_blockers(tx: &Tx<'_>, changed_paths: &[String], author: &str, tokens: &[u64]) -> Result<Vec<LeaseBlocker>, MetaError> {
    let mut out = Vec::new();
    for l in tx.leases_active()?.into_iter().filter(|l| l.exclusive) {
        let Ok(m) = PathMatcher::new(std::slice::from_ref(&l.scope)) else { continue };
        if !changed_paths.iter().any(|p| m.is_match(p)) {
            continue;
        }
        if principal(&l.holder) != principal(author) {
            out.push(LeaseBlocker { scope: l.scope, holder: l.holder, reason: "held_by_other".into() });
        } else if !tokens.contains(&l.token) {
            out.push(LeaseBlocker { scope: l.scope, holder: l.holder, reason: "stale_or_missing_token".into() });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_is_conservative_but_sound() {
        assert!(scopes_may_overlap("src/**", "src/lib.rs"));
        assert!(scopes_may_overlap("src/a/**", "src/**"));
        assert!(scopes_may_overlap("**/*.rs", "src/lib.rs"), "leading wildcard may match anything");
        assert!(!scopes_may_overlap("src/**", "docs/**"));
        assert!(!scopes_may_overlap("a/b/**", "a/c/**"));
    }
}
