//! The `workspace` group: a virtual, server-side working copy. Each `workspace_write_files` call is one atomic
//! snapshot (one new changeset) on top of the workspace head.

use base64::Engine as _;
use serde_json::{Value, json};

use super::*;
use crate::api::{enc, repo_path};
use crate::config::{Group, glob};
use crate::output::{ToolError, ToolOutput};

/// The most content one `workspace_write_files` call may carry.
pub const MAX_WRITE_BYTES: usize = 4 * 1024 * 1024;
const MAX_CHANGES: usize = 200;

/// A repository path as a model might write it: checked before it is sent anywhere.
pub fn validate_repo_path(p: &str) -> Result<(), String> {
    if p.is_empty() || p.len() > 1024 {
        return Err("a path must be 1 to 1024 bytes".into());
    }
    if p.starts_with('/') || p.ends_with('/') {
        return Err("a path is relative and names a file (no leading or trailing `/`)".into());
    }
    if p.chars().any(|c| c.is_control() || c == '\\') {
        return Err("a path may not contain control characters or backslashes".into());
    }
    if p.split('/').any(|s| s.is_empty() || s == "." || s == "..") {
        return Err("a path may not contain empty, `.` or `..` segments".into());
    }
    Ok(())
}

fn workspace_name(args: &Value, key: &str) -> Result<String, ToolError> {
    let n = str_arg(args, key).unwrap_or_default();
    if n.is_empty() || n.len() > 64 || !n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(ToolError::invalid(format!("`{key}` must be 1-64 letters, digits, `_` or `-`")));
    }
    Ok(n.to_string())
}

struct Change {
    path: String,
    put: bool,
    bytes: Vec<u8>,
    executable: bool,
}

fn parse_changes(items: &[Value]) -> Result<Vec<Change>, ToolError> {
    let mut out = Vec::with_capacity(items.len());
    let mut seen = std::collections::BTreeSet::new();
    let mut total = 0usize;
    for (i, c) in items.iter().enumerate() {
        let path = str_arg(c, "path").unwrap_or_default().to_string();
        validate_repo_path(&path).map_err(|e| ToolError::invalid(format!("changes[{i}].path: {e}")))?;
        if !seen.insert(path.clone()) {
            return Err(ToolError::invalid(format!(
                "changes[{i}].path: `{}` appears twice in one call",
                crate::sanitize::attr_value(&path)
            )));
        }
        let op = str_arg(c, "op").unwrap_or_default();
        let (text, b64) = (str_arg(c, "content"), str_arg(c, "content_base64"));
        let bytes = match (op, text, b64) {
            ("put", Some(t), None) => t.as_bytes().to_vec(),
            ("put", None, Some(b)) => base64::engine::general_purpose::STANDARD
                .decode(b.trim())
                .map_err(|e| ToolError::invalid(format!("changes[{i}].content_base64 is not valid base64: {e}")))?,
            ("put", Some(_), Some(_)) => {
                return Err(ToolError::invalid(format!("changes[{i}]: give `content` or `content_base64`, not both")));
            }
            ("put", None, None) => {
                return Err(ToolError::invalid(format!(
                    "changes[{i}]: `put` needs `content` (text) or `content_base64`"
                )));
            }
            ("delete", None, None) => Vec::new(),
            ("delete", _, _) => return Err(ToolError::invalid(format!("changes[{i}]: `delete` takes no content"))),
            _ => return Err(ToolError::invalid(format!("changes[{i}].op must be `put` or `delete`"))),
        };
        total += bytes.len();
        if total > MAX_WRITE_BYTES {
            return Err(ToolError::local(
                "too_large",
                format!("One call may carry at most {MAX_WRITE_BYTES} bytes of content."),
            )
            .hint("Split the change over several workspace_write_files calls (each becomes its own changeset)."));
        }
        out.push(Change {
            path,
            put: op == "put",
            bytes,
            executable: c.get("executable").and_then(Value::as_bool).unwrap_or(false),
        });
    }
    Ok(out)
}

