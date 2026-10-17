//! Moving data between a working directory and the repository.
//!
//! * [`scan`] turns a directory into a [`Tree`], storing file contents as blobs.
//! * [`checkout_tree`] / [`update_dir`] write trees back to disk.
//!
//! Writing is where repositories get attacked, so it is strict:
//!
//! * every parent directory must be a real directory — never a symlink — so a
//!   tree cannot write through a link planted earlier;
//! * symlink targets must stay inside the checkout (no absolute targets, no
//!   `..` that climbs above the root);
//! * files are written to a temporary name and renamed into place;
//! * contents are read through the verifying object store, so a damaged blob
//!   is an error and never silently materialized.

use std::fs;
use std::path::{Path, PathBuf};

use ag_changeset::Op;
use ag_object::hash_blob;
use ag_tree::{Entry, Tree};
use ag_types::{FileType, RepoPath};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::{Error, Repository, Result};

/// Components that are never scanned or written.
const ALWAYS_IGNORED: [&str; 2] = [".ag", ".git"];

/// Ignore rules from `.agignore`: one glob per line; `#` starts a comment; a
/// pattern without `/` matches at any depth; a trailing `/` is accepted.
#[derive(Clone, Debug)]
pub struct IgnoreRules {
    set: GlobSet,
}

impl IgnoreRules {
    /// No rules besides the built-in `.ag` and `.git`.
    pub fn none() -> Self {
        Self { set: GlobSet::empty() }
    }

    /// Parses rule text.
    pub fn parse(text: &str) -> Result<Self> {
        let mut b = GlobSetBuilder::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let pat = line.trim_end_matches('/').trim_start_matches('/');
            let anchored = line.starts_with('/') || pat.contains('/');
            let mut add = |g: &str| -> Result<()> {
                let glob = GlobBuilder::new(g)
                    .literal_separator(true)
                    .build()
                    .map_err(|e| Error::Invalid(format!("bad ignore pattern '{line}': {e}")))?;
                b.add(glob);
                Ok(())
            };
            add(pat)?;
            add(&format!("{pat}/**"))?;
            if !anchored {
                add(&format!("**/{pat}"))?;
                add(&format!("**/{pat}/**"))?;
            }
        }
        Ok(Self { set: b.build().map_err(|e| Error::Invalid(e.to_string()))? })
    }

    /// Loads `<dir>/.agignore` if present.
    pub fn load(dir: &Path) -> Result<Self> {
        match fs::read_to_string(dir.join(".agignore")) {
            Ok(t) => Self::parse(&t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::none()),
            Err(e) => Err(e.into()),
        }
    }

    /// True if the relative path is ignored.
    pub fn is_ignored(&self, rel: &str) -> bool {
        rel.split('/').any(|c| ALWAYS_IGNORED.iter().any(|a| c.eq_ignore_ascii_case(a))) || self.set.is_match(rel)
    }
}

/// What a scan did not record.
#[derive(Clone, Debug, Default)]
pub struct ScanReport {
    /// Special files (sockets, FIFOs, devices) that were skipped.
    pub skipped: Vec<String>,
}

/// Reads a directory into a tree. With `store`, file contents are written to
/// the object store; without it they are only hashed (for `status`).
pub fn scan(repo: &Repository, dir: &Path, rules: &IgnoreRules, store: bool) -> Result<(Tree, ScanReport)> {
    let mut entries: Vec<(RepoPath, Entry)> = Vec::new();
    let mut report = ScanReport::default();
    let mut stack: Vec<(PathBuf, String)> = vec![(dir.to_path_buf(), String::new())];
    while let Some((abs, rel)) = stack.pop() {
        let mut children: Vec<_> = fs::read_dir(&abs)?.collect::<std::io::Result<Vec<_>>>()?;
        children.sort_by_key(|c| c.file_name());
        for child in children {
            let name = child.file_name();
            let name = name.to_str().ok_or_else(|| Error::Invalid(format!("non-UTF-8 file name in {}", abs.display())))?.to_string();
            let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if rules.is_ignored(&child_rel) {
                continue;
            }
            let md = fs::symlink_metadata(child.path())?;
            let ft = md.file_type();
            if ft.is_dir() {
                stack.push((child.path(), child_rel));
            } else if ft.is_symlink() {
                let target = fs::read_link(child.path())?;
                let target = target.to_str().ok_or_else(|| Error::Invalid(format!("non-UTF-8 symlink target at '{child_rel}'")))?;
                let path = record_path(&child_rel)?;
                let content = if store { repo.put_blob(target.as_bytes().to_vec())? } else { hash_blob(target.as_bytes()) };
                entries.push((path, Entry { content, file_type: FileType::Symlink, executable: false }));
            } else if ft.is_file() {
                if md.len() > repo.limits().max_blob_size {
                    return Err(Error::Invalid(format!("'{child_rel}' is {} bytes, over the {} byte limit", md.len(), repo.limits().max_blob_size)));
                }
                let path = record_path(&child_rel)?;
                let data = fs::read(child.path())?;
                let content = if store { repo.put_blob(data)? } else { hash_blob(&data) };
                entries.push((path, Entry { content, file_type: FileType::Regular, executable: is_executable(&md) }));
            } else {
                report.skipped.push(child_rel);
            }
        }
    }
    let tree = Tree::from_entries(entries, repo.limits())?;
    Ok((tree, report))
}

