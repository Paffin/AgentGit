//! The `propose`, `review` and `integrate` groups: the life of a proposal.

use serde_json::{Value, json};

use super::read::{proposal_brief, status_of};
use super::*;
use crate::api::{enc, repo_path};
use crate::config::Group;
use crate::output::{ToolError, ToolOutput};
use crate::verdicts::explain_blocker;

/// Resolves the head to propose from `workspace` (its current head) or an explicit `head`.
fn head_and_stream(
    ctx: &CallCtx<'_>,
    repo: &str,
    args: &Value,
    run: Option<&str>,
) -> Result<(String, Option<String>), ToolError> {
    match (str_arg(args, "workspace").filter(|w| !w.is_empty()), str_arg(args, "head").filter(|h| !h.is_empty())) {
        (Some(w), None) => {
            if w.len() > 64 || !w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                return Err(ToolError::invalid("`workspace` must be 1-64 letters, digits, `_` or `-`"));
            }
            let v = ctx.api.get(&format!("{}/workspaces/{}", repo_path(repo), enc(w)), &[], &ctx.opts(run))?;
            let head = v.get("head").and_then(Value::as_str).unwrap_or("").to_string();
            if head.is_empty() {
                return Err(ToolError::local("failed_precondition", format!("Workspace `{w}` has no changes yet."))
                    .hint("Write files with workspace_write_files first."));
            }
            Ok((head, v.get("stream").and_then(Value::as_str).map(str::to_string)))
        }
        (None, Some(h)) => Ok((h.to_string(), None)),
        _ => Err(ToolError::invalid(
            "give exactly one of `workspace` (propose its current head) or `head` (a changeset id)",
        )),
    }
}

fn propose(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let run = ctx.run_id(args)?;
    let title = str_arg(args, "title").unwrap_or_default().trim();
    if title.is_empty() {
        return Err(ToolError::invalid("`title` may not be empty"));
    }
    ctx.step(0, 3, "reading the workspace");
    let (head, ws_stream) = head_and_stream(ctx, &repo, args, run)?;
    let stream = match str_arg(args, "stream").filter(|s| !s.is_empty()).map(str::to_string).or(ws_stream) {
        Some(s) => s,
        None => ctx.default_stream(&repo, run)?,
    };
    ctx.check_cancelled()?;
    ctx.step(1, 3, "publishing and opening the proposal");
    let mut body = json!({"stream": stream, "head": head, "title": title, "description": str_arg(args, "description").unwrap_or("")});
    if let Some(t) = str_arg(args, "task_id").filter(|t| !t.is_empty()) {
        body["task_id"] = json!(t);
    }
    let key = new_key("mcp-propose");
    let p =
        ctx.api.send("POST", &format!("{}/proposals", repo_path(&repo)), Some(&body), &ctx.opts_keyed(run, &key))?;
    let id = p.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    ctx.check_cancelled()?;
    ctx.step(2, 3, "reading the verdicts");
    // The four verdicts straight away, best effort: the proposal exists either way.
    let status = status_of(ctx, &repo, &id).ok();
    let mut data = json!({"proposal": proposal_brief(&p)});
    let mut summary = format!("Proposal {id} opened for stream `{}`.", crate::sanitize::attr_value(&stream));
    if let Some(s) = status {
        summary.push_str(" Its verdicts follow; poll proposal_status as reviewers and checks come in.");
        data["status"] = s;
    }
    Ok(ToolOutput::new(summary, data))
}

fn proposal_update(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let run = ctx.run_id(args)?;
    let id = str_arg(args, "proposal_id").unwrap_or_default();
    let (head, _) = head_and_stream(ctx, &repo, args, run)?;
    let p = ctx.api.send(
        "PATCH",
        &format!("{}/proposals/{}", repo_path(&repo), enc(id)),
        Some(&json!({"new_head": head})),
        &ctx.opts(run),
    )?;
    Ok(ToolOutput::new(
        format!(
            "Proposal {} now points at {}. Earlier approvals no longer count; check proposal_status.",
            crate::sanitize::attr_value(id),
            short(&head)
        ),
        json!({"proposal": proposal_brief(&p)}),
    ))
}

fn proposal_close(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let run = ctx.run_id(args)?;
    let id = str_arg(args, "proposal_id").unwrap_or_default();
    let p = ctx.api.send(
        "POST",
        &format!("{}/proposals/{}/close", repo_path(&repo), enc(id)),
        Some(&json!({})),
        &ctx.opts(run),
    )?;
    Ok(ToolOutput::new(
        format!("Proposal {} closed without integrating.", crate::sanitize::attr_value(id)),
        json!({"proposal": proposal_brief(&p)}),
    ))
}

