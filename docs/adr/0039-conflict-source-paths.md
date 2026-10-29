# ADR 0039: exact source paths in renamed conflicts

Status: accepted.

## Context

The merge engine already records where each version lives when a rename moves a conflicting
file. Conflict responses discarded those locations. A browser fetching both versions at the
output path could therefore mistake a renamed version for a deletion and resolve with the
wrong content.

## Decision

Preserve the engine's optional `origin` object through core conflict records and HTTP/gRPC:
`base`, `ours` and `theirs` are exact, nonempty repository paths. `Conflict.path` remains the
output location for the resolution. Ordinary same-path conflicts have no origin; older stored
outcomes also remain readable without one. No merge algorithm or metadata schema changes.

The labels refer to merge inputs. For virtual workspace sync, ours is the workspace head and
theirs is the returned stream head. For proposal preparation and integration, ours is the
stream and theirs is the proposal. CLI, SDK and MCP expose those source paths without parsing
the human-readable conflict description.

The browser requires `workspaces.conflict_origins` before opening the resolver, reads each
side from its exact source path and previews differing locations. It continues to resolve at
the output path with existing branch/proposal head preconditions. Rename/delete is a
single-output-path choice and can use the same flow; rename/rename and file/directory cases
remain unavailable there until a complete multi-path choice is designed.

## Validation

Core serialization tests preserve all three paths and read old records. Real HTTP/gRPC tests
exercise both rename orientations and the inverted inputs of proposal preparation. Browser
coverage selects the side whose file does not exist at the output path and checks the final
blob; it also covers rename/delete on either side.
