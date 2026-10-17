//! Administration: policy, pins, audit, GC, fsck, credentials and roles.

use ag_integrate::GcOptions;
use ag_protocol::v1;
use ag_types::Identity;

use super::{convert, require};
use crate::app::{App, Call};
use crate::auth::mint_token;
use crate::authz::{Authorizer, Cap, Role, Scope};
use crate::control::CredentialRow;
use crate::error::{ApiError, ApiResult};

impl App {
    // ---- policy -----------------------------------------------------------------

    /// The policy in force for a stream.
    pub async fn get_policy(&self, call: &Call, req: v1::GetPolicyRequest) -> ApiResult<v1::PolicyVersion> {
        let stream = self.stream_or_default(require(&req.repo, "repo")?, &req.stream).await?;
        let s = stream.clone();
        self.with_repo(call, require(&req.repo, "repo")?, Cap::PolicyRead, Some(&stream), move |repo, _| {
            if repo.meta().read(|tx| tx.stream(&s))?.is_none() {
                return Err(ApiError::not_found(format!("stream '{s}' not found")));
            }
            Ok(match repo.meta().read(|tx| tx.policy_current(&s))? {
                Some(row) => convert::policy(&row),
                None => v1::PolicyVersion {
                    stream: s.clone(),
                    version: 0,
                    document: ag_policy::Policy::default().to_json(),
                    doc_hash: String::new(),
                    created_at_ms: 0,
                    created_by: "builtin".into(),
                },
            })
        })
        .await
    }

