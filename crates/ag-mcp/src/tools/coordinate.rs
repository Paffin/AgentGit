//! The `coordinate` group: runs, task claims, leases and fencing tokens.

use serde_json::{Value, json};

use super::read::{lease_brief, task_brief};
use super::*;
use crate::api::{enc, repo_path};
use crate::config::Group;
use crate::output::{ToolError, ToolOutput};

fn run_json(r: &Value) -> Value {
    json!({
        "run_id": r["run_id"], "agent": r["agent"], "identity": r["identity"],
        "task_id": r.get("task_id").cloned().unwrap_or(json!("")),
        "status": r.get("status").cloned().unwrap_or(json!("running")),
    })
}

/// The agent name to start a run for: the argument, or the credential's own name when it is an agent.
fn agent_name(ctx: &CallCtx<'_>, args: &Value) -> Result<String, ToolError> {
    if let Some(a) = str_arg(args, "agent").filter(|a| !a.is_empty()) {
        return Ok(a.trim_start_matches("agent:").to_string());
    }
    let who = ctx.api.get("/v1/whoami", &[], &ctx.opts(None))?;
    let id = who.get("identity").and_then(Value::as_str).unwrap_or("");
    match id.split('#').next().and_then(|p| p.strip_prefix("agent:")) {
        Some(name) if !name.is_empty() => Ok(name.to_string()),
        _ => Err(ToolError::invalid(format!(
            "this credential authenticates as `{}`, which is not an agent: pass `agent` (the registered agent name to run as)",
            crate::sanitize::attr_value(id)
        ))),
    }
}

fn start_run(ctx: &CallCtx<'_>, args: &Value, task: Option<&str>) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let agent = agent_name(ctx, args)?;
    let mut body = json!({"agent": agent});
    if let Some(t) = task {
        body["task_id"] = json!(t);
    }
    let key = new_key("mcp-run");
    let r = ctx.api.send("POST", &format!("{}/runs", repo_path(&repo)), Some(&body), &ctx.opts_keyed(None, &key))?;
    let run = run_json(&r);
    let id = run["run_id"].as_str().unwrap_or("").to_string();
    let mut data = json!({"run": run});
    if let Some(t) = task {
        // show what was claimed; best effort
        if let Ok(v) = ctx.api.get(&format!("{}/tasks/{}", repo_path(&repo), enc(t)), &[], &ctx.opts(None)) {
            data["task"] = task_brief(&v);
        }
    }
    let summary = match task {
        Some(t) => format!(
            "Task {} claimed. Run {id} started as {}. Pass run_id=\"{id}\" to the calls that act as this run.",
            crate::sanitize::attr_value(t),
            crate::sanitize::attr_value(r.get("identity").and_then(Value::as_str).unwrap_or(""))
        ),
        None => format!(
            "Run {id} started as {}. Pass run_id=\"{id}\" to the calls that act as this run.",
            crate::sanitize::attr_value(r.get("identity").and_then(Value::as_str).unwrap_or(""))
        ),
    };
    Ok(ToolOutput::new(summary, data)
        .note("Finish the run with run_finish when you are done: it releases the leases the run holds."))
}

fn task_claim(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let task = str_arg(args, "task_id").unwrap_or_default();
    start_run(ctx, args, Some(task))
}

fn run_start(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    start_run(ctx, args, None)
}

fn task_complete(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let run = ctx.run_id(args)?;
    let id = str_arg(args, "task_id").unwrap_or_default();
    let state = str_arg(args, "state").unwrap_or("done");
    let t = ctx.api.send(
        "POST",
        &format!("{}/tasks/{}/finish", repo_path(&repo), enc(id)),
        Some(&json!({"state": state})),
        &ctx.opts(run),
    )?;
    Ok(ToolOutput::new(
        format!(
            "Task {} marked {state}. Only the owner can do this; call run_finish to end your run.",
            crate::sanitize::attr_value(id)
        ),
        json!({"task": task_brief(&t)}),
    ))
}

fn run_heartbeat(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let id = require_run(args)?;
    let v = ctx.api.send(
        "POST",
        &format!("{}/runs/{}/heartbeat", repo_path(&repo), enc(&id)),
        Some(&json!({})),
        &ctx.opts(None),
    )?;
    Ok(ToolOutput::new(
        format!("Heartbeat recorded for run {id}."),
        json!({"run_id": id, "server_time_ms": v.get("server_time_ms").cloned().unwrap_or(json!(0))}),
    ))
}

