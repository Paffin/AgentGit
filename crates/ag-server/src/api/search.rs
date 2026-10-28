//! Literal repository search with an immutable cursor and bounded per-request work.

use ag_integrate::Repository;
use ag_protocol::v1;
use ag_tree::Tree;
use ag_types::RepoPath;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::require;
use crate::app::{App, Call};
use crate::authz::Cap;
use crate::error::{ApiError, ApiResult};

const FILE_BYTES: u64 = 1024 * 1024;
const PAGE_BYTES: u64 = 16 * FILE_BYTES;
const PAGE_FILES: u32 = 1000;

#[derive(Default, Deserialize, Serialize)]
struct Cursor {
    binding: String,
    path: String,
    // Zero means the whole file was scanned; otherwise resume after this line.
    line: u32,
}

fn decode_cursor(token: &str, binding: &str) -> ApiResult<Cursor> {
    if token.is_empty() {
        return Ok(Cursor { binding: binding.into(), ..Cursor::default() });
    }
    let invalid =
        || ApiError::invalid("page_token does not belong to this search; restart with the new revision or query");
    if token.len() > 16 * 1024 {
        return Err(invalid());
    }
    let bytes = URL_SAFE_NO_PAD.decode(token).map_err(|_| invalid())?;
    let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if cursor.binding != binding {
        return Err(invalid());
    }
    Ok(cursor)
}

