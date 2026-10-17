use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use ag_changeset::{Changeset, Op};
use ag_integrate::Repository;
use ag_types::{BlobId, ChangesetId, FileType, Identity, RepoPath, StreamName, Timestamp};

use crate::GitError;
use crate::run::{git, sanitize_identity_name};

/// Options for [`import`].
#[derive(Clone, Debug)]
pub struct ImportOptions {
    /// Path of the Git repository (working tree or bare).
    pub git_dir: PathBuf,
    /// Revision whose history to import (default `HEAD`).
    pub rev: String,
    /// Stream that will point at the imported history (must not have history yet).
    pub stream: StreamName,
    /// Who performs the import.
    pub actor: Identity,
}

/// What an import did.
#[derive(Clone, Debug)]
pub struct ImportReport {
    /// Commits imported.
    pub commits: usize,
    /// The imported head.
    pub head: ChangesetId,
    /// Paths that could not be represented, with the reason.
    pub skipped_paths: Vec<(String, String)>,
    /// Submodule entries that were skipped.
    pub skipped_submodules: usize,
    /// Distinct file contents stored.
    pub blobs: usize,
}

struct CatFile {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl CatFile {
    fn start(dir: &Path) -> Result<Self, GitError> {
        let mut child = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        Ok(Self { child, stdin, stdout })
    }

