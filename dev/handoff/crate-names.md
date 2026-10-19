# Handoff: crates.io naming and packaging

Decision delegated by the owner: how the crates are named on crates.io. Nothing was published, no tag was created.

## Scheme

Every library and binary crate is published as `agentgit-<x>`; the directory stays `crates/ag-<x>`, the library name stays
`ag_<x>` (`[lib] name`), binaries keep their names (`ag`, `ag-server`, `ag-worker`, `ag-mcp`). Dependents keep
`ag-x.workspace = true`; `[workspace.dependencies]` (and `fuzz/Cargo.toml`) declare
`ag-x = { package = "agentgit-x", path = ..., version = ... }`, so **no Rust identifier changed**. `cargo -p` takes the
package name: every `-p ag-…` in workflows, scripts, docs, Dockerfile and READMEs became `-p agentgit-…`.

Availability, checked 2026-10-19 with a User-Agent header (`/tmp` script, results are HTTP statuses):

* `https://crates.io/api/v1/crates/<name>` and `https://index.crates.io/<aa>/<bb>/<name>`: **404 for all 22**
  `agentgit-{types,crypto,object,changeset,diff,tree,dag,merge,policy,agent,storage,integrate,git,protocol,client,server,
  cli,mcp,sandbox,worker,ui-assets,bench}`.
* Positive control: `ag-agent`, `ag-git`, `ag-protocol`, `ag-worker` answer **200** (taken); `ag-types`, `ag-mcp` and
  `agentgit` answer 404 (free, but unused: one consistent prefix beats a mixed scheme).

## What changed

* All 22 `[package] name`s; `[lib] name` on every crate with a library; `documentation`, per-crate complete metadata
  (`description`, `license`, `repository`, `homepage`, `documentation`, `readme`, `keywords` ≤ 5, `categories`,
  `rust-version`).
* `publish = false` removed from `ag-agent`, `ag-cli`, `ag-client`, `ag-git`, `ag-integrate`, `ag-protocol`,
  `ag-server`, `ag-storage`, `ag-ui-assets`, `ag-worker` (ten crates; the brief said nine). **21 publishable**;
  `ag-bench` stays `publish = false` (a measurement tool of this repository, reason in its `Cargo.toml`).
* Out-of-crate reads fixed: `crates/ag-protocol/proto/`, `crates/ag-storage/migrations/`,
  `crates/ag-server/migrations/control/` are copies of `proto/` and `migrations/` (those stay the source of truth);
  `scripts/sync_crate_files.py` writes them and `--check`s them (run in `Packaging`), and each of the three crates has a
  test (`proto_copy`, `migrations_copy`) that fails in a checkout when a copy differs. `agentgit-ui-assets` ships the
  Web UI from a git-ignored `dist/` (`include = [...]`, `scripts/sync_crate_files.py --ui` after `scripts/build_ui.sh`);
  `build.rs` prefers `apps/web/dist`, then `dist/`.
* `build.rs` (cli, server, worker, identical) used `CARGO_PKG_NAME` as the directory under `crates/` to decide whether
  the surrounding git checkout is its own; it now uses the manifest directory's name (otherwise `--version` would have
  lost its commit hash).
* `ag-mcp` dev-dependencies on `ag-server`/`ag-storage` are `.workspace = true` again (they are publishable now).
* `scripts/check_publish_metadata.py`: enforces the naming scheme, `[lib]` names, `documentation`, no `include_*!` of a
  file outside the crate; publish order now counts dev-dependencies (`--list`).
* `Packaging` (crates job): builds the Web UI, syncs it into the crate, checks the copies, runs
  `cargo publish --workspace --dry-run --locked` (cargo orders the crates and resolves each against the tarballs of the
  earlier ones), asserts that the packaged/verified/uploaded set equals `publish order`, and that the packages contain
  `dist/index.html`, the proto and the migrations. Path filters gained `proto/**`, `migrations/**`, `apps/web/**`.
* The quality workflow's mutation matrix uses package names. `scripts/sbom.sh` expects `agentgit-types` in the SBOM.
  `THIRD_PARTY_LICENSES.md`: the 22 workspace entries renamed by hand (cargo-about is not installed here; the
  `Security` workflow's `hygiene` job regenerates and compares, see CI evidence).
* Docs: `docs/operations/releasing.md` (names, table, files outside a crate, publish order, publishing steps, limits),
  `docs/getting-started.md`, `CONTRIBUTING.md`, `dev/STATUS.md`, `dev/agent-brief.md` (`-p` flags).

## Verified locally

`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`;
tests of `agentgit-cli`, `-protocol`, `-storage`, `-types`, `-mcp` (all passed), the three copy tests;
`cargo publish --workspace --dry-run --locked --allow-dirty`: 21 crates packaged, built from tarballs and "uploaded"
(dry run); largest package `agentgit-ui-assets`, 242 files, 880 KiB (560 KiB compressed).
`check_docs.py`, `gen_cli_docs.py --check`, `gen_readme_assets.py --check`, `check_workflows.py --strict`,
`check_no_stubs.py`, `test_release_scripts.py`, `test_coverage_summary.py`, `sync_crate_files.py --check`,
`check_publish_metadata.py --list`.

## Not verified

* The full workspace test suite was not re-run locally (the shared disk filled up mid-run; the link failures were
  `ENOSPC`, not code).
* `check_deploy.py --strict` fails here only because `kustomize` and `promtool` are not installed.
* Publishing itself, name ownership on upload, crates.io rate limits, `cargo install agentgit-cli` from the registry.
* Every crate page will show the workspace `README.md`; per-crate READMEs do not exist.

## README / ROADMAP / CHANGELOG notes (for the orchestrator)

* README: `cargo install --path crates/ag-cli` / `crates/ag-server` stay correct (a directory path). Any mention of
  `-p ag-…` must become `-p agentgit-…`; the honest-limits row about crates.io should say: names decided
  (`agentgit-*`), 21 crates dry-run packaged in CI, nothing published.
* ROADMAP "Next": drop "decide the crates.io naming scheme"; keep "publish".
* CHANGELOG (Unreleased): "Cargo packages are renamed `agentgit-<x>` (directories, library names and binaries
  unchanged; `cargo -p` flags change); 21 crates are publishable and CI dry-runs `cargo publish --workspace`; the
  protocol and migrations are packaged inside their crates (`scripts/sync_crate_files.py`)."