fn require_run(args: &Value) -> Result<String, ToolError> {
    let id = str_arg(args, "run_id").unwrap_or_default();
    if id.is_empty() || id.len() > 80 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(ToolError::invalid("`run_id` (from task_claim or run_start) is required"));
    }
    Ok(id.to_string())
}

fn run_finish(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let id = require_run(args)?;
    let status = str_arg(args, "status").unwrap_or("completed");
    let r = ctx.api.send(
        "POST",
        &format!("{}/runs/{}/complete", repo_path(&repo), enc(&id)),
        Some(&json!({"status": status, "summary": str_arg(args, "summary").unwrap_or("")})),
        &ctx.opts(None),
    )?;
    Ok(ToolOutput::new(
        format!("Run {id} finished as {status}; its leases were released. The run_id can no longer be used."),
        json!({"run": run_json(&r)}),
    ))
}

fn lease_acquire(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let run = ctx.run_id(args)?;
    let scope = str_arg(args, "scope").unwrap_or_default();
    crate::config::glob(scope).map_err(ToolError::invalid)?;
    let exclusive = bool_arg(args, "exclusive", false);
    let ttl = u64_arg(args, "ttl_seconds").unwrap_or(900);
    if ttl > ctx.cfg.max_lease_seconds {
        return Err(ToolError::refused(
            "lease_too_long",
            format!("The operator limits leases to {} seconds (you asked for {ttl}).", ctx.cfg.max_lease_seconds),
        )
        .hint("Ask for a shorter lease and renew it with lease_renew while you work."));
    }
    let mut body = json!({"scope": scope, "exclusive": exclusive, "ttl_ms": ttl * 1000});
    if let Some(t) = str_arg(args, "task_id").filter(|t| !t.is_empty()) {
        body["task_id"] = json!(t);
    }
    let key = new_key("mcp-lease");
    let v = ctx.api.send("POST", &format!("{}/leases", repo_path(&repo)), Some(&body), &ctx.opts_keyed(run, &key))?;
    let lease = lease_brief(v.get("lease").unwrap_or(&Value::Null));
    let overlapping: Vec<Value> =
        v.get("overlapping").and_then(Value::as_array).into_iter().flatten().take(50).map(lease_brief).collect();
    let mut summary = if exclusive {
        format!(
            "Exclusive lease on `{}` held for {} s. Fencing token {}: pass it as a fencing token when this change is integrated.",
            crate::sanitize::attr_value(scope),
            ttl,
            lease["fencing_token"]
        )
    } else {
        format!(
            "Soft lease on `{}` held for {} s (advisory: it signals your work, it blocks nobody).",
            crate::sanitize::attr_value(scope),
            ttl
        )
    };
    if !overlapping.is_empty() {
        summary.push_str(&format!(
            " {} overlapping leases belong to others: coordinate or choose a different scope.",
            overlapping.len()
        ));
    }
    Ok(ToolOutput::new(summary, json!({"lease": lease, "overlapping": overlapping})))
}

fn lease_renew(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let run = ctx.run_id(args)?;
    let id = str_arg(args, "lease_id").unwrap_or_default();
    let ttl = u64_arg(args, "ttl_seconds").unwrap_or(900);
    if ttl > ctx.cfg.max_lease_seconds {
        return Err(ToolError::refused(
            "lease_too_long",
            format!("The operator limits leases to {} seconds.", ctx.cfg.max_lease_seconds),
        ));
    }
    let v = ctx.api.send(
        "POST",
        &format!("{}/leases/{}/extend", repo_path(&repo), enc(id)),
        Some(&json!({"ttl_ms": ttl * 1000})),
        &ctx.opts(run),
    )?;
    let l = lease_brief(&v);
    Ok(ToolOutput::new(
        format!("Lease {} extended; the fencing token is unchanged.", crate::sanitize::attr_value(id)),
        json!({"lease": l}),
    ))
}

fn lease_release(ctx: &CallCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
    let repo = ctx.repo(args)?;
    let run = ctx.run_id(args)?;
    let id = str_arg(args, "lease_id").unwrap_or_default();
    ctx.api.send("DELETE", &format!("{}/leases/{}", repo_path(&repo), enc(id)), None, &ctx.opts(run))?;
    Ok(ToolOutput::new(
        format!("Lease {} released.", crate::sanitize::attr_value(id)),
        json!({"lease_id": id, "released": true}),
    ))
}

