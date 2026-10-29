//! Private, account-backed file edits; saving never changes a workspace or stream.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

use super::*;
use crate::api::{enc, repo_path};

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_BASE64: usize = MAX_BYTES.div_ceil(3) * 4;

fn request(ctx: &CallCtx<'_>, args: &Value, method: &str, list: bool) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let opts = ctx.opts(ctx.run_id(args)?);
    if !ctx.api.supports("files.drafts", &opts)? {
        return Err(ToolError::local("server_update_required", "This server does not support private file drafts."));
    }
    let stream = str_arg(args, "stream").unwrap_or_default();
    let endpoint = if list { "file-drafts" } else { "file-draft" };
    let path = format!("{}/{endpoint}", repo_path(&repo));
    let mut query = vec![("stream", stream.to_string())];
    if !list {
        let file = str_arg(args, "path").unwrap_or_default();
        super::workspace::validate_repo_path(file).map_err(ToolError::invalid)?;
        query.push(("path", file.to_string()));
    }
    let value = match method {
        "PUT" => ctx.api.send("PUT", &path, Some(&save_body(args)?), &opts)?,
        "DELETE" => {
            let version =
                u64_arg(args, "expected_version").ok_or_else(|| ToolError::invalid("expected_version is required"))?;
            let url = format!("{path}?stream={}&path={}&expected_version={version}", enc(stream), enc(&query[1].1));
            ctx.api.send("DELETE", &url, None, &opts)?
        }
        _ => ctx.api.get(&path, &query, &opts)?,
    };
    let data = if list { value } else { json!({"draft":value}) };
    Ok(ToolOutput::new("Private file drafts only. No workspace, stream or proposal was changed.", data)
        .note("Draft content is base64. Never save truncated output as a complete file."))
}

fn save_body(args: &Value) -> Result<Value, ToolError> {
    let bytes = match (str_arg(args, "content"), str_arg(args, "content_base64")) {
        (Some(text), None) => text.as_bytes().to_vec(),
        (None, Some(encoded)) if encoded.len() <= MAX_BASE64 => {
            STANDARD.decode(encoded).map_err(|_| ToolError::invalid("content_base64 must be valid base64"))?
        }
        (None, Some(_)) => return Err(ToolError::invalid("File draft content exceeds 4 MiB")),
        _ => return Err(ToolError::invalid("Give exactly one of content or content_base64")),
    };
    if bytes.len() > MAX_BYTES {
        return Err(ToolError::invalid("File draft content exceeds 4 MiB"));
    }
    Ok(json!({
        "stream":str_arg(args,"stream").unwrap_or_default(),
        "path":str_arg(args,"path").unwrap_or_default(),
        "base":str_arg(args,"base").unwrap_or_default(),
        "content":STANDARD.encode(bytes),
        "expected_version":u64_arg(args,"expected_version")
            .ok_or_else(|| ToolError::invalid("expected_version is required"))?,
    }))
}

fn get(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    request(ctx, args, "GET", false)
}
fn list(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    request(ctx, args, "GET", true)
}
fn save(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    request(ctx, args, "PUT", false)
}
fn clear(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    request(ctx, args, "DELETE", false)
}

/// Registers explicit private draft actions under the workspace permission group.
pub fn tools() -> Vec<ToolDef> {
    let common = || vec![p_repo(), ("stream", t_str("Exact stream name.")), p_run()];
    let keyed = || {
        let mut fields = common();
        fields.push(("path", t_str_max("Existing repository-relative file path.", 1024)));
        fields
    };
    let guarded = || {
        let mut fields = keyed();
        fields.push((
            "expected_version",
            t_int("Version returned by file_draft_get; zero only before the first save.", 0, i64::MAX),
        ));
        fields
    };
    let mut save_fields = guarded();
    save_fields.extend([
        ("base", t_str("Exact changeset containing the original file. Saving never rebases the draft.")),
        (
            "content",
            t_str_max("Complete edited file as UTF-8 text, up to 4 MiB. Use this or content_base64.", MAX_BYTES),
        ),
        ("content_base64", t_str_max("Complete edited binary file as base64. Use this or content.", MAX_BASE64)),
    ]);
    vec![
        def(
            "file_draft_get",
            Group::Workspace,
            "Read your private file draft",
            "Reads your account's private draft for an existing file. Content is base64 and may be truncated by the configured output limit; it is never a published file.",
            Ann::READ,
            input(keyed(), &["stream", "path"]),
            output(vec![("draft", o_obj())], &["draft"]),
            get,
        ),
        def(
            "file_draft_list",
            Group::Workspace,
            "List your private file drafts",
            "Lists only your account's live draft metadata in this stream, without file contents. At most 20 live drafts exist per account and repository.",
            Ann::READ,
            input(common(), &["stream"]),
            output(vec![("drafts", o_arr())], &["drafts"]),
            list,
        ),
        def(
            "file_draft_save",
            Group::Workspace,
            "Save a private file draft",
            "Saves complete edited content against an inspected draft version and exact base changeset. Existing files only, up to 4 MiB. Does not publish, snapshot, rebase or change a workspace; stale versions preserve the other edit.",
            Ann::SET,
            input(save_fields, &["stream", "path", "base", "expected_version"]),
            output(vec![("draft", o_obj())], &["draft"]),
            save,
        ),
        def(
            "file_draft_clear",
            Group::Workspace,
            "Clear your private file draft",
            "Clears only your account's draft at the inspected version. A newer draft is retained on conflict. Clearing does not remove the repository file or change a workspace.",
            Ann::DESTROY,
            input(guarded(), &["stream", "path", "expected_version"]),
            output(vec![("draft", o_obj())], &["draft"]),
            clear,
        ),
    ]
}
