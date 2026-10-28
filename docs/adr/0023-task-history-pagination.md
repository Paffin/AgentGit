# ADR 0023: Task-scoped history with continuation tokens

Status: implemented after the beta.4 release candidate.

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
