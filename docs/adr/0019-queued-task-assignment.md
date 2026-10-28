# ADR 0019: reserve an agent before a task starts

Status: accepted (development branch; not in beta.3).

## Context

A task is an executable unit with an expected result, file scope and prerequisites. A person must be able to choose a registered agent without racing the orchestrator's next queue poll. AgentGit must enforce that choice at the task claim, not just display it in the browser.

## Decision

Reuse the task's optional owner while its state is `open`. `task.create.assignee` names an enabled registered agent and sets its identity in the same transaction as creation, issue handoff and idempotency registration. A failed assignment rolls back all of these writes. An empty assignee preserves automatic routing and the previous create-request fingerprint.

`task.assign` accepts an agent name (empty restores automatic routing) and `expected_owner`. Only the task creator or an operator may change the assignment, and only while the task is queued with no running session. The expected owner prevents a stale edit from overwriting another person's decision. Assignment and its audit/outbox events commit together. The existing atomic task claim rejects a different agent.

The orchestrator respects the reserved owner for open and in-progress tasks. It waits when it has no configured executor for that agent; it never silently substitutes another. Registration is not executor installation. The Web UI loads enabled agents when the form opens and supports both assignment at creation and a permanent task-page edit. Servers advertise `tasks.assignment`; older servers retain their existing workflow. No metadata schema change is needed.

## Evidence and limits

Focused engine and HTTP scenarios cover competing agents, reassignment, stale updates, running-task rejection, return to automatic routing and rollback of an issue-linked creation. A real command-executor scenario checks that queue dispatch uses the assigned executor rather than the default. Browser coverage exercises creation and reassignment through the actual API.

This does not configure models, install executors or claim autonomous delivery. Run inspection, exact-tree checks, review and policy-controlled integration remain distinct steps in the development cycle.
