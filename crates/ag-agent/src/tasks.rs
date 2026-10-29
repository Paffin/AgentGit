//! Task ownership and dependency awareness.

use std::collections::{BTreeSet, HashSet};

use ag_crypto::random_id;
use ag_storage::Meta;
use ag_storage::meta::{MetaError, TaskRow, Tx};
use serde_json::json;

use crate::AgentError;

/// Editable instructions, compared as a whole rather than by a wall-clock timestamp.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskContent {
    /// Short description of the desired result.
    pub title: String,
    /// Detailed instructions.
    pub description: String,
    /// Expected file-path globs.
    pub scope: Vec<String>,
}

impl TaskContent {
    /// Validates user-supplied instructions using the same rules as task creation.
    pub fn validate(&self) -> Result<(), String> {
        validate_content(&self.title, &self.scope)
    }
}

impl From<&TaskRow> for TaskContent {
    fn from(task: &TaskRow) -> Self {
        Self { title: task.title.clone(), description: task.description.clone(), scope: task.scope.clone() }
    }
}

fn validate_content(title: &str, scope: &[String]) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("task title is empty".into());
    }
    for g in scope {
        ag_policy::PathMatcher::new(std::slice::from_ref(g))?;
    }
    Ok(())
}

/// Refines a task before its first run. Content, authorization, history, audit and
/// caller-supplied idempotency records share the caller's write transaction.
pub fn update_in(
    tx: &Tx<'_>,
    id: &str,
    content: &TaskContent,
    expected: &TaskContent,
    actor: &str,
    operator: bool,
) -> Result<TaskRow, MetaError> {
    content.validate().map_err(MetaError::Invalid)?;
    let task = tx.task(id)?.ok_or_else(|| MetaError::NotFound(format!("task '{id}'")))?;
    check_refinement(tx, &task, expected, actor, operator)?;
    if TaskContent::from(&task) == *content {
        return Ok(task);
    }
    persist_content(tx, &task, content, actor, operator)
}

fn persist_content(
    tx: &Tx<'_>,
    task: &TaskRow,
    content: &TaskContent,
    actor: &str,
    operator: bool,
) -> Result<TaskRow, MetaError> {
    tx.task_set_content(&task.id, &content.title, &content.description, &content.scope)?;
    tx.audit_append(
        actor,
        "task.update",
        &task.id,
        &json!({"before":TaskContent::from(task),"after":content,"operator":operator}),
    )?;
    tx.outbox_push("task.update", &json!({"id":task.id}))?;
    tx.task(&task.id)?.ok_or_else(|| MetaError::Corrupt("updated task missing".into()))
}

fn check_refinement(
    tx: &Tx<'_>,
    task: &TaskRow,
    expected: &TaskContent,
    actor: &str,
    operator: bool,
) -> Result<(), MetaError> {
    if !operator && task.created_by.split('#').next() != actor.split('#').next() {
        return Err(MetaError::Invalid("only the task creator or an operator can edit it".into()));
    }
    if task.state != "open" || !tx.runs_page(None, Some(&task.id), false, None, 1)?.is_empty() {
        return Err(MetaError::Invalid(
            "task instructions are immutable after the first run; create a follow-up task".into(),
        ));
    }
    if TaskContent::from(task) != *expected {
        return Err(MetaError::Invalid("task instructions changed; reload before saving".into()));
    }
    Ok(())
}

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
    validate_content(title, scope).map_err(AgentError::Invalid)?;
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

