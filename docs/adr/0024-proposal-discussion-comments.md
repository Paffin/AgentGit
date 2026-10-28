# ADR 0024: Proposal discussion without approval privileges

Status: implemented in development after beta.4.

Authors could submit code but could not reply in its review discussion without
receiving the reviewer role. Discussion must not grant approval rights or alter
an existing decision.

`proposal.comment` is a separate capability available to writers, reviewers and
integrators. The HTTP `/proposals/{id}/comments` endpoint and gRPC `Comment` method
accept non-empty text up to 16 KiB and require the exact inspected head. The head
check, comment insertion, audit and optional idempotency record share one metadata
transaction. A changed head rejects the write; retrying the same accepted key
returns the same entry. A changed payload cannot reuse the key.

Discussion entries use the existing review records with the fixed `comment`
decision. Callers cannot choose an approval decision through this operation.
Comments remain available after closure and never affect approval counts. No
metadata migration is necessary. Single-review lookup avoids scanning the whole
history when storing another entry. Existing `proposal.review` keeps its distinct
reviewer permission and behaviour.

The browser exposes a discussion action and composer. Proposal tabs have durable URLs and survive reload. Drafts are saved per
account/repository/proposal in tab storage with the inspected head and retry key.
A changed head requires explicit acknowledgement after inspection. Account changes
clear active UI state and cancel pending requests. A malformed saved draft is left
untouched. Discussion never supersedes an earlier approval or request for changes
in the browser summary, matching server policy evaluation.

This is the foundation for author/reviewer discussion, not line-anchored threads,
thread resolution or code suggestions. Those remain separate work.