/// The tools of the coordinate group.
pub fn tools() -> Vec<ToolDef> {
    let agent = || ("agent", t_str("Registered agent name to run as. Default: the credential's own agent name."));
    vec![
        def(
            "task_claim",
            Group::Coordinate,
            "Claim a task",
            "Claims an open, unblocked task for you and starts a run (an execution of this agent with its own identity, agent:name#run). Returns the run_id: pass it to later calls so that work, leases and proposals are attributed to this run. Fails if the task is blocked or owned by someone else.",
            Ann::ADD,
            input(vec![p_repo(), ("task_id", t_str("The task id (from task_list).")), agent()], &["task_id"]),
            output(vec![("run", o_obj()), ("task", o_obj())], &["run"]),
            task_claim,
        ),
        def(
            "run_start",
            Group::Coordinate,
            "Start a run",
            "Starts a run without claiming a task: an execution of this agent with its own identity (agent:name#run). Returns the run_id to pass to later calls.",
            Ann::ADD,
            input(vec![p_repo(), agent()], &[]),
            output(vec![("run", o_obj())], &["run"]),
            run_start,
        ),
        def(
            "task_complete",
            Group::Coordinate,
            "Complete a task",
            "Marks a task done (or cancelled). Only the task's owner can. This records that the work is finished; it does not land any code.",
            Ann::SET,
            input(
                vec![
                    p_repo(),
                    ("task_id", t_str("The task id.")),
                    ("state", t_enum("`done` (default) or `cancelled`.", &["done", "cancelled"])),
                    p_run(),
                ],
                &["task_id"],
            ),
            output(vec![("task", o_obj())], &["task"]),
            task_complete,
        ),
        def(
            "run_heartbeat",
            Group::Coordinate,
            "Run heartbeat",
            "Tells the server this run is alive. Silent runs are marked stale and lose their leases; send one at least every few minutes during long work.",
            Ann::SET,
            input(vec![p_repo(), ("run_id", t_str("The run id."))], &["run_id"]),
            output(vec![("run_id", o_str())], &["run_id"]),
            run_heartbeat,
        ),
        def(
            "run_finish",
            Group::Coordinate,
            "Finish a run",
            "Ends a run (completed or failed) and releases every lease it holds. Do this when you are done or giving up.",
            Ann::SET,
            input(
                vec![
                    p_repo(),
                    ("run_id", t_str("The run id.")),
                    ("status", t_enum("`completed` (default) or `failed`.", &["completed", "failed"])),
                    ("summary", t_str_max("One or two sentences for the record.", 2000)),
                ],
                &["run_id"],
            ),
            output(vec![("run", o_obj())], &["run"]),
            run_finish,
        ),
        def(
            "lease_acquire",
            Group::Coordinate,
            "Acquire a lease",
            "Claims a path scope (a glob such as src/auth/**). A soft lease only signals your work. An exclusive lease is refused while another exclusive lease overlaps, and is ENFORCED at integration: while it is active only its holder, presenting its fencing token, can land changes in that scope. Leases expire; renew them while you work.",
            Ann::ADD,
            input(
                vec![
                    p_repo(),
                    (
                        "scope",
                        t_str_max(
                            "Path glob, e.g. src/auth/** (`*` stays inside a directory, `**` crosses directories).",
                            512,
                        ),
                    ),
                    ("exclusive", t_bool("Exclusive lease with a fencing token. Default false (soft).")),
                    ("ttl_seconds", t_int("Lifetime in seconds. Default 900; the operator may cap it.", 30, 86_400)),
                    ("task_id", t_str("The task this lease is for.")),
                    p_run(),
                ],
                &["scope"],
            ),
            output(vec![("lease", o_obj()), ("overlapping", o_arr())], &["lease"]),
            lease_acquire,
        ),
        def(
            "lease_renew",
            Group::Coordinate,
            "Renew a lease",
            "Extends a lease you hold by another ttl_seconds from now. The fencing token stays the same.",
            Ann::SET,
            input(
                vec![
                    p_repo(),
                    ("lease_id", t_str("The lease id.")),
                    ("ttl_seconds", t_int("New lifetime in seconds from now. Default 900.", 30, 86_400)),
                    p_run(),
                ],
                &["lease_id"],
            ),
            output(vec![("lease", o_obj())], &["lease"]),
            lease_renew,
        ),
        def(
            "lease_release",
            Group::Coordinate,
            "Release a lease",
            "Releases a lease you hold. Changes you already proposed are not affected.",
            Ann::SET,
            input(vec![p_repo(), ("lease_id", t_str("The lease id.")), p_run()], &["lease_id"]),
            output(vec![("lease_id", o_str()), ("released", o_bool())], &["released"]),
            lease_release,
        ),
    ]
}