/// Records validated, immutable links while creating a separate follow-up task.
pub fn attach_source(
    tx: &Tx<'_>,
    task: &str,
    source_task: &str,
    source_run: &str,
    actor: &str,
) -> Result<(), MetaError> {
    if source_task.is_empty() {
        return if source_run.is_empty() {
            Ok(())
        } else {
            Err(MetaError::Invalid("source_run requires source_task".into()))
        };
    }
    if task == source_task {
        return Err(MetaError::Invalid("a task cannot be its own source".into()));
    }
    tx.task(source_task)?.ok_or_else(|| MetaError::NotFound("source task not found".into()))?;
    let run = (!source_run.is_empty()).then_some(source_run);
    validate_source_run(tx, source_task, run)?;
    tx.task_link_source(task, source_task, run)?;
    tx.audit_append(actor, "task.source", task, &json!({"source_task":source_task,"source_run":run}))?;
    Ok(())
}

fn validate_source_run(tx: &Tx<'_>, source_task: &str, run: Option<&str>) -> Result<(), MetaError> {
    if let Some(id) = run {
        let row = tx.run(id)?.ok_or_else(|| MetaError::NotFound("source run not found".into()))?;
        if row.task_id.as_deref() != Some(source_task) || !["failed", "abandoned"].contains(&row.status.as_str()) {
            return Err(MetaError::Invalid(
                "source run must be failed or abandoned and belong to the source task".into(),
            ));
        }
    }
    Ok(())
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
    finish_task_with_limit(meta, task, state, actor, false, None)
}

/// Like [`finish_task`] for an operator: the task is finished whoever owns it. The audit entry says that it was
/// an operator's doing and whose task it overrode, so the log never shows an owner finishing what an operator
/// finished for it.
pub fn finish_task_as_operator(meta: &Meta, task: &str, state: &str, actor: &str) -> Result<(), AgentError> {
    finish_task_with_limit(meta, task, state, actor, true, None)
}