fn encode_cursor(cursor: &Cursor) -> ApiResult<String> {
    let bytes = serde_json::to_vec(cursor).map_err(|e| ApiError::invalid(e.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Return the original UTF-8 byte index corresponding to a lowercase match.
fn find_match(line: &str, query: &str, case_sensitive: bool) -> Option<usize> {
    if case_sensitive {
        return line.find(query);
    }
    let folded = line.to_lowercase();
    let wanted = folded.find(query)?;
    let mut offset = 0;
    for (at, ch) in line.char_indices() {
        let len = ch.to_lowercase().map(char::len_utf8).sum::<usize>();
        if offset + len > wanted {
            return Some(at);
        }
        offset += len;
    }
    None
}

fn excerpt(line: &str, at: usize) -> String {
    let start = line[..at].char_indices().rev().nth(79).map_or(0, |(i, _)| i);
    let text: String = line[start..].chars().take(512).collect();
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(&text);
    if start + text.len() < line.len() {
        out.push('…');
    }
    out
}

fn validate_request(req: &v1::SearchTreeRequest) -> ApiResult<&str> {
    if req.query.is_empty() || req.query.len() > 256 || req.query.contains(['\n', '\r']) {
        return Err(ApiError::invalid(
            "query must be a non-empty literal of at most 256 UTF-8 bytes, without newlines",
        ));
    }
    let prefix = req.path_prefix.trim_matches('/');
    if !prefix.is_empty() {
        RepoPath::new(prefix).map_err(|e| ApiError::invalid(e.to_string()))?;
    }
    Ok(prefix)
}

fn revision_tree(repo: &Repository, expression: &str) -> ApiResult<(String, Tree)> {
    let expression = if expression.is_empty() { repo.default_stream().as_str() } else { expression };
    let empty = repo.meta().read(|tx| tx.stream(expression))?.is_some_and(|s| s.state.head.is_none());
    if empty {
        return Ok((String::new(), Tree::empty()));
    }
    let id = repo.resolve_rev(expression)?;
    Ok((id.to_string(), repo.tree_of(&id)?))
}

fn matches_prefix(path: &str, prefix: &str) -> bool {
    prefix.is_empty() || path == prefix || path.strip_prefix(prefix).is_some_and(|tail| tail.starts_with('/'))
}

fn after_cursor(path: &str, cursor: &Cursor) -> bool {
    path > cursor.path.as_str() || (path == cursor.path && cursor.line > 0)
}

struct Scan {
    output: v1::SearchTreeResponse,
    cursor: Cursor,
    query: String,
    case_sensitive: bool,
    limit: usize,
}

impl Scan {
    fn new(req: &v1::SearchTreeRequest, rev: String, tree_id: String, prefix: &str) -> ApiResult<Self> {
        let input = serde_json::to_vec(&(&req.repo, &tree_id, &req.query, req.case_sensitive, prefix))
            .map_err(|e| ApiError::invalid(e.to_string()))?;
        let binding = hex::encode(Sha256::digest(input));
        Ok(Self {
            cursor: decode_cursor(&req.page_token, &binding)?,
            query: if req.case_sensitive { req.query.clone() } else { req.query.to_lowercase() },
            case_sensitive: req.case_sensitive,
            limit: if req.page_size == 0 { 50 } else { req.page_size.min(100) } as usize,
            output: v1::SearchTreeResponse { rev, tree_id, ..Default::default() },
        })
    }

    fn complete_file(&mut self, path: &str) {
        self.cursor.path = path.into();
        self.cursor.line = 0;
    }

    fn scan_text(&mut self, path: &str, text: &str) -> bool {
        for (index, line) in text.lines().enumerate() {
            let number = (index + 1) as u32;
            if path == self.cursor.path && number <= self.cursor.line {
                continue;
            }
            if let Some(at) = find_match(line, &self.query, self.case_sensitive) {
                self.output.matches.push(v1::SearchMatch { path: path.into(), line: number, text: excerpt(line, at) });
                if self.output.matches.len() >= self.limit {
                    self.cursor.path = path.into();
                    self.cursor.line = number;
                    return true;
                }
            }
        }
        false
    }

    // True means this page reached a budget; the cursor then points to its last completed work.
    fn scan_file(&mut self, repo: &Repository, path: &str, entry: &ag_tree::Entry) -> ApiResult<bool> {
        if self.output.files_scanned >= PAGE_FILES {
            return Ok(true);
        }
        let length = repo
            .blob_len(&entry.content)?
            .ok_or_else(|| ApiError::new(ag_protocol::ErrorCode::ObjectMissing, "search encountered a missing blob"))?;
        if length > FILE_BYTES {
            self.output.skipped_large += 1;
            self.output.files_scanned += 1;
            self.complete_file(path);
            return Ok(false);
        }
        if self.output.scanned_bytes + length > PAGE_BYTES {
            return Ok(true);
        }
        let bytes = repo.blob(&entry.content)?;
        self.output.scanned_bytes += bytes.len() as u64;
        self.output.files_scanned += 1;
        if let Some(text) = std::str::from_utf8(&bytes).ok().filter(|text| !text.contains('\0')) {
            if self.scan_text(path, text) {
                return Ok(true);
            }
        } else {
            self.output.skipped_binary += 1;
        }
        self.complete_file(path);
        Ok(false)
    }
}

fn eligible(path: &str, prefix: &str, cursor: &Cursor) -> bool {
    matches_prefix(path, prefix) && after_cursor(path, cursor)
}

fn search(repo: &Repository, req: &v1::SearchTreeRequest) -> ApiResult<v1::SearchTreeResponse> {
    let prefix = validate_request(req)?;
    let (rev, tree) = revision_tree(repo, &req.rev)?;
    let mut scan = Scan::new(req, rev, tree.id().to_string(), prefix)?;
    for (path, entry) in tree.iter() {
        let path = path.as_str();
        if !eligible(path, prefix, &scan.cursor) {
            continue;
        }
        if scan.scan_file(repo, path, entry)? {
            scan.output.next_page_token = encode_cursor(&scan.cursor)?;
            break;
        }
    }
    Ok(scan.output)
}

impl App {
    /// Search a repository tree without invoking a process or following symbolic links.
    pub async fn search_tree(&self, call: &Call, req: v1::SearchTreeRequest) -> ApiResult<v1::SearchTreeResponse> {
        let name = require(&req.repo, "repo")?.to_owned();
        self.with_repo(call, &name, Cap::TreeRead, None, move |repo, _| search(repo, &req)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_lowercase_positions_keep_utf8_excerpts_valid() {
        let line = format!("{}İstanbul search target", "界".repeat(600));
        let at = find_match(&line, "i\u{307}stanbul", false).unwrap();
        assert!(excerpt(&line, at).contains("İstanbul"));
        assert!(excerpt(&line, at).chars().count() <= 514);
        assert!(find_match("TARGET", "target", true).is_none());
    }

    #[test]
    fn cursors_refuse_different_search_inputs_and_malformed_data() {
        let token = encode_cursor(&Cursor { binding: "one".into(), path: "a.txt".into(), line: 7 }).unwrap();
        assert_eq!(decode_cursor(&token, "one").unwrap().line, 7);
        assert!(decode_cursor(&token, "two").is_err());
        assert!(decode_cursor("not a cursor", "one").is_err());
        // Valid repository paths can contain quotes, which expand when JSON-encoded.
        let path = std::iter::repeat_n("\"".repeat(255), 16).collect::<Vec<_>>().join("/");
        let long = encode_cursor(&Cursor { binding: "one".into(), path: path.clone(), line: 1 }).unwrap();
        assert_eq!(decode_cursor(&long, "one").unwrap().path, path);
    }
}
