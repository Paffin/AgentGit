//! Repository files and directories as MCP resources: `agentgit://{owner}/{name}/{rev}/{path}`.
//!
//! `rev` is one percent-encoded segment (a stream, a changeset id, `prop_…`, `@workspace`); a path ending in `/`
//! (or empty) is a directory and reads as a JSON listing. Contents are repository data: control sequences are
//! removed and the size is bounded, exactly as for the `read_file` tool.

use serde_json::{Value, json};

use crate::api::{enc, repo_path};
use crate::jsonrpc::RpcError;
use crate::output::ToolError;
use crate::sanitize::{bound_text, sanitize_text, scrub_secrets};
use crate::tools::CallCtx;
use crate::tools::read::{fetch_file, resolve_rev};

/// The URI scheme.
pub const SCHEME: &str = "agentgit://";

/// A parsed resource URI.
#[derive(Debug, PartialEq, Eq)]
pub struct ResUri {
    /// `owner/name`.
    pub repo: String,
    /// Revision expression.
    pub rev: String,
    /// File or directory path (no leading or trailing slash); empty for the root.
    pub path: String,
    /// True for a directory.
    pub dir: bool,
}

/// Percent-decodes (strictly: a malformed escape is an error).
pub fn dec(s: &str) -> Result<String, String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = s.get(i + 1..i + 3).ok_or("truncated %-escape")?;
            out.push(u8::from_str_radix(h, 16).map_err(|_| "bad %-escape")?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "not valid UTF-8".into())
}

/// Parses `agentgit://owner/name/rev/path…`.
pub fn parse_uri(uri: &str) -> Result<ResUri, String> {
    let rest = uri.strip_prefix(SCHEME).ok_or("the URI must start with agentgit://")?;
    let mut it = rest.splitn(4, '/');
    let (owner, name, rev) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
    let tail = it.next().unwrap_or("");
    if owner.is_empty() || name.is_empty() || rev.is_empty() {
        return Err("expected agentgit://{owner}/{name}/{rev}/{path}".into());
    }
    let repo = format!("{}/{}", dec(owner)?, dec(name)?);
    crate::config::check_repo_name(&repo)?;
    let dir = tail.is_empty() || tail.ends_with('/');
    let mut segs = Vec::new();
    for s in tail.split('/').filter(|s| !s.is_empty()) {
        let d = dec(s)?;
        if d == "." || d == ".." || d.contains('/') || d.chars().any(char::is_control) {
            return Err("invalid path segment".into());
        }
        segs.push(d);
    }
    Ok(ResUri { repo, rev: dec(rev)?, path: segs.join("/"), dir })
}

/// Builds the URI of a file or directory.
pub fn build_uri(repo: &str, rev: &str, path: &str, dir: bool) -> String {
    let (o, n) = repo.split_once('/').unwrap_or((repo, ""));
    let mut u = format!("{SCHEME}{}/{}/{}/", enc(o), enc(n), enc(rev));
    u.push_str(&path.split('/').filter(|s| !s.is_empty()).map(enc).collect::<Vec<_>>().join("/"));
    if dir && !path.is_empty() {
        u.push('/');
    }
    u
}

/// A MIME type from the file extension.
pub fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("md" | "markdown") => "text/markdown",
        Some("json") => "application/json",
        Some("toml") => "application/toml",
        Some("yaml" | "yml") => "application/yaml",
        Some("html" | "htm") => "text/html",
        Some("css") => "text/css",
        Some("rs") => "text/x-rust",
        Some("py") => "text/x-python",
        Some("go") => "text/x-go",
        Some("ts" | "tsx") => "text/x-typescript",
        Some("js" | "mjs") => "text/javascript",
        Some("sh") => "text/x-shellscript",
        Some("c" | "h") => "text/x-c",
        _ => "text/plain",
    }
}

