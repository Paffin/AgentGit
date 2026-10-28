# ADR 0015: bounded, revision-bound content search

Status: accepted; implemented with local HTTP, gRPC and real-browser qualification. Hosted final-candidate checks remain required before merge.

## Context

The file browser can filter loaded names but cannot find text in repository contents. A search result must identify the exact code it searched, must obey repository read permissions, and must not trigger an unbounded checkout or process execution. An index is not yet required to deliver useful search for modest repositories.

## Decision

Add the transport-independent `tree.search` operation to `TreeService` and HTTP `GET /v1/repos/{owner}/{rname}/search`, advertised as feature `tree.search`. It scans the immutable tree resolved from `rev`, using the existing `TreeRead` authorization and object-store reads. It does not launch commands, follow symbolic links, or alter metadata, objects, history or merge semantics. Link targets are searched as text.

The query is a literal of 1–256 UTF-8 bytes, without line breaks. `case_sensitive=false` compares Unicode lowercase strings; it is not regex matching, locale collation or full Unicode case folding. An optional file/directory prefix narrows the tree. The result contains one row per matching line and a bounded Unicode-safe excerpt around its first match. Files with NUL bytes, invalid UTF-8, or more than 1 MiB of content are excluded and counted.

Each request scans at most 1000 eligible file entries and 16 MiB of content, and returns at most 100 matching lines (default 50). Continuation may therefore be necessary even when a page has no matches. Counts describe the work on that page; a file resumed halfway through its lines can be counted again. A final continuation can have no matches.

Continuation tokens encode the last path/line plus a SHA-256 binding to the repository, tree, query, case mode and prefix. They are consistency cursors, not credentials or signatures. Every request is independently authorized. A changed input or tree rejects the token. Responses include the resolved changeset, and clients continue using that immutable revision instead of resolving a moving branch again.

The Web UI exposes the limits and exclusions, preserves the query in its URL, keeps previous results after a failed continuation, and opens matches at their pinned revision and line. Older servers do not advertise `tree.search`; the UI explains that an update is needed rather than treating absence as an empty result.

## Compatibility and alternatives

This is an additive protocol change. Existing fields, operations, stored formats and hashes are unchanged; no migration is required. HTTP and gRPC share one implementation. Generated OpenAPI and TypeScript types must be regenerated together.

Client-side fetching of every file was rejected because it transfers excessive content and cannot enforce server work budgets. Shelling out to grep was rejected because repositories need not exist as filesystem checkouts. A persistent inverted index is deferred until measurements justify its storage, invalidation and recovery costs. This implementation does not claim indexed-search latency or complete binary/large-file search.

## Verification

Focused tests cover Unicode excerpts, stable pagination, exclusions, no-match budget continuations, malformed/mismatched cursors and repository authorization. Browser qualification must use a real server and UI-created repository/content; mocks cannot establish the complete workflow. Performance claims require measured repository sizes and are not made by this ADR.
