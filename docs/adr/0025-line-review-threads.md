# ADR 0025: Revision-bound line review threads

Status: implemented in development after beta.4.

Plain discussion comments and quoted code do not represent reply groups or a
resolution state. Add separate repository-local review threads, without using
approval records for discussion. Existing comments and review votes remain intact.

Each thread stores the proposal, immutable base/head IDs, path, old/new side,
line number, server-read line text, initial body and author. Creating a thread
requires the current proposal head and a valid text line in the selected revision.
The server reads the line itself; client-supplied text is never authoritative.
The old side requires a base revision. Unsupported/binary or oversized source
files fail explicitly. A thread's revisions remain GC roots, including after
resolution, proposal closure or a proposal-head replacement. Integrity checks report an unreadable source as `dangling_thread_revision`.

Replies are immutable and independently paged. They increment the thread version.
Resolution and reopening require the expected thread version, so a concurrent
reply cannot be silently marked resolved. Thread authors, proposal authors,
reviewers and integrators can change resolution; other writers can discuss.
Resolved threads must be reopened before adding a reply. All writes use the
existing repository authorization, audit/outbox and transaction-scoped idempotency.
Discussion state does not itself approve or block integration in this slice.

Metadata schema 6 adds `review_threads` and `review_thread_comments` to SQLite and
PostgreSQL. Existing rows are unchanged. Upgrades are transactional and require a
pre-upgrade backup. Older binaries refuse schema 6; rollback restores the backup
with the old binary instead of opening upgraded storage. The deterministic VCS
object format and merge semantics do not change.

Expose paged HTTP and gRPC operations, generated SDKs and the `proposals.threads`
capability. The browser preserves its legacy comment composer on older servers.
Show anchor revision explicitly and label threads on previous proposal heads as
outdated, never silently remap line numbers. Suggestions, reviewer assignment and
policy requirements for resolution remain separate work.
