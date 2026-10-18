//! Protocol negotiation and caller identity.

use ag_protocol::v1;
use ag_protocol::{ErrorCode, ProtocolError, VERSION, features, limits};

use crate::app::{App, Call};
use crate::error::{ApiError, ApiResult};

impl App {
    /// Capability negotiation. Needs no credentials: it only reveals the
    /// protocol version, limits and accepted authentication methods.
    pub fn negotiate(&self, req: v1::NegotiateRequest) -> ApiResult<v1::NegotiateResponse> {
        if !req.supported_versions.is_empty() && !req.supported_versions.iter().any(|v| v == VERSION) {
            return Err(ApiError(
                ProtocolError::new(
                    ErrorCode::FailedPrecondition,
                    format!("this server speaks {VERSION}; the client supports {}", req.supported_versions.join(", ")),
                )
                .with_detail("server_versions", VERSION),
            ));
        }
        let mut advertised: Vec<String> = features::ALL
            .iter()
            .filter(|f| **f != features::AUTH_OIDC || self.auth.oidc_enabled())
            .map(|f| f.to_string())
            .collect();
        if !req.features.is_empty() {
            advertised.retain(|f| req.features.contains(f));
        }
        let mut auth_methods = vec!["bearer".to_string()];
        if self.auth.oidc_enabled() {
            auth_methods.push("oidc".into());
        }
        Ok(v1::NegotiateResponse {
            protocol_version: VERSION.into(),
            server_version: env!("CARGO_PKG_VERSION").into(),
            features: advertised,
            limits: Some(self.advertised_limits()),
            auth_methods,
        })
    }

    /// The limits clients are told about. The ones the engine enforces (operations per changeset, changesets per
    /// publication, the largest object) are read from the engine's own `Limits`, not repeated here, so what is
    /// advertised is what is enforced; the rest are this server's configuration.
    pub fn advertised_limits(&self) -> v1::Limits {
        let engine = ag_types::Limits::default();
        let saturating = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        v1::Limits {
            max_object_bytes: self.cfg.limits.max_object_bytes.min(crate::config::engine_max_object_bytes()),
            max_request_bytes: self.cfg.limits.max_request_bytes,
            max_batch: limits::MAX_BATCH,
            max_page_size: limits::MAX_PAGE_SIZE,
            max_changeset_ops: saturating(engine.max_ops),
            max_changesets_per_publication: saturating(engine.max_changesets_per_publication),
        }
    }

    /// Who am I, and what may I do?
    pub async fn whoami(&self, call: &Call, _req: v1::WhoamiRequest) -> ApiResult<v1::Principal> {
        let bindings = self.bindings().await?;
        let p = &call.principal;
        let mut roles: Vec<String> = Vec::new();
        // The bindings that apply to this caller, each with the repository pattern it was granted on - decided
        // by the subject alone, with no invented repository name to test the pattern against.
        for b in bindings.iter().filter(|b| !p.bootstrap && b.applies_to(p)) {
            roles.push(format!("{}:{}", b.repo, b.role.as_str()));
        }
        for (role, pattern) in &p.group_roles {
            roles.push(format!("{pattern}:{}", role.as_str()));
        }
        if p.bootstrap {
            roles.push("*:admin".into());
        }
        roles.sort();
        roles.dedup();
        Ok(v1::Principal {
            identity: p.identity.to_string(),
            kind: p.kind().into(),
            method: p.method.as_str().into(),
            credential_id: p.credential_id.clone(),
            expires_at_ms: p.expires_at_ms,
            roles,
            scope: Some(p.scope.to_wire()),
        })
    }
}
