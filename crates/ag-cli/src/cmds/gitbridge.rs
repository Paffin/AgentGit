//! `ag git import|export`.

use std::path::PathBuf;

use ag_git::{ExportOptions, ImportOptions};
use ag_types::StreamName;
use anyhow::{Result, anyhow};
use clap::Subcommand;
use serde_json::json;

use crate::ctx::Ctx;
use crate::exit;

#[derive(Subcommand, Debug)]
pub enum GitCmd {
    /// Import a Git repository's history into a new stream.
    Import {
        /// Path of the Git repository.
        path: PathBuf,
        /// Revision to import.
        #[arg(long, default_value = "HEAD")]
        rev: String,
        /// Stream to create for the imported history.
        #[arg(long, default_value = "imported")]
        stream: String,
    },
    /// Export a stream's history to a Git repository (created if missing).
    Export {
        /// Stream to export.
        stream: String,
        /// Destination Git repository.
        path: PathBuf,
        /// Branch name in Git.
        #[arg(long, default_value = "main")]
        branch: String,
    },
}

pub fn git(ctx: &Ctx, cmd: &GitCmd) -> Result<u8> {
    match cmd {
        GitCmd::Import { path, rev, stream } => {
            let rep = ag_git::import(
                &ctx.repo,
                &ImportOptions {
                    git_dir: path.clone(),
                    rev: rev.clone(),
                    stream: StreamName::new(stream.clone()).map_err(|e| anyhow!("{e}"))?,
                    actor: ctx.me.clone(),
                },
            )?;
            ctx.done(
                &json!({"commits": rep.commits, "head": rep.head, "blobs": rep.blobs, "skipped_paths": rep.skipped_paths, "skipped_submodules": rep.skipped_submodules, "lossy_commits": rep.lossy_commits}),
                || {
                    let mut s = format!("Imported {} commit(s), {} distinct file content(s) into stream '{stream}' (head {}).", rep.commits, rep.blobs, rep.head.short());
                    if rep.skipped_submodules > 0 {
                        s.push_str(&format!("\n  skipped {} submodule entr{}", rep.skipped_submodules, if rep.skipped_submodules == 1 { "y" } else { "ies" }));
                    }
                    if rep.lossy_commits > 0 {
                        s.push_str(&format!("\n  {} commit(s) had text that is not valid UTF-8; the invalid bytes were replaced by U+FFFD", rep.lossy_commits));
                    }
                    for (p, why) in rep.skipped_paths.iter().take(20) {
                        s.push_str(&format!("\n  skipped path {p:?}: {why}"));
                    }
                    if rep.skipped_paths.len() > 20 {
                        s.push_str(&format!("\n  ... and {} more skipped path(s)", rep.skipped_paths.len() - 20));
                    }
                    s
                },
            )?;
        }
        GitCmd::Export { stream, path, branch } => {
            let rep = ag_git::export(
                &ctx.repo,
                &ExportOptions { stream: stream.clone(), git_dir: path.clone(), branch: branch.clone() },
            )?;
            ctx.done(&json!({"commits": rep.commits, "branch": rep.branch}), || {
                format!(
                    "Exported {} commit(s) of '{stream}' to branch '{}' in {}",
                    rep.commits,
                    rep.branch,
                    path.display()
                )
            })?;
        }
    }
    Ok(exit::OK)
}

/// All-ref migration works outside an existing AgentGit repository.
#[derive(clap::Args, Debug)]
pub struct MigrateArgs {
    /// Full local Git clone, mirror or self-contained .bundle file.
    pub source: PathBuf,
    /// New destination directory. It must not exist.
    pub destination: PathBuf,
    /// Destination namespace, for example acme/webapp.
    #[arg(long)]
    pub namespace: String,
    /// Accept reported omissions, unresolved LFS pointers and unsupported refs.
    #[arg(long)]
    pub allow_lossy: bool,
}

