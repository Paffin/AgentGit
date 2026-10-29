//! Bounded import of normalized hosting issues and comments, one atomic issue at a time.
use crate::{Error, Repository, Result};
use ag_storage::meta::{IssueActivityRow, IssueImportRow, IssueRow};
use ag_types::{Identity, Timestamp};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// Maximum encoded manifest size.
pub const MAX_ISSUE_IMPORT_BYTES: usize = 4 * 1024 * 1024;
/// Provider-neutral, deliberately limited import format.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueImportManifest {
    /// Format version, currently one.
    pub version: u32,
    /// github or gitlab.
    pub provider: String,
    /// HTTPS source repository URL.
    pub source_repo: String,
    /// Explicitly unsupported source data; application requires acknowledgement.
    #[serde(default)]
    pub unsupported: Vec<String>,
    /// At most one hundred source issues.
    pub issues: Vec<SourceIssue>,
}
/// One original issue and its bounded comments.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIssue {
    /// Provider-local number or iid, represented as text.
    pub source_id: String,
    /// Original title.
    pub title: String,
    /// Original body.
    pub body: String,
    /// open or closed.
    pub state: String,
    /// Original external display name, never used as local identity.
    pub author: String,
    /// Original issue URL.
    pub url: String,
    /// Original creation time.
    pub created_at_ms: i64,
    /// Original update time.
    pub updated_at_ms: i64,
    /// Original comments.
    #[serde(default)]
    pub comments: Vec<SourceIssueComment>,
}
/// An external comment, identified within its source issue.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIssueComment {
    /// Stable provider comment id.
    pub source_id: String,
    /// Original text.
    pub body: String,
    /// Original external display name.
    pub author: String,
    /// Original comment URL.
    pub url: String,
    /// Original creation time.
    pub created_at_ms: i64,
}
/// Reviewable import plan.
pub struct IssueImportPreview {
    /// Canonical manifest digest required by apply.
    pub digest: String,
    /// Total source issues.
    pub issues: u32,
    /// Total source comments.
    pub comments: u32,
    /// New local issues.
    pub create_count: u32,
    /// Identical completed imports.
    pub skip_count: u32,
    /// Unsupported metadata acknowledged on application.
    pub warnings: Vec<String>,
    /// Existing mappings with changed source content.
    pub conflicts: Vec<String>,
}
/// External-to-local issue mapping in an import response.
pub struct ImportedIssueMapping {
    /// Provider-local issue number or iid.
    pub source_id: String,
    /// Local issue number.
    pub number: u64,
}
/// Result of a resumable import.
pub struct IssueImportResult {
    /// Issues created by this invocation.
    pub created: u32,
    /// Previously imported identical issues.
    pub skipped: u32,
    /// Mappings for every requested source issue.
    pub mappings: Vec<ImportedIssueMapping>,
}
fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn source_text(value: &str, max: usize, label: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > max || value.contains(['\0', '\r', '\n']) {
        return Err(Error::Invalid(format!("invalid {label}")));
    }
    Ok(())
}
fn source_url(value: &str) -> Result<()> {
    if !value.starts_with("https://")
        || value.len() > 2048
        || value.chars().any(char::is_whitespace)
        || value.contains('\0')
    {
        return Err(Error::Invalid("source URLs must be bounded HTTPS URLs".into()));
    }
    Ok(())
}
fn canonical_repo(value: &str) -> Result<String> {
    source_url(value)?;
    if value.contains(['?', '#']) {
        return Err(Error::Invalid("source repository URL cannot contain query or fragment".into()));
    }
    let (host, path) =
        value[8..].split_once('/').ok_or_else(|| Error::Invalid("source repository URL needs a path".into()))?;
    if host.is_empty() || host.contains('@') || path.trim_matches('/').is_empty() {
        return Err(Error::Invalid("invalid source repository URL".into()));
    }
    Ok(format!("https://{}/{}", host.to_ascii_lowercase(), path.trim_end_matches('/')))
}
fn escaped(value: &str) -> String {
    value.chars().flat_map(|ch| if "\\`*_{}[]<>()!".contains(ch) { vec!['\\', ch] } else { vec![ch] }).collect()
}
fn body_with_source(provider: &str, author: &str, url: &str, created: i64, body: &str, limit: usize) -> Result<String> {
    source_text(author, 256, "source author")?;
    source_url(url)?;
    if created < 0 || body.contains('\0') {
        return Err(Error::Invalid("invalid source timestamp or body".into()));
    }
    let body = format!(
        "> Imported from {}: {}\n> Original author: {} · {}\n\n{}",
        provider,
        escaped(url),
        escaped(author),
        Timestamp::from_millis(created),
        body
    );
    if body.len() > limit {
        return Err(Error::Invalid(format!("imported text plus provenance exceeds {limit} UTF-8 bytes")));
    }
    Ok(body)
}
fn parse(raw: &str) -> Result<IssueImportManifest> {
    if raw.len() > MAX_ISSUE_IMPORT_BYTES {
        return Err(Error::Invalid("issue import manifest exceeds 4 MiB".into()));
    }
    let mut m: IssueImportManifest =
        serde_json::from_str(raw).map_err(|e| Error::Invalid(format!("invalid issue import manifest: {e}")))?;
    if m.version != 1 || !["github", "gitlab"].contains(&m.provider.as_str()) || m.issues.len() > 100 {
        return Err(Error::Invalid("expected manifest version1, github/gitlab and at most100 issues".into()));
    }
    m.source_repo = canonical_repo(&m.source_repo)?;
    if m.unsupported.len() > 100 {
        return Err(Error::Invalid("at most100 unsupported metadata warnings".into()));
    }
    for warning in &m.unsupported {
        source_text(warning, 240, "unsupported metadata warning")?;
    }
    m.unsupported.sort();
    m.unsupported.dedup();
    let mut ids = BTreeSet::new();
    let mut comments = 0usize;
    for issue in &mut m.issues {
        source_text(&issue.source_id, 128, "source issue id")?;
        source_text(&issue.title, 240, "issue title")?;
        if !ids.insert(issue.source_id.clone()) {
            return Err(Error::Invalid("duplicate source issue id".into()));
        }
        if !["open", "closed"].contains(&issue.state.as_str()) || issue.updated_at_ms < issue.created_at_ms {
            return Err(Error::Invalid("invalid source issue state or timestamps".into()));
        }
        body_with_source(&m.provider, &issue.author, &issue.url, issue.created_at_ms, &issue.body, 65536)?;
        comments += issue.comments.len();
        if comments > 1000 {
            return Err(Error::Invalid("issue import exceeds1000 comments".into()));
        }
        let mut comment_ids = BTreeSet::new();
        for comment in &issue.comments {
            source_text(&comment.source_id, 128, "source comment id")?;
            if !comment_ids.insert(&comment.source_id) {
                return Err(Error::Invalid("duplicate source comment id".into()));
            }
            body_with_source(&m.provider, &comment.author, &comment.url, comment.created_at_ms, &comment.body, 16384)?;
        }
        issue.comments.sort_by(|a, b| (a.created_at_ms, &a.source_id).cmp(&(b.created_at_ms, &b.source_id)));
    }
    m.issues.sort_by(|a, b| a.source_id.cmp(&b.source_id));
    Ok(m)
}
fn canonical<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|e| Error::Invalid(e.to_string()))
}
impl Repository {
    /// Validates source records and reports changed mappings without writing anything.
    pub fn preview_issue_import(&self, raw: &str) -> Result<IssueImportPreview> {
        let m = parse(raw)?;
        let mut plan = IssueImportPreview {
            digest: digest(canonical(&m)?.as_bytes()),
            issues: m.issues.len() as u32,
            comments: m.issues.iter().map(|i| i.comments.len() as u32).sum(),
            create_count: 0,
            skip_count: 0,
            warnings: m.unsupported.clone(),
            conflicts: vec![],
        };
        self.meta.read(|tx| {
            for issue in &m.issues {
                match tx.issue_import(&m.provider, &m.source_repo, &issue.source_id)? {
                    None => plan.create_count += 1,
                    Some(row) if row.digest == digest(serde_json::to_string(issue)?.as_bytes()) => plan.skip_count += 1,
                    Some(_) => plan
                        .conflicts
                        .push(format!("source issue {} was already imported with different content", issue.source_id)),
                }
            }
            Ok(())
        })?;
        Ok(plan)
    }
    /// Imports one complete issue per transaction; retry skips committed identical snapshots.
    pub fn apply_issue_import(
        &self,
        raw: &str,
        expected_digest: &str,
        allow_unsupported: bool,
        actor: &Identity,
    ) -> Result<IssueImportResult> {
        let m = parse(raw)?;
        let plan = self.preview_issue_import(raw)?;
        if plan.digest != expected_digest {
            return Err(Error::Conflict("import manifest changed since preview".into()));
        }
        if !plan.conflicts.is_empty() {
            return Err(Error::Conflict(plan.conflicts.join("; ")));
        }
        if !plan.warnings.is_empty() && !allow_unsupported {
            return Err(Error::Invalid("unsupported metadata requires explicit allow_unsupported".into()));
        }
        let mut result = IssueImportResult { created: 0, skipped: 0, mappings: vec![] };
        for issue in &m.issues {
            let source_json = canonical(issue)?;
            let source_digest = digest(source_json.as_bytes());
            let (number,created)=self.meta.write_with_error(|tx| {
                if let Some(existing)=tx.issue_import(&m.provider,&m.source_repo,&issue.source_id)? {
                    if existing.digest!=source_digest {return Err(Error::Conflict(format!("source issue {} changed during import",issue.source_id)));}
                    return Ok((existing.issue_number,false));
                }
                let row=tx.insert_imported_issue(&IssueRow{number:0,title:issue.title.clone(),body:body_with_source(&m.provider,&issue.author,&issue.url,issue.created_at_ms,&issue.body,65536)?,author:actor.to_string(),state:issue.state.clone(),created_at_ms:issue.created_at_ms,updated_at_ms:issue.updated_at_ms,revision:1,task_id:None})?;
                tx.insert_imported_activity(&IssueActivityRow{id:0,issue_number:row.number,kind:"created".into(),body:String::new(),author:actor.to_string(),created_at_ms:issue.created_at_ms})?;
                for comment in &issue.comments {
                    tx.insert_imported_activity(&IssueActivityRow{id:0,issue_number:row.number,kind:"commented".into(),body:body_with_source(&m.provider,&comment.author,&comment.url,comment.created_at_ms,&comment.body,16384)?,author:actor.to_string(),created_at_ms:comment.created_at_ms})?;
                }
                tx.insert_issue_import(&IssueImportRow{provider:m.provider.clone(),source_repo:m.source_repo.clone(),source_id:issue.source_id.clone(),digest:source_digest.clone(),issue_number:row.number,source_json:source_json.clone(),imported_by:actor.to_string()})?;
                let details=serde_json::json!({"number":row.number,"source_repo":m.source_repo,"source_id":issue.source_id,"provider":m.provider,"comments":issue.comments.len()});
                tx.audit_append(actor.as_str(),"issue.imported",&format!("issue:{}",row.number),&details)?;
                tx.outbox_push("issue",&details)?;
                Ok((row.number,true))
            })?;
            if created {
                result.created += 1
            } else {
                result.skipped += 1
            };
            result.mappings.push(ImportedIssueMapping { source_id: issue.source_id.clone(), number });
        }
        Ok(result)
    }
}
