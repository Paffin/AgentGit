# ADR 0044: one event feed for multiple repositories

Status: accepted.

## Decision

Reuse each repository's durable audit log through one bounded event feed. Callers
name up to thirty repositories explicitly. Every repository is authorized before
returning data and while following; the feed never broadens a credential's scope.
No new event store or metadata migration is needed.

Each event carries an opaque continuation cursor with the position of every
repository, bound to the normalized repository and topic set. Sequences from
separate repositories are not globally ordered. A bounded round-robin scan keeps
one busy repository from excluding the others. Replay pages examine at most4096
rows and emit at most1000 events.

An empty cursor starts at the current log positions; deliberate historical replay
uses `from_start`. Checkpoints and heartbeats advance the cursor without inventing
an activity event. HTTP SSE uses that cursor as its event ID, and reconnect resumes
from it. Existing single-repository APIs remain compatible.

Home keeps its full initial data snapshot, refreshes on connection/reconnection,
and subscribes to the loaded repository set. Up to thirty repositories share the
live connection; additional loaded repositories retain bounded polling. Hidden
pages stop the connection, and a new session starts with fresh scope and state.
CLI, SDK and MCP replay use the same cursor contract.
