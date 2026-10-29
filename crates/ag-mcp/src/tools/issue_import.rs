//! Explicit preview and apply of normalized issue-import manifests.

use serde_json::{Value, json};

use super::*;
use crate::api::repo_path;

const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;

fn import(ctx: &CallCtx<'_>, args: &Value, apply: bool) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let opts = ctx.opts(ctx.run_id(args)?);
    let manifest = str_arg(args, "manifest_json").unwrap_or_default();
    if manifest.len() > MAX_MANIFEST_BYTES {
        return Err(ToolError::invalid("The import manifest exceeds 4 MiB of UTF-8"));
    }
    if !ctx.api.supports("issues.import", &opts)? {
        return Err(ToolError::local("server_update_required", "This server does not support issue imports."));
    }
    let mut body = json!({"manifest_json":manifest});
    let action = if apply { "apply" } else { "preview" };
    if apply {
        body["expected_digest"] = json!(str_arg(args, "expected_digest").unwrap_or_default());
        body["allow_unsupported"] = json!(args.get("allow_unsupported").and_then(Value::as_bool).unwrap_or(false));
    }
    let value = ctx.api.send("POST", &format!("{}/issues/import/{action}", repo_path(&repo)), Some(&body), &opts)?;
    let summary = if apply {
        "Issue import applied. Existing source mappings prevent duplicate imports."
    } else {
        "Import preview only. Inspect conflicts and warnings before explicitly applying this digest."
    };
    Ok(ToolOutput::new(summary, value))
}

fn preview(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    import(ctx, args, false)
}
fn apply(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    import(ctx, args, true)
}

/// Import tools require the explicit issue group; neither runs by default.
pub fn tools() -> Vec<ToolDef> {
    let fields = || {
        vec![
            p_repo(),
            (
                "manifest_json",
                t_str_max(
                    "Serialized normalized import manifest, at most 4 MiB UTF-8. No provider credentials.",
                    MAX_MANIFEST_BYTES,
                ),
            ),
            p_run(),
        ]
    };
    let mut apply_fields = fields();
    apply_fields.extend([
        ("expected_digest", t_str_max("Exact digest returned by the inspected preview.", 64)),
        ("allow_unsupported", t_bool("Explicitly accept unsupported items reported by preview; false refuses them.")),
    ]);
    vec![
        def(
            "issue_import_preview",
            Group::Issue,
            "Preview an issue import",
            "Validates a normalized import manifest without writing. Returns its digest, counts, mappings conflicts and unsupported-item warnings. Inspect this result before calling issue_import_apply.",
            Ann::READ,
            input(fields(), &["manifest_json"]),
            output(
                vec![("digest", o_str()), ("warnings", o_arr()), ("conflicts", o_arr())],
                &["digest", "warnings", "conflicts"],
            ),
            preview,
        ),
        def(
            "issue_import_apply",
            Group::Issue,
            "Apply an inspected issue import",
            "Imports issues and comments from the exact previewed manifest. Requires the preview digest and an explicit unsupported-item decision. Durable source mappings make retries safe; no provider credentials are accepted.",
            Ann::SET,
            input(apply_fields, &["manifest_json", "expected_digest", "allow_unsupported"]),
            output(
                vec![("created", o_int()), ("skipped", o_int()), ("mappings", o_arr())],
                &["created", "skipped", "mappings"],
            ),
            apply,
        ),
    ]
}
