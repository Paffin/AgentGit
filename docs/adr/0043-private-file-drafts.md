# ADR 0043: private single-file drafts

Status: accepted.

## Decision

Save one existing-file edit per account, stream and path in private metadata. The
original revision, blob, file type and executable bit stay fixed until the draft is
cleared. Replacement bytes do not enter shared object history until the user opens
a change request. The draft retains its original revision for garbage collection.

Schema 12 adds `file_drafts`. Saves and clears use the existing versioned draft
contract, including identical retries and deletion tombstones. Each draft is limited
to 4 MiB; an actor can retain twenty active drafts per repository. List returns only
that actor's metadata for one authorized stream. All operations require workspace
write permission, use the full actor identity and return private, non-cacheable
responses. They publish no repository events or audit content.

Web offers explicit account save and restore in the existing single-file editor,
retaining BOM, line endings, type and mode. Creating a change request remains a
separate action; successful publication clears only the saved version it used.
CLI, SDK and MCP expose the same operations. Batch and new-file drafts are deferred.
Back up before upgrading; rollback requires restoring the pre-upgrade backup.
