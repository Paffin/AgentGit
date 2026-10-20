# Wave 4: directory renames and copy detection ("Merge and history")

State: `ag-merge/3` implemented and tested; copy detection for `ag blame` / `ag show` implemented. Decision record:
`docs/adr/0013-directory-renames.md`. Semantics: `docs/architecture/merge-semantics.md` (Directory renames, Algorithm
versions, Copies, Cost). User docs: `docs/guides/history.md`.

## What changed
* `crates/ag-merge`: `dirrename.rs` (analysis), `tree.rs` (`Algorithm`-aware `plan_renames`, relocations, notes, two new
  conflict kinds), `rename.rs` (`detect_copies`; **linear exact pairing**, identical result to the old quadratic one),
  `lib.rs` (`Algorithm::{V2,V3}`, `parse`, `ALGORITHM_VERSION = ag-merge/3+ag-diff/1`, `ALGORITHM_VERSION_V2`).
  `merge_trees` = current algorithm; `merge_trees_with(algo, ..)` reproduces `ag-merge/2`.
* `crates/ag-integrate`: merge messages carry `Directory-Rename:` lines (integrate and workspace sync);
  `MergePreview.notes: Vec<NoteInfo>`; `BlameOptions.copies`; `Repository::changeset_copies`.
* `crates/ag-cli`: `ag preview` prints `note:` lines; `ag blame --copies`; `ag show --copies` (`C old -> new`).
  (`-C` is taken by the global `-C <dir>`, so the flag is long-only.) CLI reference regenerated.
* `crates/ag-server`: only `BlameOptions { copies: false }` (server and MCP blame do not follow copies yet).
* `.cargo/mutants.toml`: the line-anchored `rename.rs` exclusion now matches any line (`:\d+:27`).

## Compatibility
* Old merges stay verifiable: `tests/compat.rs::every_merge_the_old_build_made_is_reproduced_by_this_one` now runs the
  version each recorded merge names (`Algorithm::parse`); the fixtures record `ag-merge/2+ag-diff/1`.
* No id, hash, stored format, proto field or conformance vector changed. The wire `Conflict.kind` is an open string:
  new codes `directory_rename_ambiguous`, `directory_rename_collision` (listed in `docs/protocol/agwp-v1.md`; the
  comment on the field in `proto/agwp/v1/agwp.proto` / the generated OpenAPI and SDK schemas still lists only the
  old nine and was **not** regenerated: follow-up, needs the OpenAPI golden and SDK generators).

## Evidence
* `cargo test -p agentgit-merge` (78 unit tests incl. properties: fast path == reference under V3 on trees full of
  directory moves, V3 == V2 whenever no note/directory conflict, "following a move == moving the other side by hand
  and using V2", linear exact pairing == quadratic reference), `-p agentgit-integrate` (engine, blame, compat,
  lifecycle, ...), `-p agentgit-cli` (cli, completions, json_shapes): all green. `cargo clippy -D warnings` on merge,
  integrate, cli, bench, server (all targets), `cargo fmt --check`, `scripts/check_docs.py`,
  `scripts/gen_cli_docs.py --check`, `scripts/check_no_stubs.py`.
* Cost (release, 100 000 files in one renamed directory, other side edits 1 and adds 20): V2 1.13 s, V3 1.30 s;
  all-identical contents 0.84 s / 1.15 s; 10 000 files 55 / 66 ms. Reproduce:
  `cargo test --release -p agentgit-merge --test scale directory_rename_costs -- --ignored --nocapture`.

## Not verified / limits
* Not run: the full workspace suite, PostgreSQL/S3 legs, mutation testing of the new code (the weekly job will); the
  Web UI does not show merge notes; `ag remote` / MCP blame do not take `copies`.
* The copy-source search reads the whole first-parent tree once per created file during `ag blame --copies`
  (a point lookup per changeset is used otherwise); not measured at 100 000 files.
* A relocated file whose target is occupied is a conflict even when the contents are identical (conservative).
* Directory moves into their own subtree, and chains within one side's history, are not followed (documented).

## Changelog entries (for the orchestrator)
* **Directory-rename-aware merge** (`ag-merge/3`): when one side renames a directory (most of its files moved with the
  same relative paths) and the other adds or edits files under the old directory, the changes follow the rename;
  ambiguous or colliding cases are conflicts (`directory_rename_ambiguous`, `directory_rename_collision`), never
  guesses. The merge reports a typed note (`ag preview`, merge changeset message `Directory-Rename:`). Merges
  recorded as `ag-merge/2` are still reproduced by `Algorithm::V2` / `merge_trees_with`. ADR-0013.
* The pairing of identical-content renames is linear (was quadratic in the number of identical files).
* `ag blame --copies` and `ag show --copies`: follow a file created as a copy of another (identical, or at least 50 %
  similar; at most 200 candidates); display only, no merge uses it.
* `ag_merge::ALGORITHM_VERSION` is `ag-merge/3+ag-diff/1`.

## README / ROADMAP notes
* ROADMAP "Merge and history": remove "directory renames and copy detection"; keep as open: copies in merges (not
  planned), `copies` over the server API and MCP, merge notes in the Web UI, regenerating the OpenAPI/SDK comment for
  the two new conflict codes.
* README: the `strategy: Merge (algorithm ag-merge/2+ag-diff/1)` sample line becomes `ag-merge/3+ag-diff/1` (also
  in README.ru.md; the README demo is generated: `scripts/gen_readme_assets.py --check` was not re-run here).
