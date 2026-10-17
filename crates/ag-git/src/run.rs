use std::path::Path;
use std::process::{Command, Stdio};

use crate::GitError;

/// Runs `git -C <dir> <args>` and returns its standard output.
pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, GitError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(GitError::Failed {
            args: args.join(" "),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(out.stdout)
}

/// Turns arbitrary text into something valid inside an [`ag_types::Identity`] name.
pub(crate) fn sanitize_identity_name(s: &str) -> String {
    let clean: String =
        s.chars().map(|c| if c.is_ascii_alphanumeric() || "._@#+-".contains(c) { c } else { '-' }).take(100).collect();
    let clean = clean.trim_matches('-').to_string();
    if clean.is_empty() { "unknown".into() } else { clean }
}
