# ADR 0045: resumable hosting issue import

Status: accepted.

## Decision

Import bounded, provider-neutral issue/comment manifests into an existing repository.
GitHub/GitLab exporters run in the CLI, so provider tokens never enter the server.
Preview validates the full manifest and reports counts, unsupported metadata and
conflicts. Apply requires its canonical digest and explicit acknowledgement of any
unsupported metadata. Nothing is truncated to fit a limit.

Schema13 adds a durable mapping from provider/source repository/issue ID to local
issue number, source digest and canonical source JSON. Each issue, comments, mapping
and audit/outbox record commit in one transaction. Retrying an interrupted import
skips identical completed issues; changed source content is rejected. This initial
import does not synchronize later source changes or overwrite local edits.

The authenticated importer remains the local actor. Original authors, URLs and times
are preserved as visible provenance and in the source snapshot; original issue state
and timestamps are retained. Imported identities grant no local access. Manifests
are limited to4MiB,100 issues and1000 comments and existing text limits still apply.
Back up before upgrading and restore that backup to downgrade.
