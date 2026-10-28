# ADR 0027: Persistent sources for follow-up tasks

Status: implemented in development; local native, storage, API and browser qualification recorded in the evidence journal.

Copying failure context into a description helps a person, but does not give the
server or clients a reliable relationship. A new task can record `source_task`
and optionally `source_run` as immutable creation metadata.

The source task must exist in the same repository. A source run must belong to
that task and be failed or abandoned. A run without a source task is invalid.
Source readers need task-read permission and, for a run, agent-read permission;
task creation still requires task-write. Source validation, task creation,
audit and the idempotency response share one transaction. Invalid references
leave no task or partial issue link behind. Requests without the new fields
retain the previous idempotency fingerprint. Requests with sources use a separate
versioned JSON fingerprint so delimiters inside user input cannot imitate source
fields.

This creates independent work, not another attempt of the source task. Owner,
prerequisites and retry history are not copied implicitly. Clients may prefill
scope and description for explicit review, but these remain ordinary editable
inputs. Finishing, cancelling or assigning the new task cannot change its source.
There is no automatic task execution or change to retry limits in this decision.

Metadata schema 7 adds nullable task/source-run foreign keys to `tasks` in both
SQLite and PostgreSQL. Existing tasks have no source. Referenced records cannot
be deleted while a follow-up still names them. Source metadata cannot be changed
through the API after creation. The VCS object format and history are unchanged.
Back up before upgrading; older binaries refuse schema 7 and rollback restores
the pre-upgrade backup rather than opening upgraded storage.

The additive protocol fields are advertised by `tasks.lineage`. The browser
sends them only on supporting servers; older servers retain descriptive links.
Task pages and native clients expose the stored source IDs. The orchestrator
retains them in the executor task JSON file. MCP task summaries
include them so an executor can inspect the original work without inferring a
relationship from untrusted prose.
