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
            ctx.out(
                &json!({"commits": rep.commits, "head": rep.head, "blobs": rep.blobs, "skipped_paths": rep.skipped_paths, "skipped_submodules": rep.skipped_submodules}),
                || {
                    let mut s = format!("Imported {} commit(s), {} distinct file content(s) into stream '{stream}' (head {}).", rep.commits, rep.blobs, rep.head.short());
                    if rep.skipped_submodules > 0 {
                        s.push_str(&format!("\n  skipped {} submodule entr{}", rep.skipped_submodules, if rep.skipped_submodules == 1 { "y" } else { "ies" }));
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
            ctx.out(&json!({"commits": rep.commits, "branch": rep.branch}), || {
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