fn record_path(rel: &str) -> Result<RepoPath> {
    RepoPath::new(rel).map_err(|e| Error::Invalid(format!("cannot record '{rel}': {e} (add it to .agignore to skip it)")))
}

#[cfg(unix)]
fn is_executable(md: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    md.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_md: &fs::Metadata) -> bool {
    false
}

/// Checks that a symlink target, placed at `link_path`, cannot point outside
/// the checkout root.
pub fn check_link_target(link_path: &RepoPath, target: &str) -> Result<()> {
    if target.is_empty() || target.contains('\0') {
        return Err(Error::UnsafePath(format!("symlink '{link_path}' has an empty or invalid target")));
    }
    if target.starts_with('/') || target.contains('\\') {
        return Err(Error::UnsafePath(format!("symlink '{link_path}' has an absolute or non-portable target '{target}'")));
    }
    // Depth of the directory holding the link, relative to the root.
    let mut depth = link_path.components().count() as i64 - 1;
    for comp in target.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return Err(Error::UnsafePath(format!("symlink '{link_path}' -> '{target}' escapes the repository root")));
                }
            }
            _ => depth += 1,
        }
    }
    Ok(())
}

/// Resolves `rel` under `root`, creating missing parent directories and
/// refusing to traverse anything that is not a real directory.
fn safe_target(root: &Path, rel: &RepoPath) -> Result<PathBuf> {
    let mut cur = root.to_path_buf();
    let comps: Vec<&str> = rel.components().collect();
    for (i, comp) in comps.iter().enumerate() {
        cur.push(comp);
        let last = i + 1 == comps.len();
        match fs::symlink_metadata(&cur) {
            Ok(md) if md.file_type().is_dir() && !last => {}
            Ok(md) if last && !md.file_type().is_dir() => {}
            Ok(md) if md.file_type().is_symlink() && !last => {
                return Err(Error::UnsafePath(format!("'{}' is a symlink; refusing to write through it", cur.display())));
            }
            Ok(_) => {
                return Err(Error::UnsafePath(format!("'{}' exists and has the wrong type", cur.display())));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if !last {
                    fs::create_dir(&cur)?;
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(cur)
}

fn write_entry(repo: &Repository, root: &Path, path: &RepoPath, entry: &Entry) -> Result<()> {
    let target = safe_target(root, path)?;
    let data = repo.blob(&entry.content)?;
    match entry.file_type {
        FileType::Symlink => {
            let t = std::str::from_utf8(&data).map_err(|_| Error::UnsafePath(format!("symlink '{path}' target is not UTF-8")))?;
            check_link_target(path, t)?;
            match fs::symlink_metadata(&target) {
                Ok(_) => fs::remove_file(&target)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(t, &target)?;
            #[cfg(not(unix))]
            return Err(Error::Invalid("symlinks are not supported on this platform".into()));
        }
        FileType::Regular => {
            let dir = target.parent().expect("target has a parent");
            let tmp = dir.join(format!(".ag-tmp-{}", ag_crypto::random_id("w")?));
            fs::write(&tmp, &data)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&tmp, fs::Permissions::from_mode(if entry.executable { 0o755 } else { 0o644 }))?;
            }
            fs::rename(&tmp, &target)?;
        }
    }
    Ok(())
}

fn remove_entry(root: &Path, path: &RepoPath) -> Result<()> {
    let mut abs = root.to_path_buf();
    abs.extend(path.components());
    match fs::symlink_metadata(&abs) {
        Ok(md) if !md.file_type().is_dir() => fs::remove_file(&abs)?,
        Ok(_) => return Err(Error::UnsafePath(format!("'{}' is a directory, expected a file", abs.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    }
    // Prune directories that became empty, but never the root.
    let mut dir = abs.parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        if d == root || fs::remove_dir(&d).is_err() {
            break;
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    Ok(())
}

/// Writes every entry of `tree` below `dest` (which must exist).
pub fn checkout_tree(repo: &Repository, tree: &Tree, dest: &Path) -> Result<()> {
    if !fs::symlink_metadata(dest)?.file_type().is_dir() {
        return Err(Error::UnsafePath(format!("checkout destination '{}' is not a directory", dest.display())));
    }
    for (path, entry) in tree.iter() {
        write_entry(repo, dest, path, entry)?;
    }
    Ok(())
}

/// Moves a directory that currently holds `old` to `new` by applying only the
/// difference. The caller must have checked that the directory is clean.
pub fn update_dir(repo: &Repository, old: &Tree, new: &Tree, dest: &Path) -> Result<()> {
    let ops = old.diff(new);
    // Deletions first, so a file can be replaced by a directory of the same name.
    for op in &ops {
        if let Op::Delete { path } = op {
            remove_entry(dest, path)?;
        }
    }
    for op in &ops {
        if let Op::Put { path, content, file_type, executable } = op {
            write_entry(repo, dest, path, &Entry { content: *content, file_type: *file_type, executable: *executable })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