    fn blob(&mut self, sha: &str) -> Result<Vec<u8>, GitError> {
        writeln!(self.stdin, "{sha}")?;
        self.stdin.flush()?;
        let mut header = String::new();
        self.stdout.read_line(&mut header)?;
        let parts: Vec<&str> = header.split_whitespace().collect();
        if parts.len() != 3 || parts[1] != "blob" {
            return Err(GitError::Parse(format!("cat-file said '{}' for {sha}", header.trim())));
        }
        let size: usize = parts[2].parse().map_err(|_| GitError::Parse(format!("bad size in '{}'", header.trim())))?;
        let mut data = vec![0u8; size];
        self.stdout.read_exact(&mut data)?;
        let mut nl = [0u8; 1];
        self.stdout.read_exact(&mut nl)?;
        Ok(data)
    }
}

impl Drop for CatFile {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct RawChange {
    old_mode: String,
    new_mode: String,
    new_sha: String,
    path: String,
}

/// Parses `git diff-tree --raw -z` output.
fn parse_raw(out: &[u8]) -> Result<Vec<RawChange>, GitError> {
    let mut fields = out.split(|b| *b == 0).filter(|f| !f.is_empty());
    let mut changes = Vec::new();
    while let Some(meta) = fields.next() {
        let meta = std::str::from_utf8(meta).map_err(|_| GitError::Parse("non-UTF-8 diff header".into()))?;
        let meta =
            meta.strip_prefix(':').ok_or_else(|| GitError::Parse(format!("unexpected diff-tree line '{meta}'")))?;
        let p: Vec<&str> = meta.split(' ').collect();
        if p.len() != 5 {
            return Err(GitError::Parse(format!("bad diff-tree header '{meta}'")));
        }
        let path = fields.next().ok_or_else(|| GitError::Parse("diff-tree ended after a header".into()))?;
        changes.push(RawChange {
            old_mode: p[0].to_string(),
            new_mode: p[1].to_string(),
            new_sha: p[3].to_string(),
            // Non-UTF-8 paths cannot be recorded; keep a lossy name for the report.
            path: String::from_utf8_lossy(path).into_owned(),
        });
        if std::str::from_utf8(path).is_err() {
            let last = changes.last_mut().expect("just pushed");
            last.path = format!("\u{0}invalid:{}", last.path);
        }
    }
    Ok(changes)
}

struct Commit {
    sha: String,
    parents: Vec<String>,
    author_name: String,
    author_email: String,
    time: i64,
    message: String,
}

fn list_commits(dir: &Path, rev: &str) -> Result<Vec<Commit>, GitError> {
    let out =
        git(dir, &["log", "--topo-order", "--reverse", "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%B%x1e", rev, "--"])?;
    let text = String::from_utf8_lossy(&out);
    let mut commits = Vec::new();
    for rec in text.split('\u{1e}') {
        let rec = rec.trim_start_matches('\n');
        if rec.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.splitn(6, '\u{1f}').collect();
        if f.len() != 6 {
            return Err(GitError::Parse(format!("unexpected log record with {} fields", f.len())));
        }
        commits.push(Commit {
            sha: f[0].to_string(),
            parents: f[1].split_whitespace().map(str::to_string).collect(),
            author_name: f[2].to_string(),
            author_email: f[3].to_string(),
            time: f[4].parse().map_err(|_| GitError::Parse(format!("bad timestamp '{}'", f[4])))?,
            message: f[5].trim_end().to_string(),
        });
    }
    Ok(commits)
}

/// Imports the history reachable from `opts.rev` as a new stream.
pub fn import(repo: &Repository, opts: &ImportOptions) -> Result<ImportReport, GitError> {
    git(&opts.git_dir, &["rev-parse", "--git-dir"])?;
    let commits = list_commits(&opts.git_dir, &opts.rev)?;
    if commits.is_empty() {
        return Err(GitError::Unsupported(format!("'{}' has no commits", opts.rev)));
    }
    let mut cat = CatFile::start(&opts.git_dir)?;
    let mut ids: HashMap<String, ChangesetId> = HashMap::new();
    let mut blob_cache: HashMap<String, BlobId> = HashMap::new();
    let mut skipped_paths: Vec<(String, String)> = Vec::new();
    let mut skipped_submodules = 0usize;

    for c in &commits {
        let raw = match c.parents.first() {
            None => git(
                &opts.git_dir,
                &["diff-tree", "-r", "-z", "--raw", "--no-renames", "--no-commit-id", "--root", &c.sha],
            )?,
            Some(p) => git(&opts.git_dir, &["diff-tree", "-r", "-z", "--raw", "--no-renames", p, &c.sha])?,
        };
        let mut ops: Vec<Op> = Vec::new();
        for ch in parse_raw(&raw)? {
            let absent = |m: &str| m == "000000";
            let gitlink = |m: &str| m == "160000";
            let path = if let Some(rest) = ch.path.strip_prefix('\u{0}') {
                skipped_paths.push((rest.to_string(), "path is not valid UTF-8".into()));
                continue;
            } else {
                match RepoPath::new(ch.path.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        skipped_paths.push((ch.path.clone(), e.to_string()));
                        continue;
                    }
                }
            };
            if absent(&ch.new_mode) {
                if gitlink(&ch.old_mode) {
                    skipped_submodules += 1;
                } else {
                    ops.push(Op::Delete { path });
                }
                continue;
            }
            if gitlink(&ch.new_mode) {
                skipped_submodules += 1;
                if !absent(&ch.old_mode) && !gitlink(&ch.old_mode) {
                    ops.push(Op::Delete { path });
                }
                continue;
            }
            let (file_type, executable) = match ch.new_mode.as_str() {
                "120000" => (FileType::Symlink, false),
                "100755" => (FileType::Regular, true),
                "100644" | "100664" => (FileType::Regular, false),
                other => {
                    skipped_paths.push((ch.path.clone(), format!("unsupported git mode {other}")));
                    continue;
                }
            };
            let content = match blob_cache.get(&ch.new_sha) {
                Some(b) => *b,
                None => {
                    let data = cat.blob(&ch.new_sha)?;
                    let b = repo.put_blob(data).map_err(GitError::Engine)?;
                    blob_cache.insert(ch.new_sha.clone(), b);
                    b
                }
            };
            ops.push(Op::Put { path, content, file_type, executable });
        }

        let parents = c
            .parents
            .iter()
            .map(|p| {
                ids.get(p)
                    .copied()
                    .ok_or_else(|| GitError::Parse(format!("parent {p} was not imported before {}", c.sha)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let who = if c.author_email.is_empty() { &c.author_name } else { &c.author_email };
        let author = Identity::new(format!("user:{}", sanitize_identity_name(who)))
            .map_err(|e| GitError::Parse(e.to_string()))?;
        let mut message = c.message.clone();
        if message.len() > 60_000 {
            message.truncate(60_000);
            message.push_str("\n[message truncated by the git importer]");
        }
        message.push_str(&format!("\n\nImported-From: git {}", c.sha));
        let cs = Changeset::build(
            repo.config().namespace.clone(),
            parents,
            author,
            Timestamp::from_millis(c.time * 1000),
            message,
            ops,
            repo.limits(),
        )
        .map_err(|e| GitError::Unsupported(format!("commit {}: {e}", c.sha)))?;
        let id = repo.put_changeset(&cs).map_err(GitError::Engine)?;
        ids.insert(c.sha.clone(), id);
    }

    let head = *ids.get(&commits.last().expect("non-empty").sha).expect("imported");
    let published = repo.publish(&head, &opts.actor, Some(&format!("git-import:{head}")))?;
    if published.state != ag_integrate::PublishState::Published {
        return Err(GitError::Unsupported(format!(
            "the imported history was rejected: {}",
            published.reason.unwrap_or_default()
        )));
    }
    repo.adopt_history(&opts.stream, &head, &opts.actor, &format!("import of git {}", opts.rev))?;
    Ok(ImportReport { commits: commits.len(), head, skipped_paths, skipped_submodules, blobs: blob_cache.len() })
}
