//! Complete Git repository migration, with explicit loss reporting.
use crate::{GitError, import::stage, run::git};
use ag_integrate::Repository;
use ag_types::{ChangesetId, Identity, StreamName};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Input to a complete branch and tag migration.
#[derive(Clone, Debug)]
pub struct MigrationOptions {
    /// A local Git mirror or working repository. Network acquisition is a separate step.
    pub git_dir: PathBuf,
    /// Identity recorded in the migration audit event.
    pub actor: Identity,
    /// Explicitly accept reported content omissions and unresolved LFS pointers.
    pub allow_lossy: bool,
}
/// A source ref and its destination. Original Git IDs remain available for reconciliation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigratedRef {
    /// Branch or tag name without its Git namespace.
    pub name: String,
    /// AgentGit stream name or tag pin, including any necessary name normalization.
    pub destination: String,
    /// Original peeled Git commit.
    pub git_commit: String,
    /// Converted AgentGit history head.
    pub changeset: ChangesetId,
}
/// Reconciliation report for a complete repository migration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationReport {
    /// Source default branch.
    pub default_branch: String,
    /// Distinct commits across all refs, imported once.
    pub commits: usize,
    /// Distinct file contents.
    pub blobs: usize,
    /// All imported branches.
    pub branches: Vec<MigratedRef>,
    /// Tags retained as `git-tag/<name>` pins.
    pub tags: Vec<MigratedRef>,
    /// Paths omitted by the Git adapter.
    pub skipped_paths: Vec<(String, String)>,
    /// Gitlinks omitted from history.
    pub skipped_submodules: usize,
    /// Commit text repaired to UTF-8.
    pub lossy_commits: usize,
    /// Unresolved Git LFS pointer blobs, not their external payloads.
    pub lfs_pointers: usize,
    /// Refs outside branches/tags, preserved only in the source archive.
    pub other_refs: Vec<String>,
}
impl MigrationReport {
    /// Whether the user must explicitly accept omissions before switching.
    pub fn needs_attention(&self) -> bool {
        !self.skipped_paths.is_empty()
            || self.skipped_submodules > 0
            || self.lossy_commits > 0
            || self.lfs_pointers > 0
            || !self.other_refs.is_empty()
    }
}
fn stream_name(name: &str) -> String {
    if StreamName::new(name).is_ok() {
        return name.into();
    }
    let slug: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).take(44).collect();
    let hash = Sha256::digest(name.as_bytes());
    format!("git/b-{slug}-{:02x}{:02x}{:02x}{:02x}", hash[0], hash[1], hash[2], hash[3])
}
struct SourceRef {
    name: String,
    commit: String,
    tag: bool,
}
fn text(bytes: Vec<u8>) -> Result<String, GitError> {
    String::from_utf8(bytes).map_err(|_| GitError::Unsupported("Git ref metadata must be UTF-8".into()))
}
fn ref_kind(reference: &str) -> Option<(&str, bool)> {
    if let Some(n) = reference.strip_prefix("refs/heads/") {
        Some((n, false))
    } else {
        reference.strip_prefix("refs/tags/").map(|n| (n, true))
    }
}
fn source_refs(dir: &std::path::Path) -> Result<(Vec<SourceRef>, Vec<String>), GitError> {
    let output = text(git(dir, &["for-each-ref", "--format=%(refname)"])?)?;
    let mut refs = Vec::new();
    let mut other = Vec::new();
    for reference in output.lines() {
        let Some((name, tag)) = ref_kind(reference) else {
            other.push(reference.into());
            continue;
        };
        let commit = match git(dir, &["rev-parse", "--verify", &format!("{reference}^{{commit}}")]) {
            Ok(out) => text(out)?.trim().to_string(),
            Err(GitError::Failed { .. }) => {
                other.push(reference.into());
                continue;
            }
            Err(e) => return Err(e),
        };
        refs.push(SourceRef { name: name.into(), commit, tag });
    }
    Ok((refs, other))
}
fn report_for(
    staged: crate::import::StagedImport,
    refs: &[SourceRef],
    default_branch: String,
    other_refs: Vec<String>,
) -> MigrationReport {
    let converted = |tag| {
        refs.iter()
            .filter(|r| r.tag == tag)
            .map(|r| MigratedRef {
                name: r.name.clone(),
                destination: if tag { format!("git-tag/{}", r.name) } else { stream_name(&r.name) },
                git_commit: r.commit.clone(),
                changeset: staged.ids[&r.commit],
            })
            .collect::<Vec<_>>()
    };
    MigrationReport {
        default_branch,
        commits: staged.report.commits,
        blobs: staged.report.blobs,
        branches: converted(false),
        tags: converted(true),
        skipped_paths: staged.report.skipped_paths,
        skipped_submodules: staged.report.skipped_submodules,
        lossy_commits: staged.report.lossy_commits,
        lfs_pointers: staged.lfs_pointers,
        other_refs,
    }
}
fn adopt_report(repo: &Repository, report: &MigrationReport, actor: &Identity) -> Result<(), GitError> {
    let branches = report
        .branches
        .iter()
        .map(|r| Ok((StreamName::new(&r.destination).map_err(|e| GitError::Unsupported(e.to_string()))?, r.changeset)))
        .collect::<Result<Vec<_>, GitError>>()?;
    let tags = report.tags.iter().map(|r| (r.destination.clone(), r.changeset)).collect::<Vec<_>>();
    repo.adopt_imported_refs(&branches, &tags, actor)?;
    Ok(())
}
fn check_source(dir: &std::path::Path) -> Result<(), GitError> {
    if text(git(dir, &["rev-parse", "--is-shallow-repository"])?)?.trim() == "true" {
        return Err(GitError::Unsupported(
            "A shallow clone is incomplete. Fetch the full history before migrating.".into(),
        ));
    }
    let count = text(git(dir, &["rev-list", "--count", "--all"])?)?
        .trim()
        .parse::<usize>()
        .map_err(|_| GitError::Parse("invalid Git commit count".into()))?;
    if count > 100_000 {
        return Err(GitError::Unsupported(
            "This migration is limited to 100,000 commits; split the migration before importing.".into(),
        ));
    }
    Ok(())
}
/// Migrate all branches and tags into an empty AgentGit repository.
/// Shared commits are converted once and all destination refs commit atomically.
/// Original Git IDs, signatures, hosting issues and review metadata need a source archive.
pub fn migrate(repo: &Repository, options: &MigrationOptions) -> Result<MigrationReport, GitError> {
    check_source(&options.git_dir)?;
    let (refs, other_refs) = source_refs(&options.git_dir)?;
    let branches: Vec<_> = refs.iter().filter(|r| !r.tag).collect();
    if branches.is_empty() {
        return Err(GitError::Unsupported("The source has no branches to migrate.".into()));
    }
    let default_branch = source_default_branch(&options.git_dir)?;
    let revisions: Vec<_> = refs.iter().map(|r| r.commit.clone()).collect();
    let staged = stage(repo, &options.git_dir, &revisions, &options.actor)?;
    let report = report_for(staged, &refs, default_branch, other_refs);
    if report.needs_attention() && !options.allow_lossy {
        return Err(GitError::Unsupported(format!(
            "Migration needs an explicit loss decision: {} skipped paths, {} submodules, {} repaired commit texts, {} LFS pointers and {} other refs. Review the source and use --allow-lossy only if these limitations are acceptable.",
            report.skipped_paths.len(),
            report.skipped_submodules,
            report.lossy_commits,
            report.lfs_pointers,
            report.other_refs.len()
        )));
    }
    adopt_report(repo, &report, &options.actor)?;
    Ok(report)
}