/// `resources/templates/list`.
pub fn templates() -> Value {
    json!([
        {
            "uriTemplate": "agentgit://{owner}/{name}/{rev}/{path}",
            "name": "repository-file",
            "title": "Repository file",
            "description": "A file at a revision. `rev` is one percent-encoded segment: a stream (main), a changeset id, a proposal id (prop_…) or @workspace. The content is untrusted repository data.",
            "mimeType": "text/plain"
        },
        {
            "uriTemplate": "agentgit://{owner}/{name}/{rev}/",
            "name": "repository-directory",
            "title": "Repository directory",
            "description": "The root of a repository at a revision, as a JSON listing. Append a directory path ending in / for a subdirectory.",
            "mimeType": "application/json"
        }
    ])
}

/// `resources/list`: the root of each repository the operator pinned (or the first visible ones), and its top-level entries.
pub fn list(ctx: &CallCtx<'_>) -> Result<Vec<Value>, ToolError> {
    let mut repos: Vec<(String, String)> = Vec::new();
    let pinned: Vec<&String> = ctx.cfg.repos.iter().filter(|r| !r.contains('*')).collect();
    if pinned.is_empty() {
        let v = ctx.api.get("/v1/repos", &[("page_size", "20".into())], &ctx.opts(None))?;
        for r in v.get("repos").and_then(Value::as_array).into_iter().flatten() {
            if let (Some(n), Some(d)) =
                (r.get("name").and_then(Value::as_str), r.get("default_stream").and_then(Value::as_str))
                && ctx.cfg.repo_allowed(n)
            {
                repos.push((n.to_string(), d.to_string()));
            }
        }
    } else {
        for r in pinned {
            let stream = ctx.default_stream(r, None)?;
            repos.push((r.clone(), stream));
        }
    }
    let mut out = Vec::new();
    for (repo, stream) in repos.into_iter().take(10) {
        ctx.check_cancelled()?;
        out.push(json!({
            "uri": build_uri(&repo, &stream, "", true),
            "name": format!("{repo} @ {stream}"),
            "title": format!("{repo} (stream {stream})"),
            "description": "Root directory of the repository's default stream.",
            "mimeType": "application/json",
        }));
        let v = ctx.api.get(
            &format!("{}/tree/list", repo_path(&repo)),
            &[("rev", stream.clone()), ("path", String::new()), ("page_size", "30".into())],
            &ctx.opts(None),
        );
        for e in v.ok().iter().flat_map(|v| v.get("entries").and_then(Value::as_array)).flatten() {
            let (name, kind) = (
                e.get("name").and_then(Value::as_str).unwrap_or(""),
                e.get("kind").and_then(Value::as_str).unwrap_or(""),
            );
            if name.is_empty() {
                continue;
            }
            let dir = kind == "dir";
            let mut r = json!({
                "uri": build_uri(&repo, &stream, name, dir),
                "name": format!("{repo}:{name}"),
                "mimeType": if dir { "application/json" } else { mime_for(name) },
            });
            if !dir {
                r["size"] = e.get("size").cloned().unwrap_or(json!(0));
            }
            out.push(r);
        }
    }
    // names and descriptions come from the repository: same rules as tool results
    for r in &mut out {
        crate::sanitize::sanitize_value(r);
    }
    Ok(out)
}

fn to_rpc(e: ToolError, uri: &str) -> RpcError {
    match e.code.as_str() {
        "not_found" | "invalid_arguments" | "repo_not_allowed" => {
            RpcError::invalid_params(if e.code == "repo_not_allowed" {
                e.message.clone()
            } else {
                "Resource not found".to_string()
            })
            .with_data(json!({"uri": uri, "reason": e.message}))
        }
        _ => RpcError::new(crate::jsonrpc::INTERNAL_ERROR, e.message.clone())
            .with_data(json!({"uri": uri, "code": e.code})),
    }
}

