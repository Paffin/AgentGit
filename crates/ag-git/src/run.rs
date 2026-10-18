use std::path::Path;
use std::process::{Command, Stdio};

use crate::GitError;

/// The variables that point git at a repository, working tree, index or object store other than the one `-C`
/// names. A process started by a git hook (`ag git export` from a `post-commit` hook, say) inherits them, and
/// they win over `-C`: the bridge would read or write the hook's repository instead of the one it was given.
const REPOSITORY_ENV: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_PREFIX",
];

/// `git -C <dir>` as the bridge runs it: without the inherited variables that redirect git to another
/// repository, never prompting and never taking optional locks. Every git process of the bridge starts here.
pub(crate) fn git_command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir);
    for key in REPOSITORY_ENV {
        cmd.env_remove(key);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0").env("GIT_OPTIONAL_LOCKS", "0");
    cmd
}

/// Runs `git -C <dir> <args>` and returns its standard output.
pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, GitError> {
    let out = git_command(dir).args(args).stdin(Stdio::null()).output()?;
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
