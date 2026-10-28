# ADR 0016: durable repository issues

Status: implemented; qualification in progress.

## Decision

User issues are separate from orchestrator tasks. An issue has a repository-local number, title, description, author, open/closed state, revision and optional link to an existing task in the same repository. Closing an issue does not finish its linked task. Immutable activity records retain comments and lifecycle events.

HTTP and gRPC expose the same service, advertised as `issues.v1`. Repository readers can read; writers can create and comment; only the author or repository administrators can edit or change state. Edits require the expected revision and reject stale writes. Creation and comments support idempotency keys. Mutations, activity, audit and event outbox records commit together. Continuations are bounded and scoped to their repository and filter, or issue.

The browser provides a searchable list and permanent issue pages, discussion, state changes and task links. Drafts are kept in tab storage under account/repository/issue keys, without credentials. A conflicting update retains the local draft and requires explicit acknowledgement of the newer version before replacement. Live issue events refresh views without replacing an active edit draft.

## Storage and upgrades

Metadata schema version 4 adds `issues` and `issue_activity` to each repository database on SQLite and PostgreSQL. Back up before upgrading. Older binaries reject the newer metadata schema; downgrade requires restoring the pre-upgrade backup. SQLite database backups and PostgreSQL database dumps include these tables. This changes no object hashes or existing task semantics.

## Remaining scope

This is a first tracker slice, not GitLab parity. Labels, milestones, attachments, templates, issue dependencies, boards and GitHub/GitLab issue import remain outstanding. Descriptions and comments render as plain text. No claim of complete provider migration is made.