/// Enforces the operator's second line of defence on a set of paths about to be written.
fn check_write_scope(ctx: &CallCtx<'_>, args: &Value, repo: &str, paths: &[&str]) -> Result<(), ToolError> {
    if let Some(set) = ctx.cfg.write_path_set().map_err(|e| ToolError::local("internal", e))? {
        let outside: Vec<&&str> = paths.iter().filter(|p| !set.is_match(**p)).collect();
        if !outside.is_empty() {
            let shown: Vec<String> =
                outside.iter().take(10).map(|p| format!("`{}`", crate::sanitize::attr_value(p))).collect();
            return Err(ToolError::refused(
                "path_not_allowed",
                format!(
                    "Refused before anything was written: {} {} outside the paths the operator allows this MCP server to write ({}).",
                    shown.join(", "),
                    if outside.len() == 1 { "is" } else { "are" },
                    ctx.cfg.write_paths.join(", ")
                ),
            )
            .hint("Write only inside those paths. This limit is set by the operator with --write-path and cannot be changed from here."));
        }
    }
    let wanted = str_arg(args, "lease_id");
    if !ctx.cfg.require_lease && wanted.is_none() {
        return Ok(());
    }
    let who = ctx.api.get("/v1/whoami", &[], &ctx.opts(None))?;
    let me = who.get("identity").and_then(Value::as_str).unwrap_or("").split('#').next().unwrap_or("").to_string();
    let v = ctx.api.get(&format!("{}/leases", repo_path(repo)), &[("active_only", "true".into())], &ctx.opts(None))?;
    let now = now_ms();
    let mine: Vec<&Value> = v
        .get("leases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|l| {
            l.get("holder").and_then(Value::as_str).is_some_and(|h| h.split('#').next() == Some(me.as_str()))
                && l.get("released_at_ms").and_then(Value::as_i64).unwrap_or(0) == 0
                && l.get("expires_at_ms").and_then(Value::as_i64).unwrap_or(0) > now
        })
        .collect();
    let considered: Vec<&Value> = match wanted {
        Some(id) => {
            let found: Vec<&Value> =
                mine.iter().copied().filter(|l| l.get("id").and_then(Value::as_str) == Some(id)).collect();
            if found.is_empty() {
                return Err(ToolError::refused(
                    "lease_not_held",
                    format!("Lease `{}` is not an active lease held by {me}.", crate::sanitize::attr_value(id)),
                )
                .hint(
                    "Acquire one with lease_acquire (or renew an expired one with lease_acquire again), then retry.",
                ));
            }
            found
        }
        None => mine,
    };
    let scopes: Vec<String> =
        considered.iter().filter_map(|l| l.get("scope").and_then(Value::as_str).map(str::to_string)).collect();
    let mut matchers = Vec::new();
    for s in &scopes {
        if let Ok(g) = glob(s) {
            matchers.push(g.compile_matcher());
        }
    }
    let outside: Vec<&&str> = paths.iter().filter(|p| !matchers.iter().any(|m| m.is_match(**p))).collect();
    if !outside.is_empty() {
        let shown: Vec<String> =
            outside.iter().take(10).map(|p| format!("`{}`", crate::sanitize::attr_value(p))).collect();
        return Err(ToolError::refused(
            "path_outside_lease",
            format!(
                "Refused before anything was written: {} {} outside {}. {}",
                shown.join(", "),
                if outside.len() == 1 { "is" } else { "are" },
                if scopes.is_empty() {
                    "any lease you hold (you hold none that is active)".to_string()
                } else {
                    format!("the lease scope(s) you hold: {}", scopes.join(", "))
                },
                if ctx.cfg.require_lease {
                    "The operator requires writes to be covered by a lease (--require-lease)."
                } else {
                    "You asked for the write to be checked against a lease."
                }
            ),
        )
        .hint("Acquire a lease that covers these paths with lease_acquire, or keep the change inside your lease."));
    }
    Ok(())
}

