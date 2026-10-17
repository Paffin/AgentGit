//! Fetching the candidate tree of a proposal and writing it to disk, without trusting the server.
//!
//! The worker is about to attest "this exact tree passes this check". So it does not take the server's word
//! for what the tree is:
//!
//! * the manifest the server sends is rebuilt into a tree and its identifier **recomputed**; it must equal
//!   the id the job was created for, otherwise nothing is checked;
//! * every blob is hash-verified by the client as it arrives;
//! * paths are validated (`RepoPath`), never written through a symlink, and symlink targets may not point
//!   outside the checkout;
//! * the total size is capped while downloading (the manifest does not carry sizes).

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, chown, lchown, symlink};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ag_client::{Client, ClientError};
use ag_object::ObjectKind;
use ag_protocol::{ErrorCode, v1};
use ag_tree::{Entry, Tree};
use ag_types::{BlobId, FileType, Limits, RepoPath, TreeId};
use futures_util::StreamExt;

/// Concurrent blob downloads.
const PARALLEL: usize = 8;
/// Blobs up to this size are remembered so identical small files are fetched once.
const CACHE_BELOW: usize = 64 * 1024;

/// What went wrong fetching or writing a candidate.
#[derive(Debug, thiserror::Error)]
pub enum CheckoutError {
    /// The proposal's candidate is no longer the tree the job was created for.
    #[error("the candidate is no longer the job's tree: {0}")]
    Stale(String),
    /// The server answered with an error.
    #[error("{0}")]
    Server(ClientError),
    /// The server's data is inconsistent, unsafe or too large.
    #[error("{0}")]
    Bad(String),
    /// A local file-system problem.
    #[error("cannot write the checkout: {0}")]
    Io(#[from] std::io::Error),
}

impl From<ClientError> for CheckoutError {
    fn from(e: ClientError) -> Self {
        if e.code() == Some(ErrorCode::StaleCandidate) {
            CheckoutError::Stale(e.to_string())
        } else {
            CheckoutError::Server(e)
        }
    }
}

/// The candidate manifest of `proposal`, verified to be exactly `expected`.
pub async fn fetch_manifest(
    client: &Client,
    repo: &str,
    proposal: &str,
    expected: &TreeId,
) -> Result<Tree, CheckoutError> {
    let mut entries: Vec<(RepoPath, Entry)> = Vec::new();
    let mut token = String::new();
    loop {
        let req = v1::GetCandidateRequest {
            repo: repo.into(),
            proposal: proposal.into(),
            expected_tree_id: expected.to_string(),
            page_size: 5000,
            page_token: token.clone(),
        };
        let page =
            client.retrying(|| async { client.execution().get_candidate(client.req(req.clone())).await }).await?;
        let page = page.into_inner();
        if page.tree_id != expected.to_string() {
            return Err(CheckoutError::Stale(format!("the server returned tree {}, not {expected}", page.tree_id)));
        }
        for e in page.entries {
            entries.push(entry_from_wire(&e)?);
        }
        if page.next_page_token.is_empty() {
            break;
        }
        token = page.next_page_token;
    }
    let tree = Tree::from_entries(entries, &Limits::default())
        .map_err(|e| CheckoutError::Bad(format!("the candidate manifest is invalid: {e}")))?;
    if tree.id() != *expected {
        return Err(CheckoutError::Bad(format!(
            "the manifest the server sent hashes to {}, not to the job's tree {expected}; refusing to check it",
            tree.id()
        )));
    }
    Ok(tree)
}

fn entry_from_wire(e: &v1::TreeEntry) -> Result<(RepoPath, Entry), CheckoutError> {
    let path =
        RepoPath::new(e.path.clone()).map_err(|err| CheckoutError::Bad(format!("bad path '{}': {err}", e.path)))?;
    let content: BlobId =
        e.blob_id.parse().map_err(|_| CheckoutError::Bad(format!("bad blob id '{}' for '{}'", e.blob_id, e.path)))?;
    let file_type = match e.file_type.as_str() {
        // The protocol says `regular`; servers before that was fixed said `file`.
        "regular" | "file" => FileType::Regular,
        "symlink" => FileType::Symlink,
        other => return Err(CheckoutError::Bad(format!("unknown file type '{other}' for '{}'", e.path))),
    };
    Ok((path, Entry { content, file_type, executable: e.executable }))
}

/// A materialized tree.
#[derive(Debug)]
pub struct Checkout {
    /// Number of entries written.
    pub files: usize,
    /// Total content bytes downloaded and written.
    pub bytes: u64,
}

/// Downloads every blob of `tree` and writes the tree below `dest` (which must exist and be empty).
///
/// With `owner`, every file, directory and link is given to that host user and group (the worker is root and
/// the command is not).
pub async fn write_tree(
    client: &Client,
    repo: &str,
    tree: &Tree,
    dest: &Path,
    owner: Option<(u32, u32)>,
    max_bytes: u64,
) -> Result<Checkout, CheckoutError> {
    if !fs::symlink_metadata(dest)?.file_type().is_dir() {
        return Err(CheckoutError::Bad(format!("{} is not a directory", dest.display())));
    }
    if let Some((uid, gid)) = owner {
        chown(dest, Some(uid), Some(gid))?;
    }
    let items: Vec<(RepoPath, Entry)> = tree.iter().map(|(p, e)| (p.clone(), *e)).collect();
    let cache: BlobCache = Mutex::new(HashMap::new());
    let cache = &cache;
    let mut bytes = 0u64;
    let mut files = 0usize;

    // Downloads run ahead of the writer, in order.
    let fetches = futures_util::stream::iter(items)
        .map(move |(path, entry)| fetch_blob(client, repo, cache, path, entry))
        .buffered(PARALLEL);
    tokio::pin!(fetches);

    while let Some(next) = fetches.next().await {
        let (path, entry, data) = next?;
        bytes = bytes.saturating_add(data.len() as u64);
        if bytes > max_bytes {
            return Err(CheckoutError::Bad(format!(
                "the candidate is larger than this worker's limit of {max_bytes} bytes"
            )));
        }
        write_entry(dest, &path, &entry, &data, owner)?;
        files += 1;
    }
    Ok(Checkout { files, bytes })
}

type BlobCache = Mutex<HashMap<BlobId, Arc<Vec<u8>>>>;

/// One verified blob (small ones are remembered, so identical small files are fetched once).
async fn fetch_blob(
    client: &Client,
    repo: &str,
    cache: &BlobCache,
    path: RepoPath,
    entry: Entry,
) -> Result<(RepoPath, Entry, Arc<Vec<u8>>), CheckoutError> {
    let id = entry.content;
    let cached = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned();
    if let Some(c) = cached {
        return Ok((path, entry, c));
    }
    let raw = client.get_object(repo, &id.object_id()).await?;
    if raw.kind != ObjectKind::Blob {
        return Err(CheckoutError::Bad(format!("object {id} for '{path}' is not a blob")));
    }
    let data = Arc::new(raw.payload);
    if data.len() <= CACHE_BELOW {
        cache.lock().unwrap_or_else(|e| e.into_inner()).insert(id, data.clone());
    }
    Ok((path, entry, data))
}

fn write_entry(
    root: &Path,
    path: &RepoPath,
    entry: &Entry,
    data: &[u8],
    owner: Option<(u32, u32)>,
) -> Result<(), CheckoutError> {
    let target = safe_target(root, path, owner)?;
    match entry.file_type {
        FileType::Symlink => {
            let t = std::str::from_utf8(data)
                .map_err(|_| CheckoutError::Bad(format!("symlink '{path}' has a target that is not UTF-8")))?;
            check_link_target(path, t)?;
            if fs::symlink_metadata(&target).is_ok() {
                fs::remove_file(&target)?;
            }
            symlink(t, &target)?;
            if let Some((uid, gid)) = owner {
                lchown(&target, Some(uid), Some(gid))?;
            }
        }
        FileType::Regular => {
            // Created exclusively: nothing may already be there, least of all a symlink to follow.
            use std::io::Write as _;
            let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&target)?;
            f.write_all(data)?;
            drop(f);
            fs::set_permissions(&target, fs::Permissions::from_mode(if entry.executable { 0o755 } else { 0o644 }))?;
            if let Some((uid, gid)) = owner {
                chown(&target, Some(uid), Some(gid))?;
            }
        }
    }
    Ok(())
}

