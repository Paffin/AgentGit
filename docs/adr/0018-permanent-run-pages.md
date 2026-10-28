# ADR 0018: permanent pages for agent runs

Status: implemented; browser qualification in progress.

## Decision

Expose `agent.get_run` over HTTP and gRPC, advertised as `agents.run_lookup`. It reads
one run by repository and run ID, using the same repository read permission as the
run list. It finds finished and historical runs directly; it does not depend on the
latest 100/1000 items being loaded. An unrelated principal receives no repository data.
No new storage schema or object format is required.

The browser links to permanent run pages from the run table, live board and task detail.
A page shows the recorded session status and summary, task, current file reservations,
heartbeat and timing. The change-request link filters by the full run identity, not
just the agent name; two sessions of the same agent remain distinct. Completed is a
session status, not a substitute for validation, review or integration.

Running sessions refresh their heartbeat while the page is visible. Event updates
refresh the other data, and old-account requests are cancelled/discarded when navigation
or credentials change. Historical pages stop periodic polling. Older servers retain
plain run labels and receive no unsupported lookup requests.

## Limits

This is run inspection, not a remote process debugger or a stdout log store. The summary
is supplied by the agent and rendered as text. Current reservations do not purport to
be lease history. Cancelling a task asks connected orchestrators to stop their managed
sessions; manually started external processes may require separate intervention.
