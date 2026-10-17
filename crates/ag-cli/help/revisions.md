Revisions: naming a point in history

Wherever a command takes a revision you can use:

    @                  the head of the default workspace
    @NAME              the head of workspace NAME
    main               a stream name (its current head)
    prop_8f3a1c        a proposal id (its proposed head); a unique prefix of 6+ characters is enough
    e55836a9d643       a changeset id, or a unique prefix of 6+ hex characters

optionally followed by `~N` (the N-th first-parent ancestor) and `^N` (the N-th parent):

    main~2             two changesets before the head of main
    prop_8f3a1c^2      the second parent of the proposed head (for a merge)

`REV:PATH` names a file at a revision:

    ag cat main:src/lib.rs
    ag cat @:README.md             the workspace head
    ag blame main:src/lib.rs

Without a revision, most commands use the head of the default workspace, or the default stream when the
workspace has no snapshots yet.

If a name cannot be resolved, `ag stream list`, `ag proposal list` and `ag log` show what exists.

More: docs/concepts.md