fn workspace_create(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let run = ctx.run_id(args)?;
    let name = workspace_name(args, "name")?;
    let stream = match str_arg(args, "stream") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => ctx.default_stream(&repo, run)?,
    };
    let mut body = json!({"name": name, "stream": stream});
    if let Some(r) = str_arg(args, "from_rev").filter(|r| !r.is_empty()) {
        body["from_rev"] = json!(r);
    }
    let w = ctx.api.send("POST", &format!("{}/workspaces", repo_path(&repo)), Some(&body), &ctx.opts(run))?;
    Ok(ToolOutput::new(
        format!(
            "Workspace `{name}` created on `{}`. Write files with workspace_write_files.",
            crate::sanitize::attr_value(&stream)
        ),
        json!({"workspace": workspace_brief(&w)}),
    ))
}

fn workspace_brief(w: &Value) -> Value {
    json!({"name": w["name"], "stream": w["stream"], "base": w["base"], "head": w["head"], "owner": w["owner"]})
}

fn write_files(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let run = ctx.run_id(args)?;
    let name = workspace_name(args, "workspace")?;
    let message = str_arg(args, "message").unwrap_or_default().trim();
    if message.is_empty() {
        return Err(ToolError::invalid("`message` (the changeset message) may not be empty"));
    }
    let items = args.get("changes").and_then(Value::as_array).cloned().unwrap_or_default();
    if items.is_empty() || items.len() > MAX_CHANGES {
        return Err(ToolError::invalid(format!("`changes` needs 1 to {MAX_CHANGES} entries")));
    }
    let changes = parse_changes(&items)?;
    ctx.step(0, 2, "checking the paths against the allow-list");
    let paths: Vec<&str> = changes.iter().map(|c| c.path.as_str()).collect();
    check_write_scope(ctx, args, &repo, &paths)?;
    ctx.check_cancelled()?;
    ctx.step(1, 2, "writing the snapshot");
    let wire: Vec<Value> = changes
        .iter()
        .map(|c| {
            if c.put {
                json!({"path": c.path, "op": "put", "content": base64::engine::general_purpose::STANDARD.encode(&c.bytes), "executable": c.executable})
            } else {
                json!({"path": c.path, "op": "delete"})
            }
        })
        .collect();
    let mut body = json!({"message": message, "changes": wire});
    if let Some(h) = str_arg(args, "expected_head").filter(|h| !h.is_empty()) {
        body["expected_head"] = json!(h);
    }
    let w = ctx.api.send(
        "POST",
        &format!("{}/workspaces/{}/changes", repo_path(&repo), enc(&name)),
        Some(&body),
        &ctx.opts(run),
    )?;
    let head = w.get("head").and_then(Value::as_str).unwrap_or("").to_string();
    let files: Vec<Value> = changes
        .iter()
        .map(|c| json!({"path": c.path, "op": if c.put { "put" } else { "delete" }, "bytes": c.bytes.len()}))
        .collect();
    Ok(ToolOutput::new(
        format!(
            "Snapshot {} written to workspace `{name}`: {} files. Propose it with propose when it is complete.",
            short(&head),
            files.len()
        ),
        json!({"workspace": workspace_brief(&w), "head": head, "files": files, "message": message}),
    ))
}

fn resolution_source(value: &Value, index: usize) -> Result<(Value, usize), ToolError> {
    let text = str_arg(value, "content");
    let base64 = str_arg(value, "content_base64");
    let blob = str_arg(value, "blob_id");
    if [text, base64, blob].into_iter().flatten().count() != 1 {
        return Err(ToolError::invalid(format!(
            "resolutions[{index}]: give exactly one of `content`, `content_base64` or `blob_id`"
        )));
    }
    if let Some(blob) = blob {
        if blob.len() != 64 || !blob.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(ToolError::invalid(format!("resolutions[{index}].blob_id must be a 64-character object id")));
        }
        return Ok((json!({"blob_id": blob}), 0));
    }
    let bytes = match (text, base64) {
        (Some(text), _) => text.as_bytes().to_vec(),
        (_, Some(encoded)) => base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .map_err(|e| ToolError::invalid(format!("resolutions[{index}].content_base64: {e}")))?,
        _ => return Err(ToolError::invalid("a resolution needs one content source")),
    };
    Ok((json!({"content": base64::engine::general_purpose::STANDARD.encode(&bytes)}), bytes.len()))
}