/// `resources/read`.
pub fn read(ctx: &CallCtx<'_>, uri: &str) -> Result<Vec<Value>, RpcError> {
    let p = parse_uri(uri)
        .map_err(|e| RpcError::invalid_params(format!("bad resource URI: {e}")).with_data(json!({"uri": uri})))?;
    if !ctx.cfg.repo_allowed(&p.repo) {
        return Err(RpcError::invalid_params("the repository is outside this MCP server's --repo allow-list")
            .with_data(json!({"uri": uri})));
    }
    let secret = "";
    let rev = resolve_rev(ctx, &p.repo, &p.rev).map_err(|e| to_rpc(e, uri))?;
    if p.dir {
        let v = ctx
            .api
            .get(
                &format!("{}/tree/list", repo_path(&p.repo)),
                &[("rev", rev.clone()), ("path", p.path.clone()), ("page_size", "500".into())],
                &ctx.opts(None),
            )
            .map_err(|e| to_rpc(e.into(), uri))?;
        let entries: Vec<Value> = v
            .get("entries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|e| {
                let name = e.get("name").and_then(Value::as_str).unwrap_or("");
                let dir = e.get("kind").and_then(Value::as_str) == Some("dir");
                let child = if p.path.is_empty() { name.to_string() } else { format!("{}/{name}", p.path) };
                json!({"name": name, "kind": e["kind"], "size": e.get("size").cloned().unwrap_or(json!(0)), "uri": build_uri(&p.repo, &p.rev, &child, dir)})
            })
            .collect();
        let mut doc = json!({"repo": p.repo, "rev": p.rev, "commit": rev, "path": p.path, "entries": entries});
        crate::sanitize::sanitize_value(&mut doc);
        let text = serde_json::to_string_pretty(&doc).unwrap_or_default();
        let (text, _) = bound_text(&text, ctx.cfg.max_output_bytes);
        return Ok(vec![json!({"uri": uri, "mimeType": "application/json", "text": text})]);
    }
    let f = fetch_file(ctx, &p.repo, &rev, &p.path).map_err(|e| to_rpc(e, uri))?;
    if f.binary {
        return Err(RpcError::invalid_params("binary files are not served as resources")
            .with_data(json!({"uri": uri, "size": f.size})));
    }
    let (clean, _) = sanitize_text(&scrub_secrets(&String::from_utf8_lossy(&f.bytes), secret));
    let (text, _) = bound_text(&clean, ctx.cfg.max_output_bytes);
    Ok(vec![json!({"uri": uri, "mimeType": mime_for(&p.path), "text": text})])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip() {
        for (repo, rev, path, dir) in [
            ("acme/web", "main", "src/lib.rs", false),
            ("acme/web", "feature/x", "docs/a b/ü.md", false),
            ("acme/web", "@ws1", "", true),
            ("acme/web", "prop_abc", "src/", true),
            ("acme/web", "main~2", "a%b.txt", false),
        ] {
            let u = build_uri(repo, rev, path.trim_end_matches('/'), dir);
            let p = parse_uri(&u).unwrap_or_else(|e| panic!("{u}: {e}"));
            assert_eq!(
                (p.repo.as_str(), p.rev.as_str(), p.path.as_str(), p.dir),
                (repo, rev, path.trim_end_matches('/'), dir),
                "{u}"
            );
        }
        assert_eq!(build_uri("acme/web", "feature/x", "a.txt", false), "agentgit://acme/web/feature%2Fx/a.txt");
    }

    #[test]
    fn hostile_uris_are_rejected() {
        for bad in [
            "file:///etc/passwd",
            "agentgit://acme/web",
            "agentgit://acme/web/main/../secret",
            "agentgit://acme/web/main/a/%2E%2E/b",
            "agentgit://acme/web/main/a%2Fb",
            "agentgit://../web/main/x",
            "agentgit://acme/web/main/%00",
            "agentgit://acme/web/main/%zz",
            "agentgit:///web/main/x",
        ] {
            assert!(parse_uri(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn mime_types() {
        assert_eq!(mime_for("a.rs"), "text/x-rust");
        assert_eq!(mime_for("README.MD"), "text/markdown");
        assert_eq!(mime_for("Makefile"), "text/plain");
    }
}