fn request_validation(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let run = ctx.run_id(args)?;
    let id = str_arg(args, "proposal_id").unwrap_or_default();
    let checks: Vec<&str> =
        args.get("checks").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let v = ctx.api.send(
        "POST",
        &format!("{}/proposals/{}/validate", repo_path(&repo), enc(id)),
        Some(&json!({"checks": checks})),
        &ctx.opts(run),
    )?;
    let jobs: Vec<Value> = v
        .get("jobs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(50)
        .map(|j| json!({"id": j.get("id").cloned().unwrap_or(json!("")), "check": j.get("check").cloned().unwrap_or(json!("")), "state": j.get("state").cloned().unwrap_or(json!(""))}))
        .collect();
    Ok(ToolOutput::new(
        format!(
            "{} validation jobs queued or already running for the candidate tree. Results arrive as signed attestations; poll proposal_status.",
            jobs.len()
        ),
        json!({"jobs": jobs}),
    ))
}

fn review(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let id = str_arg(args, "proposal_id").unwrap_or_default();
    let reviewed = str_arg(args, "head").unwrap_or_default();
    let decision = str_arg(args, "decision").unwrap_or_default();
    let body = str_arg(args, "body").unwrap_or_default().trim();
    if decision != "approve" && body.is_empty() {
        return Err(ToolError::invalid(
            "`body` is required unless the decision is `approve`: say what must change or what you noticed",
        ));
    }
    // An approval binds to the head that was looked at: refuse if the proposal moved since.
    let d = ctx.api.get(&format!("{}/proposals/{}", repo_path(&repo), enc(id)), &[], &ctx.opts(None))?;
    let current = d.get("proposal").and_then(|p| p.get("head")).and_then(Value::as_str).unwrap_or("");
    if current != reviewed {
        return Err(ToolError::local(
            "head_changed",
            format!(
                "The proposal's head is now {}, not {} that you reviewed. Nothing was recorded.",
                short(current),
                short(reviewed)
            ),
        )
        .hint("Read the new head with proposal_status, look at it (diff, read_file), then review again with the new `head`."));
    }
    ctx.check_cancelled()?;
    let r = ctx.api.send(
        "POST",
        &format!("{}/proposals/{}/reviews", repo_path(&repo), enc(id)),
        Some(&json!({"decision": decision, "body": body})),
        &ctx.opts(None),
    )?;
    Ok(ToolOutput::new(
        format!("Recorded `{decision}` on proposal {} for head {}.", crate::sanitize::attr_value(id), short(reviewed)),
        json!({"review": {
            "id": r.get("id").cloned().unwrap_or(json!(0)),
            "proposal": r.get("proposal").cloned().unwrap_or(json!(id)),
            "reviewer": r.get("reviewer").cloned().unwrap_or(json!("")),
            "decision": r.get("decision").cloned().unwrap_or(json!(decision)),
            "head": r.get("head").cloned().unwrap_or(json!(reviewed)),
        }}),
    ))
}

fn integrate(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    eprintln!("ag-mcp: WARNING: the integrate tool is being used: this can move a stream head");
    let repo = ctx.repo(args)?;
    let run = ctx.run_id(args)?;
    let id = str_arg(args, "proposal_id").unwrap_or_default();
    let tokens: Vec<u64> =
        args.get("fencing_tokens").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).collect();
    let key = new_key("mcp-integrate");
    let v = ctx.api.send(
        "POST",
        &format!("{}/proposals/{}/integrate", repo_path(&repo), enc(id)),
        Some(&json!({"fencing_tokens": tokens, "idempotency_key": key})),
        &ctx.opts_keyed(run, &key),
    )?;
    let o = v.get("outcome").cloned().unwrap_or(Value::Null);
    let status = o.get("status").and_then(Value::as_str).unwrap_or("unknown").to_string();
    let blockers: Vec<Value> =
        o.get("blockers").and_then(Value::as_array).into_iter().flatten().map(explain_blocker).collect();
    let conflicts: Vec<Value> =
        o.get("conflicts").and_then(Value::as_array).into_iter().flatten().take(100).cloned().collect();
    let summary = match status.as_str() {
        "integrated" => format!(
            "INTEGRATED: proposal {} landed ({} files); the stream head is now {}.",
            crate::sanitize::attr_value(id),
            o.get("changed_files").and_then(Value::as_u64).unwrap_or(0),
            short(o.get("new_head").and_then(Value::as_str).unwrap_or(""))
        ),
        "blocked" => {
            format!("NOT integrated: blocked by {} requirements that someone can still provide.", blockers.len())
        }
        "conflict" => format!("NOT integrated: {} conflicting paths.", conflicts.len()),
        "denied" => "NOT integrated: refused by policy.".to_string(),
        "invalid" => "NOT integrated: the request was invalid.".to_string(),
        other => format!("Integration finished with status `{}`.", crate::sanitize::attr_value(other)),
    };
    Ok(ToolOutput::new(
        summary,
        json!({
            "status": status,
            "operation_id": v.get("operation_id").cloned().unwrap_or(json!("")),
            "new_head": o.get("new_head").cloned().unwrap_or(json!("")),
            "previous_head": o.get("previous_head").cloned().unwrap_or(json!("")),
            "strategy": o.get("strategy").cloned().unwrap_or(json!("")),
            "changed_files": o.get("changed_files").cloned().unwrap_or(json!(0)),
            "blockers": blockers,
            "conflicts": conflicts,
            "reason": o.get("reason").cloned().unwrap_or(json!("")),
        }),
    ))
}

