//! Preview and explicit application of normalized hosting issues.
use super::require;
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::ApiResult;
use ag_protocol::v1;

impl App {
    /// Validates and previews without writing repository state.
    pub async fn preview_issue_import(
        &self,
        call: &Call,
        req: v1::PreviewIssueImportRequest,
    ) -> ApiResult<v1::IssueImportPreview> {
        self.with_repo(call, require(&req.repo, "repo")?, Cap::IssueWrite, None, move |repo, _| {
            let p = repo.preview_issue_import(&req.manifest_json)?;
            Ok(v1::IssueImportPreview {
                digest: p.digest,
                issues: p.issues,
                comments: p.comments,
                create_count: p.create_count,
                skip_count: p.skip_count,
                warnings: p.warnings,
                conflicts: p.conflicts,
            })
        })
        .await
    }
    /// Applies exactly the previewed manifest, skipping committed identical source issues.
    pub async fn apply_issue_import(
        &self,
        call: &Call,
        req: v1::ApplyIssueImportRequest,
    ) -> ApiResult<v1::IssueImportResult> {
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::IssueWrite, None, move |repo, actor| {
            let result =
                repo.apply_issue_import(&req.manifest_json, &req.expected_digest, req.allow_unsupported, actor)?;
            Ok(v1::IssueImportResult {
                created: result.created,
                skipped: result.skipped,
                mappings: result
                    .mappings
                    .into_iter()
                    .map(|m| v1::ImportedIssueMapping { source_id: m.source_id, number: m.number })
                    .collect(),
            })
        })
        .await
    }
}
