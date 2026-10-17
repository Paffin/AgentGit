# Reading history: log, show, blame

Everything here is **derived** from the changeset DAG on demand. Nothing is stored, so it can never disagree
with the history it describes.

## `ag log`, `ag show`

```sh
ag log -n 20                    # newest first
ag log --path src/auth          # only changesets that touched src/auth or anything beneath it
ag show <rev>                   # message, parents, and what changed
```

`ag show` lists each change as `A` (added), `M` (modified), `D` (deleted) or **`R old -> new (NN% similar)`**
for a rename: a path the changeset deleted paired with a similar path it created (the same
[rename detection](../architecture/merge-semantics.md#renames) the merge uses; renames are display, not
history — the stored changeset is still a delete and a put).

## `ag blame`

```sh
ag blame src/lib.rs                  # the workspace head (or the default stream)
ag blame main:src/lib.rs             # a specific revision: [REV:]PATH
ag blame -L 40,60 src/lib.rs         # only lines 40 to 60
ag blame --json src/lib.rs           # machine-readable
```

```text
 02c169c6bdf3 user:alice                2026-10-17T16:35:45.911Z src/old.rs 1) use std::fmt;
 02c169c6bdf3 user:alice                2026-10-17T16:35:45.911Z src/old.rs 2) pub fn parse() {}
 4cc4173d2ef1 agent:refactor-bot#run_91 2026-10-17T16:35:45.965Z src/lib.rs 3) pub fn render() {}
```

Each line shows the changeset that introduced it, its author and time, the path the file had **in that
changeset** (shown when a rename is involved — here the first two lines predate the move from `src/old.rs`),
and the line's text. `--json` adds the original line number and
the changeset's summary.

How the answer is built:

- Lines pass from a changeset to its parents when the diff (the same deterministic Myers implementation the
  merge uses) shows them unchanged; a line is blamed on the first changeset where it appears.
- A changeset that did not touch the path is skipped without reading the file.
- **Merges** give each line to the parent that already had it. A merge is blamed only for lines it had to
  write itself — a conflict resolution — not for lines either side brought.
- **Renames are followed**: when a changeset creates the path and deletes a similar file, blame continues in
  the old file. For merge parents other than the first only the same path is considered.
- A line that moved within a file is a delete plus an insert in the diff, so it is blamed on the changeset
  that moved it. There is no copy/move detection across files.

Limits, always reported: files over 4 MiB, binary files and files with more than 100 000 lines are refused;
after `--max-commits` changesets (default 100 000) the walk stops and the remaining lines are marked `^`
(`"boundary": true`, `"truncated": true`) — they were found no later than the changeset shown, and may be
older.

Blame needs only the changeset graph and the blobs of the revisions that touched the file, so it works on
unpublished snapshots in a workspace as well.

## Against a server

```sh
ag remote blame main:src/lib.rs      # over gRPC
curl -H "Authorization: Bearer $TOKEN" \
     "$SERVER/v1/repos/acme/web/blame?path=src/lib.rs&rev=main&first_line=40&last_line=60"
```

The API is `history.blame` ([AGWP reference](../protocol/agwp-v1.md)); it needs the `reader` role. An empty
`rev` means the repository's default stream.