fn resolution(value: &Value, index: usize) -> Result<(Value, usize), ToolError> {
    let path = str_arg(value, "path").unwrap_or_default();
    validate_repo_path(path).map_err(|e| ToolError::invalid(format!("resolutions[{index}].path: {e}")))?;
    let op = str_arg(value, "op").unwrap_or("put");
    if op == "delete" {
        if ["content", "content_base64", "blob_id"].iter().any(|field| value.get(field).is_some()) {
            return Err(ToolError::invalid(format!("resolutions[{index}]: delete takes no content")));
        }
        return Ok((json!({"path": path, "op": "delete"}), 0));
    }
    if op != "put" {
        return Err(ToolError::invalid(format!("resolutions[{index}].op must be put or delete")));
    }
    let file_type = str_arg(value, "file_type").unwrap_or("regular");
    if !["regular", "symlink"].contains(&file_type) {
        return Err(ToolError::invalid(format!("resolutions[{index}].file_type must be regular or symlink")));
    }
    let (mut wire, bytes) = resolution_source(value, index)?;
    wire["path"] = json!(path);
    wire["op"] = json!("put");
    wire["file_type"] = json!(file_type);
    wire["executable"] = json!(value.get("executable").and_then(Value::as_bool).unwrap_or(false));
    Ok((wire, bytes))
}

fn parse_resolutions(items: &[Value]) -> Result<Vec<Value>, ToolError> {
    if items.len() > MAX_CHANGES {
        return Err(ToolError::invalid("too many resolutions"));
    }
    let mut result = Vec::with_capacity(items.len());
    let mut seen = std::collections::BTreeSet::new();
    let mut total = 0usize;
    for (index, item) in items.iter().enumerate() {
        let (wire, bytes) = resolution(item, index)?;
        if !seen.insert(wire["path"].to_string()) {
            return Err(ToolError::invalid("a resolved path appears twice"));
        }
        total += bytes;
        if total > MAX_WRITE_BYTES {
            return Err(ToolError::local(
                "too_large",
                "Resolved content exceeds the 4 MiB limit; use existing blob ids for large files.",
            ));
        }
        result.push(wire);
    }
    Ok(result)
}

fn sync_body(ctx: &CallCtx<'_>, args: &Value, repo: &str, run: Option<&str>) -> Result<Value, ToolError> {
    let expected = str_arg(args, "expected_stream_head").filter(|head| !head.is_empty());
    if expected.is_some() && !ctx.api.supports("workspaces.sync_precondition", &ctx.opts(run))? {
        return Err(ToolError::local(
            "server_update_required",
            "This server cannot pin the branch revision for workspace sync.",
        )
        .hint("Update the server before applying an inspected conflict resolution."));
    }
    let items = args.get("resolutions").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    let wire = parse_resolutions(items)?;
    let paths: Vec<&str> = wire.iter().filter_map(|value| value["path"].as_str()).collect();
    if !paths.is_empty() {
        check_write_scope(ctx, args, repo, &paths)?;
    }
    let mut body = json!({"resolutions": wire});
    if let Some(expected) = expected {
        body["expected_stream_head"] = json!(expected);
    }
    Ok(body)
}

