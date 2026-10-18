//! Task ownership and dependency awareness.

use std::collections::{BTreeSet, HashSet};

use ag_crypto::random_id;
use ag_storage::Meta;
use ag_storage::meta::{MetaError, TaskRow, Tx};
use serde_json::json;

use crate::AgentError;

/// Creates a task. `scope` lists the path globs the task is expected to touch.
pub fn create_task(
    meta: &Meta,
    title: &str,
    description: &str,
    scope: &[String],
    actor: &str,
) -> Result<TaskRow, AgentError> {
    create_task_with(meta, title, description, scope, &[], actor, |_, _| Ok(())).map(|(t, ())| t)
}

/// Creates a task that depends on `depends_on` — all in one transaction — and runs `also` in it too: whatever it
/// writes (an idempotency record, say) is committed with the task or not at all. `also` may run again if the
/// transaction is retried, and its error rolls the task back. A dependency that does not exist is
/// [`AgentError::NotFound`].
pub fn create_task_with<R>(
    meta: &Meta,
    title: &str,
    description: &str,
    scope: &[String],
    depends_on: &[String],
    actor: &str,
    also: impl Fn(&Tx<'_>, &TaskRow) -> Result<R, MetaError>,
) -> Result<(TaskRow, R), AgentError> {
    if title.trim().is_empty() {
        return Err(AgentError::Invalid("task title is empty".into()));
    }
    for g in scope {
        ag_policy::PathMatcher::new(std::slice::from_ref(g)).map_err(AgentError::Invalid)?;
    }
    let id = random_id("task")?;
    meta.write(|tx| {
        let t = tx.task_create(&id, title, description, scope, actor)?;
        tx.audit_append(actor, "task.create", &id, &json!({"title": title}))?;
        for d in depends_on {
            add_dependency_in_tx(tx, &id, d, actor)?;
        }
        let extra = also(tx, &t)?;
        Ok((t, extra))
    })
    .map_err(|e| match e {
        MetaError::Invalid(m) => AgentError::Conflict(m),
        MetaError::NotFound(m) => AgentError::NotFound(m),
        other => AgentError::Meta(other),
    })
}

/// Makes `task` depend on `depends_on`, refusing self-dependencies and cycles.
pub fn add_dependency(meta: &Meta, task: &str, depends_on: &str, actor: &str) -> Result<(), AgentError> {
    if task == depends_on {
        return Err(AgentError::Invalid("a task cannot depend on itself".into()));
    }
    meta.write(|tx| add_dependency_in_tx(tx, task, depends_on, actor)).map_err(|e| match e {
        MetaError::Invalid(m) => AgentError::Conflict(m),
        MetaError::NotFound(m) => AgentError::NotFound(m),
        other => AgentError::Meta(other),
    })
}

fn add_dependency_in_tx(tx: &Tx<'_>, task: &str, depends_on: &str, actor: &str) -> Result<(), MetaError> {
    for t in [task, depends_on] {
        if tx.task(t)?.is_none() {
            return Err(MetaError::NotFound(format!("task '{t}'")));
        }
    }
    // Adding task -> depends_on creates a cycle iff depends_on already reaches task.
    if reaches(tx, depends_on, task)? {
        return Err(MetaError::Invalid(format!("dependency {task} -> {depends_on} would create a cycle")));
    }
    tx.task_dep_add(task, depends_on)?;
    tx.audit_append(actor, "task.depend", task, &json!({"on": depends_on}))?;
    Ok(())
}

fn reaches(tx: &Tx<'_>, from: &str, to: &str) -> Result<bool, MetaError> {
    let mut seen = HashSet::new();
    let mut stack = vec![from.to_string()];
    while let Some(cur) = stack.pop() {
        if cur == to {
            return Ok(true);
        }
        if seen.insert(cur.clone()) {
            stack.extend(tx.task_deps(&cur)?);
        }
    }
    Ok(false)
}

/// Dependencies of `task` that are not done yet.
pub fn unfinished_dependencies(tx: &Tx<'_>, task: &str) -> Result<Vec<String>, MetaError> {
    let mut out = Vec::new();
    for d in tx.task_deps(task)? {
        if tx.task(&d)?.is_some_and(|t| t.state != "done") {
            out.push(d);
        }
    }
    Ok(out)
}

/// Claims a task for `owner` and marks it in progress. Fails if it is owned by
/// someone else, finished, or waiting on unfinished dependencies.
pub(crate) fn claim_in_tx(tx: &Tx<'_>, task: &str, owner: &str) -> Result<(), MetaError> {
    let t = tx.task(task)?.ok_or_else(|| MetaError::NotFound(format!("task '{task}'")))?;
    if t.state == "done" || t.state == "cancelled" {
        return Err(MetaError::Invalid(format!("task '{task}' is {}", t.state)));
    }
    if let Some(o) = t.owner.as_ref().filter(|o| *o != owner) {
        return Err(MetaError::Invalid(format!("task '{task}' is owned by {o}")));
    }
    let waiting = unfinished_dependencies(tx, task)?;
    if !waiting.is_empty() {
        return Err(MetaError::Invalid(format!(
            "task '{task}' is blocked by unfinished tasks: {}",
            waiting.join(", ")
        )));
    }
    tx.task_update(task, Some(owner), Some("in_progress"))
}

/// Marks a task done or cancelled. Only its owner (or anyone, if unowned) may; an owner acting as a run
/// (`agent:bot#run_…`) may only finish the task *that run* claimed - another run of the same agent has no say in
/// it. Use [`finish_task_as_operator`] for an operator that overrides the owner.
pub fn finish_task(meta: &Meta, task: &str, state: &str, actor: &str) -> Result<(), AgentError> {
    finish_task_inner(meta, task, state, actor, false)
}

/// Like [`finish_task`] for an operator: the task is finished whoever owns it. The audit entry says that it was
/// an operator's doing and whose task it overrode, so the log never shows an owner finishing what an operator
/// finished for it.
pub fn finish_task_as_operator(meta: &Meta, task: &str, state: &str, actor: &str) -> Result<(), AgentError> {
    finish_task_inner(meta, task, state, actor, true)
}

fn finish_task_inner(meta: &Meta, task: &str, state: &str, actor: &str, operator: bool) -> Result<(), AgentError> {
    if !["done", "cancelled"].contains(&state) {
        return Err(AgentError::Invalid("state must be 'done' or 'cancelled'".into()));
    }
    let (principal, run_id) = match actor.split_once('#') {
        Some((p, r)) => (p, Some(r)),
        None => (actor, None),
    };
    meta.write(|tx| {
        let t = tx.task(task)?.ok_or_else(|| MetaError::NotFound(format!("task '{task}'")))?;
        let mut details = json!({"state": state});
        match t.owner.as_ref() {
            Some(o) if operator => {
                // Recorded only when the operator really overrides somebody.
                if o != principal {
                    details["operator"] = json!(true);
                    details["owner"] = json!(o);
                }
            }
            Some(o) if o != principal => {
                return Err(MetaError::Invalid(format!("task '{task}' is owned by {o}")));
            }
            Some(_) => {
                if let Some(run_id) = run_id {
                    let claimed_by_this_run = tx.run(run_id)?.is_some_and(|r| {
                        r.task_id.as_deref() == Some(task) && format!("agent:{}", r.agent) == principal
                    });
                    if !claimed_by_this_run {
                        return Err(MetaError::Invalid(format!("task '{task}' was not claimed by run '{run_id}'")));
                    }
                }
            }
            None => {}
        }
        tx.task_update(task, None, Some(state))?;
        tx.audit_append(actor, "task.finish", task, &details).map(|_| ())
    })
    .map_err(|e| match e {
        MetaError::Invalid(m) => AgentError::Conflict(m),
        MetaError::NotFound(m) => AgentError::NotFound(m),
        other => AgentError::Meta(other),
    })
}

/// Open tasks whose dependencies are all done: what an agent may start now.
pub fn ready_tasks(meta: &Meta) -> Result<Vec<TaskRow>, AgentError> {
    Ok(meta.read(|tx| {
        let mut out = Vec::new();
        for t in tx.tasks(Some("open"))? {
            if unfinished_dependencies(tx, &t.id)?.is_empty() {
                out.push(t);
            }
        }
        Ok(out)
    })?)
}

/// All tasks (transitively) blocked behind `task`.
pub fn dependents(meta: &Meta, task: &str) -> Result<BTreeSet<String>, AgentError> {
    Ok(meta.read(|tx| {
        let all = tx.tasks(None)?;
        let mut out = BTreeSet::new();
        let mut frontier = vec![task.to_string()];
        while let Some(cur) = frontier.pop() {
            for t in &all {
                if tx.task_deps(&t.id)?.contains(&cur) && out.insert(t.id.clone()) {
                    frontier.push(t.id.clone());
                }
            }
        }
        Ok(out)
    })?)
}
