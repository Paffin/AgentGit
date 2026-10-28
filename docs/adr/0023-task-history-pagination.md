# ADR 0023: Task-scoped history with continuation tokens

Status: implemented in beta.4.

A task page previously filtered only the newest 100 repository runs and change
requests in the browser. Activity elsewhere could hide every result of an older
task. The page warned about the limit but did not let the reader recover history.

`ListRunsRequest` and `ListProposalsRequest` now accept an optional `task_id`.
Filtering happens in the metadata query before the limit. Run listings also accept
`page_token` and return `next_page_token`; their ordering is descending
`(started_at, run_id)`. Proposal listings retain their existing keyset ordering.
The changes are additive, shared by HTTP and gRPC, and advertised as `tasks.history`.
Read permissions and repository isolation are unchanged. No metadata migration is
needed. A continuation is not a frozen snapshot: later state changes may alter a
filtered listing, and newer entries appear after refresh.

The task page loads 50 matching records at a time. Readers can load older records,
and live refresh preserves the number of pages already expanded. Changing account,
repository or task cancels pending work and clears old results before loading.
Older servers retain a clearly labelled repository-history fallback. TypeScript
and Python SDKs expose the fields; Python's typed pagination includes runs.

Qualification uses 110 stored records with tied timestamps to verify filtering
before limits and stable boundaries. The HTTP scenario checks multiple pages,
combined filters, malformed tokens and an unrelated user's inability to read the
repository. Chromium desktop and phone viewport each load all 51 linked runs and
51 linked proposals with newer unrelated activity. This browser fixture qualifies
history navigation, not autonomous agent execution or a physical phone.

The native orchestrator uses the same task-scoped pages when recovering retry
budgets after restart. Its former agent-wide 200-run window could lose a failed
attempt behind unrelated work. It now rejects repeated cursors, repeated run IDs,
unrelated records and incomplete retrieval rather than accepting partial history.
Retrieval is bounded at 10,000 task runs; reaching a bound with more history defers
recovery. On a server without `tasks.history`, fewer than 200 agent runs can still
be inspected safely; a full legacy page is treated as potentially truncated.
A real command/server regression reproduces the old failure with 205 unrelated
runs and verifies that a spent task is cancelled without another process launch.