    /// Stores a new policy version.
    pub async fn set_policy(&self, call: &Call, req: v1::SetPolicyRequest) -> ApiResult<v1::PolicyVersion> {
        let stream = require(&req.stream, "stream")?.to_string();
        let s = stream.clone();
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::PolicyWrite, Some(&stream), move |repo, actor| {
            Ok(convert::policy(&repo.set_policy(&s, &req.document, actor)?))
        })
        .await
    }

    /// All versions of a stream's policy.
    pub async fn policy_history(
        &self,
        call: &Call,
        req: v1::PolicyHistoryRequest,
    ) -> ApiResult<v1::PolicyHistoryResponse> {
        let stream = self.stream_or_default(require(&req.repo, "repo")?, &req.stream).await?;
        let s = stream.clone();
        self.with_repo(call, require(&req.repo, "repo")?, Cap::PolicyRead, Some(&stream), move |repo, _| {
            Ok(v1::PolicyHistoryResponse { versions: repo.policy_history(&s)?.iter().map(convert::policy).collect() })
        })
        .await
    }

    // ---- pins ---------------------------------------------------------------------

    /// Pins a revision as a garbage-collection root.
    pub async fn pin(&self, call: &Call, req: v1::PinRequest) -> ApiResult<v1::Pin> {
        let (name, rev) = (require(&req.name, "name")?.to_string(), require(&req.rev, "rev")?.to_string());
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::PinsWrite, None, move |repo, actor| {
            let id = repo.resolve_rev(&rev)?;
            repo.pin(&name, &id, &req.reason, actor)?;
            let pins = repo.pins()?;
            let row = pins.iter().find(|p| p.name == name).ok_or_else(|| ApiError::internal("pin vanished"))?;
            Ok(convert::pin(row))
        })
        .await
    }

    /// Removes a pin.
    pub async fn unpin(&self, call: &Call, req: v1::UnpinRequest) -> ApiResult<v1::Empty> {
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::PinsWrite, None, move |repo, actor| {
            if repo.unpin(&req.name, actor)? {
                Ok(v1::Empty {})
            } else {
                Err(ApiError::not_found(format!("pin '{}' not found", req.name)))
            }
        })
        .await
    }

    /// All pins.
    pub async fn list_pins(&self, call: &Call, req: v1::ListPinsRequest) -> ApiResult<v1::ListPinsResponse> {
        self.with_repo(call, require(&req.repo, "repo")?, Cap::PinsRead, None, |repo, _| {
            Ok(v1::ListPinsResponse { pins: repo.pins()?.iter().map(convert::pin).collect() })
        })
        .await
    }

    // ---- audit / maintenance --------------------------------------------------------------

    /// Audit entries mentioning `contains` (newest first).
    pub async fn list_audit(&self, call: &Call, req: v1::ListAuditRequest) -> ApiResult<v1::ListAuditResponse> {
        let limit = if req.limit == 0 { 100 } else { req.limit.min(1000) } as usize;
        self.with_repo(call, require(&req.repo, "repo")?, Cap::AuditRead, None, move |repo, _| {
            let needle = (!req.contains.is_empty()).then_some(req.contains.as_str());
            Ok(v1::ListAuditResponse {
                entries: repo.meta().read(|tx| tx.audit_list(needle, limit))?.iter().map(convert::audit).collect(),
            })
        })
        .await
    }

    /// Recomputes the audit hash chain.
    pub async fn verify_audit(&self, call: &Call, req: v1::VerifyAuditRequest) -> ApiResult<v1::VerifyAuditResponse> {
        self.with_repo(call, require(&req.repo, "repo")?, Cap::AuditRead, None, |repo, _| {
            let r = repo.meta().read(|tx| tx.audit_verify())?;
            Ok(v1::VerifyAuditResponse {
                ok: r.first_bad.is_none(),
                entries: r.entries,
                detail: match r.first_bad {
                    None => format!("chain intact; head {}", r.head),
                    Some(seq) => format!("the chain is broken at entry {seq}"),
                },
            })
        })
        .await
    }

    /// Garbage collection (fails closed; honours grace and retention).
    pub async fn gc(&self, call: &Call, req: v1::GcRequest) -> ApiResult<v1::GcResponse> {
        let repo_name = require(&req.repo, "repo")?.to_string();
        let control = self.control.clone();
        let name = repo_name.clone();
        self.with_repo_mut(call, &repo_name, Cap::GcRun, None, move |repo, actor| {
            let mut opts = GcOptions { dry_run: req.dry_run, ..GcOptions::default() };
            if req.grace_ms > 0 {
                opts.grace_ms = req.grace_ms;
            }
            if req.reflog_retention_ms > 0 {
                opts.reflog_retention_ms = req.reflog_retention_ms;
            }
            let r = repo.gc(&opts, actor.as_str())?;
            if !r.dry_run {
                // Keep the quota accounting honest after deleting objects.
                let total: u64 = repo.objects().list()?.iter().map(|o| o.size).sum();
                control.set_object_bytes(&name, total)?;
            }
            Ok(v1::GcResponse {
                reachable_changesets: r.reachable_changesets as u64,
                reachable_blobs: r.reachable_blobs as u64,
                scanned: r.scanned as u64,
                deleted: r.deleted.len() as u64,
                bytes: r.bytes,
                kept_young: r.kept_young as u64,
                kept_staged: r.kept_staged as u64,
                dry_run: r.dry_run,
            })
        })
        .await
    }

    /// Whole-repository integrity check.
    pub async fn fsck(&self, call: &Call, req: v1::FsckRequest) -> ApiResult<v1::FsckResponse> {
        self.with_repo(call, require(&req.repo, "repo")?, Cap::FsckRun, None, |repo, _| {
            let r = repo.fsck()?;
            Ok(v1::FsckResponse {
                ok: r.is_ok(),
                issues: r
                    .issues
                    .iter()
                    .map(|i| v1::FsckIssue { kind: i.kind.clone(), object: String::new(), detail: i.detail.clone() })
                    .collect(),
                objects_checked: r.objects_checked as u64,
            })
        })
        .await
    }

    // ---- credentials ------------------------------------------------------------------------

    /// Mints a scoped, expiring credential. The secret is returned once.
    pub async fn mint_credential(
        &self,
        call: &Call,
        req: v1::MintCredentialRequest,
    ) -> ApiResult<v1::MintCredentialResponse> {
        let me = &call.principal;
        let subject =
            if req.subject.is_empty() { me.identity.principal() } else { Identity::new(req.subject.clone())? };
        let wanted = Scope::from_wire(&req.scope.clone().unwrap_or_default())?;
        if !me.scope.contains(&wanted) {
            return Err(ApiError::denied("a credential cannot grant more than the credential used to mint it"));
        }
        let own = subject == me.identity.principal();
        if !own {
            let bindings = self.bindings().await?;
            let patterns: Vec<String> =
                if wanted.repos.is_empty() { vec!["*".to_string()] } else { wanted.repos.raw().to_vec() };
            for p in &patterns {
                Authorizer::check(me, &bindings, p, Cap::CredentialsMint, None).map_err(|e| {
                    if e.code() == ag_protocol::ErrorCode::NotFound {
                        ApiError::denied(format!(
                            "minting credentials for {subject} on '{p}' requires an administrator"
                        ))
                    } else {
                        e
                    }
                })?;
            }
        } else if me.scope.is_unrestricted() && me.method == crate::auth::AuthMethod::Token && !me.bootstrap {
            // Self-minting is always allowed (it can only narrow); nothing more to check.
        }
        if matches!(subject.kind(), ag_types::IdentityKind::System) {
            return Err(ApiError::invalid("credentials cannot be minted for system identities"));
        }
        let is_agent = subject.kind() == ag_types::IdentityKind::Agent;
        let max = if is_agent { self.cfg.auth.max_agent_ttl_secs } else { self.cfg.auth.max_ttl_secs };
        let ttl = if req.ttl_secs == 0 { max.min(if is_agent { 3600 } else { 24 * 3600 }) } else { req.ttl_secs };
        if ttl > max {
            return Err(ApiError::invalid(format!(
                "ttl_secs may be at most {max} for {} credentials",
                subject.kind().prefix()
            )));
        }
        let now = self.clock.now().as_millis();
        let mut expires = now + i64::from(ttl) * 1000;
        if me.expires_at_ms > 0 {
            expires = expires.min(me.expires_at_ms); // a child never outlives its parent
        }
        let minted = mint_token()?;
        let row = CredentialRow {
            id: minted.credential_id.clone(),
            secret_hash: minted.secret_hash.clone(),
            subject: subject.to_string(),
            kind: subject.kind().prefix().to_string(),
            description: req.description.chars().take(200).collect(),
            scope: wanted.to_wire(),
            created_at_ms: now,
            expires_at_ms: expires,
            revoked_at_ms: None,
            last_used_ms: 0,
            created_by: me.identity.to_string(),
        };
        let control = self.control.clone();
        let wire = row.to_wire();
        tokio::task::spawn_blocking(move || control.insert_credential(&row)).await.map_err(ApiError::internal)??;
        Ok(v1::MintCredentialResponse { credential: Some(wire), token: minted.token })
    }

    async fn may_manage_credential(&self, call: &Call, cred: &CredentialRow) -> ApiResult<bool> {
        let me = &call.principal;
        if me.bootstrap || cred.subject == me.identity.principal().to_string() {
            return Ok(true);
        }
        let bindings = self.bindings().await?;
        let scope = Scope::from_wire(&cred.scope)?;
        let patterns: Vec<String> =
            if scope.repos.is_empty() { vec!["*".to_string()] } else { scope.repos.raw().to_vec() };
        Ok(patterns.iter().all(|p| Authorizer::check(me, &bindings, p, Cap::CredentialsRevoke, None).is_ok()))
    }

    /// Lists credentials. By default the caller's own; `subject` names
    /// another identity (the caller must administer it) and `*` lists all
    /// credentials the caller administers.
    pub async fn list_credentials(
        &self,
        call: &Call,
        req: v1::ListCredentialsRequest,
    ) -> ApiResult<v1::ListCredentialsResponse> {
        let me = call.principal.identity.principal().to_string();
        let subject = if req.subject.is_empty() { me.clone() } else { req.subject.clone() };
        let filter = (subject != "*").then(|| subject.clone());
        let (control, include_revoked) = (self.control.clone(), req.include_revoked);
        let all = tokio::task::spawn_blocking(move || control.credentials(filter.as_deref(), include_revoked))
            .await
            .map_err(ApiError::internal)??;
        let mut out = Vec::new();
        for c in all {
            if self.may_manage_credential(call, &c).await? {
                out.push(c.to_wire());
            }
        }
        if out.is_empty() && subject != "*" && subject != me {
            // Do not reveal whether the subject has credentials to someone who cannot manage them.
            return Ok(v1::ListCredentialsResponse { credentials: out });
        }
        Ok(v1::ListCredentialsResponse { credentials: out })
    }

    /// Revokes a credential immediately.
    pub async fn revoke_credential(
        &self,
        call: &Call,
        req: v1::RevokeCredentialRequest,
    ) -> ApiResult<v1::RevokeCredentialResponse> {
        let id = require(&req.credential_id, "credential_id")?.to_string();
        let control = self.control.clone();
        let cid = id.clone();
        let cred =
            tokio::task::spawn_blocking(move || control.credential(&cid)).await.map_err(ApiError::internal)??;
        let Some(cred) = cred else {
            return Err(ApiError::not_found("credential not found"));
        };
        if !self.may_manage_credential(call, &cred).await? {
            return Err(ApiError::not_found("credential not found"));
        }
        let (control, now) = (self.control.clone(), self.clock.now().as_millis());
        let revoked = tokio::task::spawn_blocking(move || control.revoke_credential(&id, now))
            .await
            .map_err(ApiError::internal)??;
        Ok(v1::RevokeCredentialResponse { revoked })
    }

    // ---- roles -----------------------------------------------------------------------------------

    async fn check_roles_admin(&self, call: &Call, repo_pattern: &str) -> ApiResult<()> {
        let bindings = self.bindings().await?;
        Authorizer::check(&call.principal, &bindings, repo_pattern, Cap::RolesManage, None).map_err(|e| {
            if e.code() == ag_protocol::ErrorCode::NotFound {
                ApiError::denied(format!("managing roles on '{repo_pattern}' requires an administrator"))
            } else {
                e
            }
        })
    }

    /// Grants a role to a subject pattern on a repository pattern.
    pub async fn grant_role(&self, call: &Call, req: v1::GrantRoleRequest) -> ApiResult<v1::RoleBinding> {
        let role = Role::parse(&req.role).ok_or_else(|| {
            ApiError::invalid(format!(
                "unknown role '{}' (reader, writer, reviewer, integrator, runner, admin)",
                req.role
            ))
        })?;
        let repo = if req.repo.is_empty() { "*".to_string() } else { req.repo.clone() };
        require(&req.subject, "subject")?;
        self.check_roles_admin(call, &repo).await?;
        let (control, by, subject) = (self.control.clone(), call.principal.identity.to_string(), req.subject.clone());
        let row = tokio::task::spawn_blocking(move || control.grant_role(&subject, &repo, role, &by))
            .await
            .map_err(ApiError::internal)??;
        self.invalidate_bindings();
        Ok(row)
    }

    /// Revokes a role binding.
    pub async fn revoke_role(&self, call: &Call, req: v1::RevokeRoleRequest) -> ApiResult<v1::Empty> {
        let role = Role::parse(&req.role).ok_or_else(|| ApiError::invalid(format!("unknown role '{}'", req.role)))?;
        let repo = if req.repo.is_empty() { "*".to_string() } else { req.repo.clone() };
        self.check_roles_admin(call, &repo).await?;
        let (control, subject) = (self.control.clone(), req.subject.clone());
        let removed = tokio::task::spawn_blocking(move || control.revoke_role(&subject, &repo, role))
            .await
            .map_err(ApiError::internal)??;
        self.invalidate_bindings();
        if removed { Ok(v1::Empty {}) } else { Err(ApiError::not_found("no such role binding")) }
    }

    /// Lists role bindings the caller administers.
    pub async fn list_roles(&self, call: &Call, req: v1::ListRolesRequest) -> ApiResult<v1::ListRolesResponse> {
        let (control, subject, repo) = (self.control.clone(), req.subject.clone(), req.repo.clone());
        let all = tokio::task::spawn_blocking(move || {
            control
                .roles((!subject.is_empty()).then_some(subject.as_str()), (!repo.is_empty()).then_some(repo.as_str()))
        })
        .await
        .map_err(ApiError::internal)??;
        let bindings = self.bindings().await?;
        let me = call.principal.identity.principal().to_string();
        let out = all
            .into_iter()
            .filter(|b| {
                b.subject == me
                    || Authorizer::check(&call.principal, &bindings, &b.repo, Cap::RolesManage, None).is_ok()
            })
            .collect();
        Ok(v1::ListRolesResponse { bindings: out })
    }
}