/// Resolves `rel` below `root`, creating directories as needed and refusing to traverse anything that is
/// not a real directory.
fn safe_target(root: &Path, rel: &RepoPath, owner: Option<(u32, u32)>) -> Result<PathBuf, CheckoutError> {
    let mut cur = root.to_path_buf();
    let comps: Vec<&str> = rel.components().collect();
    for (i, comp) in comps.iter().enumerate() {
        cur.push(comp);
        if i + 1 == comps.len() {
            break;
        }
        match fs::symlink_metadata(&cur) {
            Ok(md) if md.file_type().is_dir() => {}
            Ok(_) => {
                return Err(CheckoutError::Bad(format!(
                    "'{}' exists and is not a directory; refusing to write through it",
                    cur.display()
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&cur)?;
                fs::set_permissions(&cur, fs::Permissions::from_mode(0o755))?;
                if let Some((uid, gid)) = owner {
                    chown(&cur, Some(uid), Some(gid))?;
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(cur)
}

/// A symlink target must stay inside the checkout.
fn check_link_target(link: &RepoPath, target: &str) -> Result<(), CheckoutError> {
    let bad = |why: &str| CheckoutError::Bad(format!("symlink '{link}' -> '{target}' {why}"));
    if target.is_empty() || target.contains('\0') {
        return Err(bad("has an empty or invalid target"));
    }
    if target.starts_with('/') || target.contains('\\') {
        return Err(bad("has an absolute or non-portable target"));
    }
    let mut depth = link.components().count() as i64 - 1;
    for comp in target.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return Err(bad("escapes the checkout"));
                }
            }
            _ => depth += 1,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> RepoPath {
        RepoPath::new(s).unwrap()
    }

    #[test]
    fn symlink_targets_cannot_leave_the_checkout() {
        assert!(check_link_target(&p("a/link"), "../b").is_ok());
        assert!(check_link_target(&p("a/link"), "b/c").is_ok());
        assert!(check_link_target(&p("link"), "../x").is_err());
        assert!(check_link_target(&p("a/link"), "../../x").is_err());
        assert!(check_link_target(&p("a/link"), "/etc/passwd").is_err());
        assert!(check_link_target(&p("a/link"), "").is_err());
        assert!(check_link_target(&p("a/link"), "..\\x").is_err());
    }

    #[test]
    fn writes_never_go_through_a_symlink_or_over_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::os::unix::fs::symlink(outside.path(), root.join("evil")).unwrap();
        let e = Entry {
            content: BlobId::new(ag_types::ObjectId::from_bytes([1; 32])),
            file_type: FileType::Regular,
            executable: false,
        };
        // a path whose directory component is a symlink
        let err = write_entry(root, &p("evil/x"), &e, b"data", None).unwrap_err();
        assert!(matches!(err, CheckoutError::Bad(_)), "{err}");
        assert!(!outside.path().join("x").exists());
        // a file that is already there (even a symlink to somewhere else) is not overwritten
        std::os::unix::fs::symlink(outside.path().join("target"), root.join("f")).unwrap();
        assert!(write_entry(root, &p("f"), &e, b"data", None).is_err());
        assert!(!outside.path().join("target").exists());
        // the normal case
        write_entry(root, &p("d/e/ok.sh"), &Entry { executable: true, ..e }, b"#!/bin/sh\n", None).unwrap();
        let md = fs::metadata(root.join("d/e/ok.sh")).unwrap();
        assert_eq!(md.permissions().mode() & 0o777, 0o755);
        assert_eq!(fs::read(root.join("d/e/ok.sh")).unwrap(), b"#!/bin/sh\n");
    }

    #[test]
    fn manifest_entries_are_validated() {
        let ok = v1::TreeEntry {
            path: "src/lib.rs".into(),
            blob_id: "ab".repeat(32),
            size: 0,
            file_type: "regular".into(),
            executable: false,
        };
        assert!(entry_from_wire(&ok).is_ok());
        for bad in [
            v1::TreeEntry { path: "../etc/passwd".into(), ..ok.clone() },
            v1::TreeEntry { path: "/abs".into(), ..ok.clone() },
            v1::TreeEntry { blob_id: "zz".into(), ..ok.clone() },
            v1::TreeEntry { file_type: "fifo".into(), ..ok.clone() },
        ] {
            assert!(entry_from_wire(&bad).is_err(), "{bad:?}");
        }
    }
}