fn create_bundle(source: &std::path::Path, bundle: &std::path::Path) -> Result<()> {
    use anyhow::bail;
    use std::{fs, process::Command};
    if source.is_file() {
        fs::copy(source, bundle)?;
    } else {
        let output = Command::new("git")
            .arg("-C")
            .arg(source)
            .args(["bundle", "create"])
            .arg(bundle)
            .arg("--all")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .env_remove("GIT_NAMESPACE")
            .output()?;
        if !output.status.success() {
            bail!("Could not create source bundle: {}", String::from_utf8_lossy(&output.stderr));
        }
    }
    Ok(())
}
fn save_archive(root: &std::path::Path, bundle: &std::path::Path, report: &ag_git::MigrationReport) -> Result<()> {
    let archive = root.join(".ag/import");
    std::fs::create_dir_all(&archive)?;
    std::fs::copy(bundle, archive.join("source.bundle"))?;
    std::fs::write(archive.join("report.json"), serde_json::to_vec_pretty(report)?)?;
    Ok(())
}
fn verify_migration(repo: &ag_integrate::Repository) -> Result<()> {
    use anyhow::bail;
    if !repo.fsck()?.is_ok() {
        bail!("Migrated repository failed verification; destination was not published.");
    }
    Ok(())
}
fn migration_options(
    mirror: &std::path::Path,
    namespace: ag_types::Namespace,
    actor: &ag_types::Identity,
) -> Result<ag_integrate::InitOptions> {
    Ok(ag_integrate::InitOptions {
        namespace,
        default_stream: StreamName::new(ag_git::source_default_branch(mirror)?).map_err(|e| anyhow!("{e}"))?,
        actor: actor.clone(),
        storage: None,
    })
}
fn build_migration(
    cli: &crate::Cli,
    args: &MigrateArgs,
    namespace: ag_types::Namespace,
    stage: &std::path::Path,
    bundle: &std::path::Path,
) -> Result<ag_git::MigrationReport> {
    use ag_integrate::Repository;
    let mirror = stage.join("mirror.git");
    ag_git::unpack_bundle(bundle, &mirror)?;
    let actor = crate::ctx::identity(cli)?;
    let root = stage.join("repository");
    let repo = Repository::init(&root, &migration_options(&mirror, namespace, &actor)?)?;
    let report = ag_git::migrate(
        &repo,
        &ag_git::MigrationOptions { git_dir: mirror, actor: actor.clone(), allow_lossy: args.allow_lossy },
    )?;
    verify_migration(&repo)?;
    save_archive(&root, bundle, &report)?;
    // Make the default branch immediately usable; an empty worktree would look like every file was deleted.
    repo.sync_workspace("default", &root, &actor)?;
    drop(repo);
    Ok(report)
}
fn publish_directory(root: &std::path::Path, destination: &std::path::Path) -> Result<()> {
    use anyhow::Context;
    use std::fs;
    // An exclusive reservation prevents replacing a concurrently created destination.
    fs::create_dir(destination).context("Destination was created by another process")?;
    if let Err(e) = (|| -> std::io::Result<()> {
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            fs::rename(entry.path(), destination.join(entry.file_name()))?;
        }
        Ok(())
    })() {
        let _ = fs::remove_dir(destination);
        return Err(e.into());
    }
    Ok(())
}
fn destination_stage(args: &MigrateArgs) -> Result<(PathBuf, tempfile::TempDir)> {
    use anyhow::bail;
    use std::fs;
    if args.destination.exists() {
        bail!("Destination already exists. Choose a new directory; existing work is never overwritten.");
    }
    let destination = std::path::absolute(&args.destination)?;
    let parent = destination.parent().ok_or_else(|| anyhow!("Destination needs a parent directory"))?;
    fs::create_dir_all(parent)?;
    let stage = tempfile::Builder::new().prefix(".agentgit-migration-").tempdir_in(parent)?;
    Ok((destination, stage))
}
pub fn migrate(cli: &crate::Cli, args: &MigrateArgs) -> Result<u8> {
    use ag_types::Namespace;
    use anyhow::Context;
    use std::fs;
    let namespace = Namespace::new(&args.namespace).map_err(|e| anyhow!("{e}"))?;
    let source = fs::canonicalize(&args.source).context("Cannot open the source clone or bundle")?;
    let (destination, stage) = destination_stage(args)?;
    let bundle = stage.path().join("source.bundle");
    create_bundle(&source, &bundle)?;
    let report = build_migration(cli, args, namespace, stage.path(), &bundle)?;
    publish_directory(&stage.path().join("repository"), &destination)?;
    crate::ctx::emit(cli.json, &report, || {
        format!(
            "Migrated {} commits, {} branches and {} tags into {}.\nOriginal source unchanged; archive and report: .ag/import/.\nNext: cd {} && ag status",
            report.commits,
            report.branches.len(),
            report.tags.len(),
            destination.display(),
            destination.display()
        )
    })?;
    Ok(exit::OK)
}
