# Contributing to AgentGit

Thanks for helping! AgentGit's whole point is *correctness you can rely on*, so contributions are
reviewed for that first, and for features second. This guide gets you productive quickly.

## Ground rules

- Be kind: see the [Code of Conduct](CODE_OF_CONDUCT.md).
- By contributing you agree that your work is licensed under the [Apache License 2.0](LICENSE)
  (Section 5 of the license). There is no CLA.
- Security issues go through [SECURITY.md](SECURITY.md), not public issues.
- For anything bigger than a bug fix, **open an issue first** so we can agree on the design.
  Changes to on-disk formats, hashing, merge semantics or the publication protocol need an ADR in
  [`docs/adr/`](docs/adr/).

## Set up

You need Rust **1.89+** (the MSRV), a C compiler (SQLite is built from source), and optionally
`git` (for the Git bridge and its tests) and `python3` (for the demo and doc generators).

```bash
git clone https://github.com/Paffin/AgentGit && cd AgentGit
cargo build --workspace
cargo test --workspace          # ~200 tests, a few seconds
./examples/quickstart.sh        # the end-to-end demo shown in the README
```

## Before you open a pull request

CI runs exactly these; run them locally:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo build -p ag-cli && python3 scripts/gen_cli_docs.py --check   # CLI reference is generated
```

If you change a command or flag, regenerate the reference with `python3 scripts/gen_cli_docs.py`
(build the binary you want documented first; the script prefers `target/release/ag`, or set `AG=`).

## What a good change looks like

1. **Tests with the change.** Bug fix → a test that fails without it. New guarantee → a test that
   tries to break it. We have several styles; pick the one that fits:
   - unit and property tests (`proptest`) next to the code;
   - `crates/ag-changeset/tests/conformance.rs` + `tests/conformance/gen_vectors.py`: byte-level
     vectors from an *independent* Python implementation of the formats;
   - `crates/ag-integrate/tests/`: end-to-end engine tests, including **fault injection**
     (`Failpoints`) at every step of publication and integration, and concurrency tests;
   - `crates/ag-cli/tests/`: the real binary, including `examples/quickstart.sh`.
2. **Docs with the change.** User-visible behaviour belongs in `docs/` and, if notable, in
   [`CHANGELOG.md`](CHANGELOG.md). Doc claims should be checkable: link to the test that proves a
   guarantee, don't just assert it.
3. **No new `unsafe`.** The workspace forbids it.
4. **Determinism.** Anything that feeds a hash or a merge must be deterministic and versioned.
   Iterating a `HashMap` into a hash is a bug.
5. **Small, reviewable commits** with imperative subject lines (`fix(merge): ...`, `feat(cli): ...`).

## Project layout

See [`docs/architecture/overview.md`](docs/architecture/overview.md). In short: `ag-types`,
`ag-object`, `ag-changeset`, `ag-tree`, `ag-dag`, `ag-diff`, `ag-merge` form the deterministic core;
`ag-storage` holds the object store and metadata; `ag-policy` and `ag-agent` add policy and
agent coordination; `ag-integrate` is the engine; `ag-git` the bridge; `ag-cli` the `ag` binary.

## Reporting bugs

Use the bug template. The most useful report includes the output of `ag --version`, the exact
commands, and `ag verify` / `ag --json op show <operation id>` output if an integration was involved.