fn workspace_sync(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let run = ctx.run_id(args)?;
    let name = workspace_name(args, "workspace")?;
    let body = sync_body(ctx, args, &repo, run)?;
    let value = ctx.api.send(
        "POST",
        &format!("{}/workspaces/{}/sync", repo_path(&repo), enc(&name)),
        Some(&body),
        &ctx.opts(run),
    )?;
    Ok(sync_output(&name, &value))
}

fn sync_output(name: &str, value: &Value) -> ToolOutput {
    let status = value.get("status").and_then(Value::as_str).unwrap_or("");
    let conflicts: Vec<Value> =
        value.get("conflicts").and_then(Value::as_array).into_iter().flatten().take(200).cloned().collect();
    let summary = match status {
        "up_to_date" => format!("Workspace `{name}` already contains the stream head."),
        "fast_forwarded" => format!("Workspace `{name}` was fast-forwarded to the stream head."),
        "merged" => format!("Workspace `{name}` merged the stream head cleanly."),
        "conflicted" => {
            format!("Workspace `{name}` was NOT changed: {} conflicting paths need your resolution.", conflicts.len())
        }
        other => format!("Sync finished with status `{}`.", crate::sanitize::attr_value(other)),
    };
    let mut out = ToolOutput::new(
        summary,
        json!({"status": status, "head": value.get("head").cloned().unwrap_or(json!("")), "stream_head": value.get("stream_head").cloned().unwrap_or(json!("")), "conflicts": conflicts}),
    );
    if status == "conflicted" {
        let pinned = value.get("stream_head").and_then(Value::as_str).is_some_and(|head| !head.is_empty());
        let advice = if pinned {
            "Read both versions at the returned stream_head and your workspace revision. When a conflict has origin paths, read origin.ours at your workspace revision and origin.theirs at stream_head; otherwise use the conflict path. Decide every conflicting path, then repeat workspace_sync with expected_stream_head set to that inspected stream_head and resolutions for every conflict. Existing blob ids preserve binary content, file type and executable mode."
        } else {
            "This server did not identify a pinned branch revision. Upgrade it before applying an inspected resolution through MCP, or resolve through the local CLI. Do not treat the moving branch name as the reviewed revision."
        };
        out = out.note(advice);
    }
    out
}

fn workspace_delete(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let run = ctx.run_id(args)?;
    let name = workspace_name(args, "workspace")?;
    ctx.api.send("DELETE", &format!("{}/workspaces/{}", repo_path(&repo), enc(&name)), None, &ctx.opts(run))?;
    Ok(ToolOutput::new(
        format!("Workspace `{name}` removed. Changesets already proposed stay in history."),
        json!({"workspace": name, "deleted": true}),
    ))
}