/// The tools of the propose, review and integrate groups.
pub fn tools() -> Vec<ToolDef> {
    let ws_or_head = || {
        vec![
            ("workspace", t_str_max("Propose the current head of this workspace.", 64)),
            ("head", t_str("Or propose this changeset id.")),
        ]
    };
    vec![
        def(
            "propose",
            Group::Propose,
            "Open a proposal",
            "Publishes a workspace head (or a changeset) and opens a proposal to integrate it into a stream. This does NOT land anything: policy, validation and reviewers decide that. The result already contains the four verdicts and what blocks them.",
            Ann::ADD,
            input(
                {
                    let mut p = vec![p_repo()];
                    p.extend(ws_or_head());
                    p.extend([
                        ("title", t_str_max("One-line title.", 300)),
                        ("description", t_str_max("What changed and why; what was tested. Plain text.", 8000)),
                        ("stream", t_str("Target stream (default: the workspace's stream).")),
                        ("task_id", t_str("The task this proposal completes.")),
                        p_run(),
                    ]);
                    p
                },
                &["title"],
            ),
            output(vec![("proposal", o_obj()), ("status", o_obj())], &["proposal"]),
            propose,
        ),
        def(
            "proposal_update",
            Group::Propose,
            "Update a proposal",
            "Moves an open proposal to a newer head (after more workspace_write_files). Approvals of the old head stop counting.",
            Ann::SET,
            input(
                {
                    let mut p = vec![p_repo(), ("proposal_id", t_str("Proposal id (prop_…)."))];
                    p.extend(ws_or_head());
                    p.push(p_run());
                    p
                },
                &["proposal_id"],
            ),
            output(vec![("proposal", o_obj())], &["proposal"]),
            proposal_update,
        ),
        def(
            "proposal_close",
            Group::Propose,
            "Close a proposal",
            "Closes a proposal without integrating it. Use it to withdraw work that should not land.",
            Ann::DESTROY,
            input(vec![p_repo(), ("proposal_id", t_str("Proposal id (prop_…).")), p_run()], &["proposal_id"]),
            output(vec![("proposal", o_obj())], &["proposal"]),
            proposal_close,
        ),
        def(
            "request_validation",
            Group::Propose,
            "Request validation",
            "Queues the policy's required checks for a proposal's candidate tree on the execution plane (trusted runners). Results come back as signed attestations bound to that exact tree; poll proposal_status.",
            Ann::ADD,
            input(
                vec![
                    p_repo(),
                    ("proposal_id", t_str("Proposal id (prop_…).")),
                    (
                        "checks",
                        t_arr(
                            "Only these checks (default: all required checks that have a runner).",
                            t_str_max("A check name.", 120),
                            20,
                        ),
                    ),
                    p_run(),
                ],
                &["proposal_id"],
            ),
            output(vec![("jobs", o_arr())], &["jobs"]),
            request_validation,
        ),
        def(
            "review",
            Group::Review,
            "Review a proposal",
            "Records approve, request_changes or comment on a proposal. You must pass the `head` you actually reviewed (from proposal_status); if the proposal moved, nothing is recorded. The author cannot approve their own work, and the policy decides who counts as an approver.",
            Ann::ADD,
            input(
                vec![
                    p_repo(),
                    ("proposal_id", t_str("Proposal id (prop_…).")),
                    ("head", t_str("The proposal head you reviewed.")),
                    ("decision", t_enum("Your decision.", &["approve", "request_changes", "comment"])),
                    ("body", t_str_max("Your reasoning. Required unless approving.", 4000)),
                ],
                &["proposal_id", "head", "decision"],
            ),
            output(vec![("review", o_obj())], &["review"]),
            review,
        ),
        def(
            "integrate",
            Group::Integrate,
            "Integrate a proposal",
            "LANDS a proposal on its stream if policy allows. Enabled only because the operator passed --enable-integrate. Blocked, conflicting and denied outcomes change nothing and are explained. Pass the fencing tokens of exclusive leases you hold if their scopes are touched.",
            Ann::DESTROY,
            input(
                vec![
                    p_repo(),
                    ("proposal_id", t_str("Proposal id (prop_…).")),
                    (
                        "fencing_tokens",
                        t_arr(
                            "Fencing tokens of exclusive leases you hold (from lease_acquire).",
                            json!({"type": "integer", "minimum": 0}),
                            20,
                        ),
                    ),
                    p_run(),
                ],
                &["proposal_id"],
            ),
            output(
                vec![("status", o_str()), ("blockers", o_arr()), ("conflicts", o_arr())],
                &["status", "blockers", "conflicts"],
            ),
            integrate,
        ),
    ]
}
