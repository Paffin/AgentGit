Leases: claiming a part of the tree

A lease claims a path scope (a glob such as `src/auth/**`) for an identity, for a limited time.

    ag lease acquire 'src/auth/**' --ttl 30m                soft: a signal, never blocks anyone
    ag lease acquire 'src/auth/**' --ttl 30m --exclusive    enforced at integration; prints a fencing token
    ag lease extend LEASE --ttl 30m                         the token stays the same
    ag lease release LEASE
    ag lease list [--all]

Soft leases are advisory. Acquiring one over someone else's reports the overlap as a warning.

Exclusive leases are refused while another principal holds an overlapping exclusive lease, and they are
enforced when a change lands: while the lease is active, a change that touches its scope can only be
integrated if its author is the holder AND the integration presents the lease's current fencing token:

    ag integrate PROPOSAL --fencing-token 7

Fencing tokens come from one strictly increasing counter. If a holder stalls, its lease expires and someone
else acquires the scope with a higher token; when the stalled holder wakes up, its old token no longer passes.
Expiry never deletes changes that were already proposed: it only stops a stale holder from landing them into
a scope that is now someone else's.

`ag integrate` reports a lease problem as a blocker (exit 3): "exclusive lease on 'src/**' held by ...
(held_by_other / stale_or_missing_token)". Look with `ag lease list`, wait or ask the holder, then pass the
current token.

Finishing an agent run (`ag agent finish RUN`) releases that run's leases; `ag agent reap` abandons runs with
no recent heartbeat and releases theirs.

`ag predict` shows overlapping work before it collides; `ag preview PROPOSAL` gives the definitive merge answer.

More: docs/guides/agents.md
