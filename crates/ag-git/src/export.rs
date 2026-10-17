use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use ag_changeset::Op;
use ag_dag::ancestors;
use ag_integrate::Repository;
use ag_types::{ChangesetId, FileType};

use crate::GitError;
use crate::run::git;

/// Options for [`export`].
#[derive(Clone, Debug)]
pub struct ExportOptions {
    /// Stream to export.
    pub stream: String,
    /// Destination Git repository; created (with `git init`) if it does not exist.
    pub git_dir: PathBuf,
    /// Branch name to create in Git.
    pub branch: String,
}

/// What an export did.
#[derive(Clone, Debug)]
pub struct ExportReport {
    /// Commits written.
    pub commits: usize,
    /// Branch that points at the exported head.
    pub branch: String,
}

fn git_ident(identity: &str) -> (String, String) {
    let name: String = identity.chars().filter(|c| !matches!(c, '<' | '>' | '\n')).collect();
    let email = format!("{}@agentgit.invalid", name.replace(['#', ':'], "."));
    (name, email)
}

/// A path as `git fast-import` wants it. Paths are written verbatim, except that one starting with a double
/// quote would be read as a C-style quoted string (dropping the quotes, or failing the whole import), so those
/// are quoted explicitly. A [`ag_types::RepoPath`] cannot contain a backslash, so `"` is the only character
/// that needs escaping.
#[doc(hidden)]
pub fn fast_import_path(path: &str) -> String {
    if path.starts_with('"') { format!("\"{}\"", path.replace('"', "\\\"")) } else { path.to_string() }
}

fn data(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(format!("data {}\n", bytes.len()).as_bytes());
    buf.extend_from_slice(bytes);
    buf.push(b'\n');
}

/// Exports the history of a stream to a Git repository.
pub fn export(repo: &Repository, opts: &ExportOptions) -> Result<ExportReport, GitError> {
    let head = repo.resolve_rev(&opts.stream)?;
    let fresh = !opts.git_dir.join(".git").exists() && !opts.git_dir.join("HEAD").exists();
    if fresh {
        std::fs::create_dir_all(&opts.git_dir)?;
        git(&opts.git_dir, &["init", "-q"])?;
    }
    if opts.branch.is_empty()
        || opts.branch.starts_with('-')
        || opts.branch.contains(['\n', ' ', '~', '^', ':', '?', '*', '[', '\\'])
        || opts.branch.contains("..")
    {
        return Err(GitError::Unsupported(format!("'{}' is not a usable branch name", opts.branch)));
    }

    // Parents before children: ascending generation.
    let mut order = ancestors(&repo.graph(), &[head], 5_000_000).map_err(|e| GitError::Unsupported(e.to_string()))?;
    order.reverse();
    let mut marks: std::collections::HashMap<ChangesetId, usize> = std::collections::HashMap::new();

    let mut child = Command::new("git")
        .arg("-C")
        .arg(&opts.git_dir)
        .args(["fast-import", "--quiet", "--date-format=raw"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("piped");
    let branch_ref = format!("refs/heads/{}", opts.branch);

    let write_all = (|| -> Result<(), GitError> {
        for (i, id) in order.iter().enumerate() {
            let cs = repo.changeset(id)?;
            let mark = i + 1;
            let (name, email) = git_ident(cs.author.as_str());
            let mut buf = Vec::new();
            if cs.parents.is_empty() && i > 0 {
                buf.extend_from_slice(format!("reset {branch_ref}\n").as_bytes());
            }
            buf.extend_from_slice(format!("commit {branch_ref}\nmark :{mark}\n").as_bytes());
            let ts = cs.timestamp.as_millis().div_euclid(1000);
            buf.extend_from_slice(
                format!("author {name} <{email}> {ts} +0000\ncommitter {name} <{email}> {ts} +0000\n").as_bytes(),
            );
            data(&mut buf, cs.message.as_bytes());
            if let Some(first) = cs.parents.first() {
                let m = marks.get(first).ok_or_else(|| GitError::Parse("parent not exported yet".into()))?;
                buf.extend_from_slice(format!("from :{m}\n").as_bytes());
            }
            for p in cs.parents.iter().skip(1) {
                let m = marks.get(p).ok_or_else(|| GitError::Parse("parent not exported yet".into()))?;
                buf.extend_from_slice(format!("merge :{m}\n").as_bytes());
            }
            for op in &cs.ops {
                match op {
                    Op::Delete { path } => {
                        buf.extend_from_slice(format!("D {}\n", fast_import_path(path.as_str())).as_bytes())
                    }
                    Op::Put { path, content, file_type, executable } => {
                        let mode = match (file_type, executable) {
                            (FileType::Symlink, _) => "120000",
                            (FileType::Regular, true) => "100755",
                            (FileType::Regular, false) => "100644",
                        };
                        buf.extend_from_slice(
                            format!("M {mode} inline {}\n", fast_import_path(path.as_str())).as_bytes(),
                        );
                        data(&mut buf, &repo.blob(content)?);
                    }
                }
            }
            buf.push(b'\n');
            stdin.write_all(&buf)?;
            marks.insert(*id, mark);
        }
        Ok(())
    })();
    drop(stdin);
    let out = child.wait_with_output()?;
    write_all?;
    if !out.status.success() {
        return Err(GitError::Failed {
            args: "fast-import".into(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    if fresh {
        git(&opts.git_dir, &["symbolic-ref", "HEAD", &branch_ref])?;
        git(&opts.git_dir, &["reset", "--hard", "-q"])?;
    }
    Ok(ExportReport { commits: order.len(), branch: opts.branch.clone() })
}
