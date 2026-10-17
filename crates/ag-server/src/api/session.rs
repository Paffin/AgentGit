//! Protocol negotiation and caller identity.

use ag_protocol::v1;
use ag_protocol::{ErrorCode, ProtocolError, VERSION, features, limits};

use crate::app::{App, Call};
use crate::authz::Authorizer;
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
            limits: Some(v1::Limits {
                max_object_bytes: self.cfg.limits.max_object_bytes,
                max_request_bytes: self.cfg.limits.max_request_bytes,
                max_batch: limits::MAX_BATCH,
                max_page_size: limits::MAX_PAGE_SIZE,
                max_changeset_ops: 100_000,
                max_changesets_per_publication: 10_000,
            }),
            auth_methods,
        })
    }

    /// Who am I, and what may I do?
    pub async fn whoami(&self, call: &Call, _req: v1::WhoamiRequest) -> ApiResult<v1::Principal> {
        let bindings = self.bindings().await?;
        let p = &call.principal;
        let mut roles: Vec<String> = Vec::new();
        for b in bindings.iter() {
            if Authorizer::roles(p, std::slice::from_ref(b), &b.repo.replace('*', "x")).contains(&b.role) {
                roles.push(format!("{}:{}", b.repo, b.role.as_str()));
            }
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