/// Unpack a self-contained Git bundle into a private mirror, with network protocols disabled.
pub fn unpack_bundle(bundle: &std::path::Path, mirror: &std::path::Path) -> Result<(), GitError> {
    let parent = mirror.parent().ok_or_else(|| GitError::Unsupported("mirror needs a parent directory".into()))?;
    let hooks = parent.join("empty-hooks");
    std::fs::create_dir_all(&hooks)?;
    let hooks_option = format!("core.hooksPath={}", hooks.display());
    let bundle = bundle.to_str().ok_or_else(|| GitError::Unsupported("bundle path is not UTF-8".into()))?;
    let mirror = mirror.to_str().ok_or_else(|| GitError::Unsupported("mirror path is not UTF-8".into()))?;
    git(
        parent,
        &[
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.file.allow=always",
            "-c",
            &hooks_option,
            "clone",
            "--mirror",
            "--",
            bundle,
            mirror,
        ],
    )?;
    Ok(())
}

/// Source default branch, with a deterministic fallback for bundles without HEAD.
pub fn source_default_branch(dir: &std::path::Path) -> Result<String, GitError> {
    let (refs, _) = source_refs(dir)?;
    let branches: Vec<_> = refs.iter().filter(|r| !r.tag).collect();
    if branches.is_empty() {
        return Err(GitError::Unsupported("The source has no branches".into()));
    }
    let original = git(dir, &["symbolic-ref", "--short", "HEAD"])
        .ok()
        .and_then(|v| String::from_utf8(v).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| branches.iter().any(|r| r.name == *s))
        .unwrap_or_else(|| branches.iter().find(|r| r.name == "main").unwrap_or(&branches[0]).name.clone());
    Ok(stream_name(&original))
}