/// Finishes only if the optional execution-policy precondition still matches.
/// The check shares the task transition transaction; zero is a meaningful expected limit.
pub fn finish_task_with_limit(
    meta: &Meta,
    task: &str,
    state: &str,
    actor: &str,
    operator: bool,
    expected_max_attempts: Option<u32>,
) -> Result<(), AgentError> {
    if !["done", "cancelled"].contains(&state) {
        return Err(AgentError::Invalid("state must be 'done' or 'cancelled'".into()));
    }
    let (principal, run_id) = match actor.split_once('#') {
        Some((p, r)) => (p, Some(r)),
        None => (actor, None),
    };
    meta.write(|tx| {
        let t = tx.task(task)?.ok_or_else(|| MetaError::NotFound(format!("task '{task}'")))?;
        if expected_max_attempts.is_some_and(|expected| t.max_attempts != expected) {
            return Err(MetaError::Invalid("task attempt limit changed; reload before finishing".into()));
        }
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
            if unfinished_dependencies(tx, &t.id)?.is_empty()
                && (t.max_attempts == 0 || t.attempts_started < u64::from(t.max_attempts))
            {
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

/// Assigns queued work to a registered agent, or returns it to automatic routing.
/// The creator or an operator may assign only while the task is still open.
pub fn assign_task(
    meta: &Meta,
    id: &str,
    agent: &str,
    expected_owner: &str,
    actor: &str,
    operator: bool,
) -> Result<TaskRow, AgentError> {
    if !agent.is_empty() {
        crate::registry::validate_name(agent)?;
    }
    meta.write(|tx| assign_in_tx(tx, id, agent, expected_owner, actor, operator)).map_err(|e| match e {
        MetaError::Invalid(m) => AgentError::Conflict(m),
        MetaError::NotFound(m) => AgentError::NotFound(m),
        other => AgentError::Meta(other),
    })
}

/// Composes assignment with task creation and its idempotency record in one transaction.
pub fn assign_in_tx(
    tx: &Tx<'_>,
    id: &str,
    agent: &str,
    expected_owner: &str,
    actor: &str,
    operator: bool,
) -> Result<TaskRow, MetaError> {
    let task = tx.task(id)?.ok_or_else(|| MetaError::NotFound(format!("task '{id}'")))?;
    check_assignment(tx, &task, expected_owner, actor, operator)?;
    let owner = assignment_owner(tx, agent)?;
    tx.task_set_owner(id, owner.as_deref())?;
    let detail = json!({"from":task.owner,"to":owner,"operator":operator});
    tx.audit_append(actor, "task.assign", id, &detail)?;
    tx.outbox_push("task.assign", &detail)?;
    tx.task(id)?.ok_or_else(|| MetaError::Corrupt("assigned task missing".into()))
}

fn check_assignment(
    tx: &Tx<'_>,
    task: &TaskRow,
    expected_owner: &str,
    actor: &str,
    operator: bool,
) -> Result<(), MetaError> {
    if !operator && task.created_by.split('#').next() != actor.split('#').next() {
        return Err(MetaError::Invalid("only the task creator or an operator can assign it".into()));
    }
    if task.state != "open" || !tx.runs_running_for_task(&task.id)?.is_empty() {
        return Err(MetaError::Invalid("only queued tasks can be reassigned; stop the current work first".into()));
    }
    if task.owner.as_deref().unwrap_or("") != expected_owner {
        return Err(MetaError::Invalid("the task assignment changed; reload before choosing an agent".into()));
    }
    Ok(())
}

fn assignment_owner(tx: &Tx<'_>, agent: &str) -> Result<Option<String>, MetaError> {
    if agent.is_empty() {
        return Ok(None);
    }
    let registered = tx.agent(agent)?.ok_or_else(|| MetaError::NotFound(format!("agent '{agent}'")))?;
    if registered.disabled {
        return Err(MetaError::Invalid("the selected agent is disabled".into()));
    }
    Ok(Some(registered.identity))
}

/// Largest explicit attempt limit supported by the task policy and history readers.
pub const MAX_ATTEMPTS: u32 = 10_000;

/// Validates a task-local attempt limit; zero retains the legacy server behavior.
pub fn validate_attempt_limit(max: u32) -> Result<(), AgentError> {
    if max > MAX_ATTEMPTS {
        return Err(AgentError::Invalid(format!("max_attempts must be between 0 and {MAX_ATTEMPTS}")));
    }
    Ok(())
}

/// Updates execution policy without changing task instructions or run history.
/// The creator may edit before execution; afterwards an operator is required.
pub fn set_attempt_limit_in(
    tx: &Tx<'_>,
    id: &str,
    max: u32,
    expected: u32,
    actor: &str,
    operator: bool,
) -> Result<TaskRow, MetaError> {
    validate_attempt_limit(max).map_err(|e| MetaError::Invalid(e.to_string()))?;
    let task = tx.task(id)?.ok_or_else(|| MetaError::NotFound(format!("task '{id}'")))?;
    validate_attempt_update(&task, expected, actor, operator)?;
    if max == expected {
        return Ok(task);
    }
    tx.task_set_attempt_limit(id, max)?;
    tx.audit_append(
        actor,
        "task.attempts",
        id,
        &json!({"before":expected,"after":max,"attempts_started":task.attempts_started,"operator":operator}),
    )?;
    tx.outbox_push("task.attempts", &json!({"id":id,"max_attempts":max}))?;
    tx.task(id)?.ok_or_else(|| MetaError::Corrupt("updated task missing".into()))
}

fn validate_attempt_update(task: &TaskRow, expected: u32, actor: &str, operator: bool) -> Result<(), MetaError> {
    if !operator && (task.created_by.split('#').next() != actor.split('#').next() || task.attempts_started > 0) {
        return Err(MetaError::Invalid(
            "only an operator can change an attempt limit after execution starts; before that the creator may edit it"
                .into(),
        ));
    }
    if task.state == "done" || task.state == "cancelled" {
        return Err(MetaError::Invalid("finished task policy is immutable; create a follow-up task".into()));
    }
    if task.max_attempts != expected {
        return Err(MetaError::Invalid("task attempt limit changed; reload before saving".into()));
    }
    Ok(())
}