/// The tools of the workspace group.
pub fn tools() -> Vec<ToolDef> {
    let change = json!({
        "type": "object",
        "required": ["path", "op"],
        "additionalProperties": false,
        "properties": {
            "path": {"type": "string", "description": "Relative file path, e.g. src/lib.rs.", "maxLength": 1024},
            "op": {"type": "string", "enum": ["put", "delete"], "description": "`put` creates or replaces the file, `delete` removes it."},
            "content": {"type": "string", "description": "The new file content as text (for `put`). Use this for source code."},
            "content_base64": {"type": "string", "description": "The new file content, base64 (for `put`), for binary files."},
            "executable": {"type": "boolean", "description": "Mark the file executable (for `put`)."}
        }
    });
    let resolution = json!({
        "type": "object",
        "required": ["path"],
        "additionalProperties": false,
        "properties": {
            "path": {"type": "string", "maxLength": 1024, "description": "A conflicting path."},
            "content": {"type": "string", "description": "The resolved content as text."},
            "content_base64": {"type": "string", "description": "The resolved content, base64."},
            "blob_id": {"type":"string","description":"Existing content blob to retain without downloading or re-encoding it.","minLength":64,"maxLength":64},
            "op": {"type":"string","enum":["put","delete"],"description":"Defaults to put. Delete takes no content source."},
            "file_type": {"type":"string","enum":["regular","symlink"],"description":"Type to preserve for put; defaults to regular."},
            "executable": {"type":"boolean","description":"Executable mode to preserve for put."}
        }
    });
    vec![
        def(
            "workspace_create",
            Group::Workspace,
            "Create a workspace",
            "Creates a virtual workspace: a private line of work on the server that starts at a stream's head. No checkout is needed. Edit it with workspace_write_files.",
            Ann::ADD,
            input(
                vec![
                    p_repo(),
                    ("name", t_str_max("Workspace name: 1-64 letters, digits, `_` or `-`.", 64)),
                    ("stream", t_str("Stream to start from (default: the default stream).")),
                    ("from_rev", t_str("Start from this revision instead of the stream head.")),
                    p_run(),
                ],
                &["name"],
            ),
            output(vec![("workspace", o_obj())], &["workspace"]),
            workspace_create,
        ),
        def(
            "workspace_write_files",
            Group::Workspace,
            "Write files (snapshot)",
            "Applies a batch of file changes (put or delete) to a workspace as ONE atomic snapshot: a new changeset on top of the workspace head. Paths must be relative. The operator may restrict writable paths and require a covering lease; a refused write changes nothing and says why.",
            Ann::ADD,
            input(
                vec![
                    p_repo(),
                    ("workspace", t_str_max("Workspace name.", 64)),
                    ("message", t_str_max("Changeset message: say what and why in one or two sentences.", 1000)),
                    (
                        "changes",
                        t_arr("The file changes (1-200, at most 4 MiB of content in total).", change, MAX_CHANGES),
                    ),
                    (
                        "expected_head",
                        t_str("Refuse if the workspace head is not this changeset (guards against concurrent edits)."),
                    ),
                    (
                        "lease_id",
                        t_str("Check that every path is covered by this lease (which you must hold) before writing."),
                    ),
                    p_run(),
                ],
                &["workspace", "message", "changes"],
            ),
            output(vec![("workspace", o_obj()), ("head", o_str()), ("files", o_arr())], &["head", "files"]),
            write_files,
        ),
        def(
            "workspace_sync",
            Group::Workspace,
            "Sync with the stream",
            "Brings the stream head into the workspace: up to date, fast-forward, or a clean three-way merge. On conflicts it changes nothing and lists the conflicting paths; resolve them by calling again with `resolutions` (the final content of every conflicting path).",
            Ann::ADD,
            input(
                vec![
                    p_repo(),
                    ("workspace", t_str_max("Workspace name.", 64)),
                    (
                        "resolutions",
                        t_arr("Final content or existing blobs for every conflicting path.", resolution, MAX_CHANGES),
                    ),
                    (
                        "expected_stream_head",
                        t_str("Exact branch head inspected when preparing these resolutions; reject if it moved."),
                    ),
                    ("lease_id", t_str("Check the resolved paths against this lease before writing.")),
                    p_run(),
                ],
                &["workspace"],
            ),
            output(
                vec![("status", o_str()), ("head", o_str()), ("stream_head", o_str()), ("conflicts", o_arr())],
                &["status", "conflicts"],
            ),
            workspace_sync,
        ),
        def(
            "workspace_delete",
            Group::Workspace,
            "Delete a workspace",
            "Removes a workspace. Changesets that were already proposed stay in history; unproposed snapshots become unreachable.",
            Ann::DESTROY,
            input(vec![p_repo(), ("workspace", t_str_max("Workspace name.", 64)), p_run()], &["workspace"]),
            output(vec![("workspace", o_str()), ("deleted", o_bool())], &["deleted"]),
            workspace_delete,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_validation_blocks_traversal_and_oddities() {
        for ok in ["a", "src/lib.rs", "docs/a b.md", "ü/x"] {
            assert!(validate_repo_path(ok).is_ok(), "{ok}");
        }
        for bad in ["", "/etc/passwd", "a/", "../x", "a/../b", "a//b", "./a", "a\\b", "a\nb", "a\u{0}b"] {
            assert!(validate_repo_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn change_parsing() {
        let ok = parse_changes(&[
            json!({"path": "a.txt", "op": "put", "content": "hi"}),
            json!({"path": "b", "op": "delete"}),
        ])
        .unwrap();
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[0].bytes, b"hi");
        let e = |v: Value| parse_changes(&[v]).err().unwrap().message;
        assert!(e(json!({"path": "a", "op": "put"})).contains("needs `content`"));
        assert!(e(json!({"path": "a", "op": "put", "content": "x", "content_base64": "eA=="})).contains("not both"));
        assert!(e(json!({"path": "a", "op": "delete", "content": "x"})).contains("takes no content"));
        assert!(e(json!({"path": "a", "op": "rename"})).contains("put` or `delete"));
        assert!(e(json!({"path": "a", "op": "put", "content_base64": "!!"})).contains("base64"));
        assert!(parse_changes(&[json!({"path": "a", "op": "delete"}), json!({"path": "a", "op": "delete"})]).is_err());
        let big = "x".repeat(MAX_WRITE_BYTES + 1);
        assert_eq!(
            parse_changes(&[json!({"path": "a", "op": "put", "content": big})]).err().unwrap().code,
            "too_large"
        );
    }
    #[test]
    fn resolutions_keep_blob_identity_type_mode_and_deletion() {
        let blob = "a".repeat(64);
        let values = parse_resolutions(&[
            json!({"path":"image.bin","blob_id":blob,"executable":true,"file_type":"regular"}),
            json!({"path":"link","blob_id":"b".repeat(64),"file_type":"symlink"}),
            json!({"path":"gone","op":"delete"}),
        ])
        .unwrap();
        assert_eq!(
            values[0],
            json!({"path":"image.bin","op":"put","blob_id":blob,"executable":true,"file_type":"regular"})
        );
        assert_eq!(values[1]["file_type"], "symlink");
        assert_eq!(values[2], json!({"path":"gone","op":"delete"}));
    }

    #[test]
    fn resolutions_reject_ambiguous_sources_and_duplicate_paths() {
        for value in [
            json!({"path":"a","content":"text","blob_id":"a".repeat(64)}),
            json!({"path":"a","content_base64":"!!"}),
            json!({"path":"a","blob_id":"short"}),
            json!({"path":"a","op":"delete","content":"text"}),
            json!({"path":"a","content":"text","file_type":"directory"}),
        ] {
            assert!(parse_resolutions(&[value]).is_err());
        }
        assert!(parse_resolutions(&[json!({"path":"a","content":""}), json!({"path":"a","op":"delete"})]).is_err());
    }
    #[test]
    fn sync_preserves_renamed_conflict_source_paths() {
        let conflict = json!({"path":"renamed.txt","kind":"content","message":"both edited",
            "origin":{"base":"original.txt","ours":"renamed.txt","theirs":"original.txt"}});
        let legacy = json!({"path":"same.txt","kind":"content","message":""});
        let out = sync_output(
            "work",
            &json!({"status":"conflicted","stream_head":"known",
            "conflicts":[conflict.clone(),legacy.clone()]}),
        );
        assert_eq!(out.data["conflicts"], json!([conflict, legacy]));
        assert!(out.notes.join(" ").contains("origin.ours"));
        assert!(out.notes.join(" ").contains("origin.theirs"));
    }

    #[test]
    fn sync_guidance_never_treats_an_absent_target_as_a_safe_pin() {
        let legacy = sync_output("work", &json!({"status":"conflicted","conflicts":[]}));
        let advice = legacy.notes.join(" ");
        assert!(advice.contains("upgrade") || advice.contains("Upgrade"));
        assert!(!advice.contains("expected_stream_head"));
        let pinned = sync_output("work", &json!({"status":"conflicted","stream_head":"known","conflicts":[]}));
        assert!(pinned.notes.join(" ").contains("expected_stream_head"));
    }
}
