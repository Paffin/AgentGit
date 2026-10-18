# Handoff: docs_site (the documentation site, the tutorials, the no-stubs check)

Work package `docs_site` of wave 2 (audit area "docs site, CLI/docs track tail"). It finishes the second half of the
CLI/docs track (`dev/handoff/cli-docs.md`, `dev/tracks/tracks.md` "cli-docs"). Everything below is on `main`; the claims
say what was run and where. Written 2026-10-18.

## What is on `main`

| Commit | What |
|---|---|
| `0b53f10` | `scripts/gen_cli_docs.py` also writes and checks `docs/reference/{help-topics.md, cli-json.md, completions/*, man/*}`; hand-written `reference/configuration.md` and `reference/completions.md`; `docs/adr/README.md`; CLI: `--stream` next to the positional `STREAM` (policy show/history, queue list/run, predict), `integrate --idempotency-key` (alias of `--key`), `--quiet` prints the bare key / identity / fencing token for `validate keygen` / `agent start` / `lease acquire`; exit code 1 for not-found / already-exists documented in `ag help exit-codes` (not changed); CI job `shells` |
| `a2f8c86` | ADR-0009 in the ADR index |
| `931a9bc` | five tutorials, FAQ, glossary, comparison; `scripts/doc_snippets.py` (the harness) + its tests; `crates/ag-policy/tests/cookbook.rs`; `crates/ag-cli/tests/docs_snippets.rs`; `scripts/check_docs.py` extended; `ag push --quiet` prints the bare proposal id |
| `1f448c8` | `mkdocs.yml`, `docs/requirements.{in,txt}` (hash-locked), `scripts/mkdocs_hooks.py` + tests, `.github/workflows/docs.yml` |
| `0950267` | `scripts/check_no_stubs.py` + allow-list + tests; `check_docs.py --require-ag` and the no-stubs check in the `docs-and-demo` job; `ag-sandbox` shim modules renamed from `*_stub.rs` to `*_unsupported.rs` |
| (this slice) | the README "See it work" transcript is checked against the demo's output (`quickstart.rs`); cross-links from the existing guides to the tutorials; this handoff |

## Evidence

* CI at `a2f8c86`: https://github.com/Paffin/AgentGit/actions/runs/39101831170 (all 10 jobs green, including the new
  `shell completions (bash, zsh, fish, PowerShell)` job with `AG_REQUIRE_SHELLS=1`: zsh and fish were installed there,
  PowerShell is on the image; the Windows, Containers and Packaging runs of that commit were green too).
* CI at `0950267`: CI https://github.com/Paffin/AgentGit/actions/runs/39103713003 (green: the `docs-and-demo` job ran
  `check_docs.py --require-ag` and `check_no_stubs.py`; `test (macos-latest)` ran the doc-snippet test under bash 3.2/BSD
  tools), Docs https://github.com/Paffin/AgentGit/actions/runs/39103713014 (green: strict site build, the tutorials against
  freshly built `ag` and `ag-server`, the cookbook parser test; the `deploy` job **succeeded with "Pages is not enabled,
  nothing is published"**, which is the guard working: `configure-pages` failed inside `continue-on-error`, every later step was
  skipped), Containers .../39103713026, Packaging .../39103713004, Windows .../39103713024: all green.
* CI at `94c3ddc` (README transcript check, cross-links, handoff): CI .../runs/39104231011, Docs .../39104231032, Containers
  .../39104231009, Packaging .../39104231045, Windows .../39104231027: all green.
* CI at `36db3e1` (`AG_REQUIRE_DOC_SNIPPETS=1` in the `test` job, so a skipped tutorial test is a failure; `check_docs` also checks
  that the `docs/*.md` pages named by `ag`'s help and messages exist): CI .../runs/39104741823, Docs .../39104741909, Packaging
  .../39104741932, Windows .../39104741865: all green (Containers is path-filtered and did not run).
* Local (this worktree, Linux): `cargo test -p ag-cli -p ag-policy` all green; `cargo clippy -p ag-cli -p ag-policy -p ag-sandbox
  --all-targets -D warnings`; `cargo fmt --all --check`; `python3 scripts/doc_snippets.py` (five pages, 275 statements, 48
  expectations) run several times, also against a Linux `ag-server`; `scripts/test_doc_snippets.py` (29 tests),
  `scripts/test_mkdocs_hooks.py` (10, with a real MkDocs build), `scripts/test_check_no_stubs.py` (18);
  `mkdocs build --strict` with exactly the versions in `docs/requirements.txt` installed with `--require-hashes`; a
  headless-Chromium screenshot of the built site (desktop width, light theme) looks right.
* Mutation checks (each failed as it should, then was restored): a typo in a cookbook recipe fails `cookbook.rs`; a broken
  link, anchor or file fails `mkdocs build --strict`; a reworded refusal message or an undocumented `AG_*` variable fails
  `check_docs.py`; a stale README transcript line fails `quickstart.rs`.

## What was built, and what it does and does not prove

* **The site.** MkDocs 1.6.1 + Material 9.7.7 (MkDocs 2.0 is a different, incompatible system; the pin is deliberate).
  `strict: true` with `validation: {anchors, unrecognized_links, omitted_files}: warn`, so a broken link, anchor or nav entry
  fails the build. `scripts/mkdocs_hooks.py` rewrites links that leave `docs/` to the repository at `DOCS_SOURCE_REF` (the
  published commit in the deploy job) and appends pages missing from `nav` under "More" **and reports them**: the other work
  packages that add pages (`guides/mcp.md`, testing/compatibility pages, ...) should add them to `nav` in `mkdocs.yml`, but
  nothing breaks if they forget. The `offline` plugin makes the site usable from `file://` (and forces `.html` URLs). No Google
  Fonts, no third-party requests at view time, except Material's mermaid loader for the two architecture pages (a CDN script;
  the diagrams stay readable code blocks offline).
* **Generated reference.** `gen_cli_docs.py --check` is the freshness check (already in `docs-and-demo`); the man pages are
  stored without the build stamp that `--version` prints (it changes every commit), and `tests/completions.rs` applies the
  same rule. `cli-json.md` is rendered from the golden shape files, so the page cannot disagree with the test that pins them.
  `reference/configuration.md` is hand-written; `check_docs.py` fails when `ag` reads an `AG_*` variable the page does not
  mention or the docs name one that is not in the code.
* **Tutorials** (`docs/tutorials/`): first agent-safe repository, team server (a real `ag-server` on a free port), policy
  cookbook, migrating a Git repository (a real Git repo; export compared tree by tree), troubleshooting a blocked
  integration (every refusal reproduced; a table of the messages checked against the Rust sources by `check_docs.py`).
  `scripts/doc_snippets.py` executes the blocks in one bash process per page in a scratch `HOME`, checks each exit status
  and that the `<!-- expect -->` text appears in the block's output, and refuses a page that holds an unmarked shell block.
  `cargo test -p ag-cli` runs the pages that need only `ag` (skipping `ag-server` pages: a stale binary in `target/` must not
  be trusted); the `Docs` workflow runs all of them against binaries built at that commit.
* **Cookbook test.** `crates/ag-policy/tests/cookbook.rs`: every ```json block must be marked; recipes go through
  `Policy::parse` (and round-trip), the listed mistakes must be refused with the stated reason.
* **No stubs.** `scripts/check_no_stubs.py` over production Rust (minus `#[cfg(test)]`/`#[test]` items and the files they pull in), Go
  (minus `_test.go`, `testutil`, `testdata`), the Web UI and the SDK sources. Found at the start: **no** `todo!`, TODO/FIXME/XXX/HACK,
  "for now" or "not implemented" in product code; 87 allow-listed uses, each with a reason in `scripts/no_stubs_allowlist.txt`
  (HTML `placeholder` attributes, SQL parameter placeholders, `{placeholders}` in route templates, the secret scanner's
  placeholder list, failure-injection points, `#[doc(hidden)]` test hooks, constant-time `dummy` comparison, doc comments that
  honestly say a module is tested with a stand-in). It is a pattern check: a function that returns a constant has no marker, so
  it keeps the *markers* out, it does not prove the absence of stubs.

## Audit triage

* **Confirmed, ops-docs-claims `scripts/check_docs.py:6` (no workflow ran it): fixed** (`docs-and-demo` runs it with
  `--require-ag`; CI also sets `CI`, so a missing binary fails). The `clients_ops` package can drop this item.
* **Contested, ops-docs-claims `examples/quickstart.sh:7` (the README transcript is not what the test checks): fixed on the test
  side**: `crates/ag-cli/tests/quickstart.rs` now checks every output line of the README's "See it work" transcript against the
  demo's output (ids and `…` are wildcards). The README itself was not edited (orchestrator's fold).
* Other audit items that mention `docs/` for other areas (`adr-traceability.md` quotas, `agwp-v1.md` traceparent, the stale
  "planned server mode" sentences in `overview.md`/`security.md`/`SECURITY.md`) belong to the server, security and clients packages
  and were not touched.
* `ag cat` / `ag remote cat` / `ag policy example` ignore `--json` and `ag remote events --follow --json` is not NDJSON
  (cli-client-git, contested/low): left to `policy_cli_execution`/`clients_ops`. `ag help json` documents the first as an
  exception on purpose.

## NOT verified / limits

* **GitHub Pages was not enabled**, so a real deploy has never happened; only the skip path is proven (above). After the owner
  enables Settings → Pages → Source: GitHub Actions, the next push to `main` runs `configure-pages`, `upload-pages-artifact@v3`
  and `deploy-pages@v4` for the first time; if an action version needs a bump it will show there. The site URL in `mkdocs.yml`
  (`https://paffin.github.io/AgentGit/`) is the default for this repository name.
* The tutorials were run on **Linux and macOS** (macOS: `ag`-only pages through `cargo test`; the `ag-server` page runs on Linux
  only). **Not on Windows**: the harness needs bash (`docs_snippets.rs` skips there).
* The completions are checked to *parse* in bash, zsh, fish and PowerShell (CI `shells` job) and bash completion is exercised; completing
  in a live zsh/fish/PowerShell session and parsing the elvish script are **not** tested.
* The site was looked at at desktop width only (headless Chromium, light theme); dark theme, phone layout and a screen reader were
  not checked. The mermaid diagrams need the CDN script.
* The comparison page's statements about Git, GitHub, Gerrit and Jujutsu are from their public documentation as understood when
  written, not from running them; the page says so and invites corrections. `jj` interoperability with an AgentGit import/export was
  not tried.
* `docs/requirements.txt` is not covered by Dependabot (no `pip` entry for `/docs`; `security_tail` owns `dependabot.yml`):
  regenerate it with the command in `docs/requirements.in`.
* `check_no_stubs.py` stale allow-list entries only warn in CI (a package that deletes allow-listed code must not break `main`);
  `python3 scripts/check_no_stubs.py --strict` lists them. The Go `internal/clock` package holds a `Fake` clock for tests: it is
  allow-listed with that reason and should move under `internal/testutil` (orchestrator package).

## Next steps (in order)

1. **Owner:** enable Pages (Settings → Pages → Source: GitHub Actions) and look at the first deploy run of the `Docs` workflow.
2. Packages that add pages: add them to `nav` in `mkdocs.yml` (and, for decision records, to `docs/adr/README.md`; `check_docs.py`
   enforces the latter). `mkdocs build --strict` shows pages that are only in "More".
3. `release` package: ship `ag completions`/`ag man` output (or `docs/reference/{completions,man}`) in the release archives.
4. `security_tail`: pin the actions in `docs.yml` like the others (`scripts/check_workflows.py --pin`) and decide about
   Dependabot for `docs/requirements.txt`.
5. `orchestrator`: move the Go `Fake` clock under `internal/testutil`, then delete its allow-list line.
6. Later: a `doctest-skip` audit (there are none today), a link checker for external URLs (not done: it needs the network),
   and a Russian version of the site if the README's Russian twin should have one (`README.ru.md` has no site counterpart).

## Changelog entries (for the orchestrator's fold)

* Documentation site: MkDocs Material, strict build (broken links and anchors fail it), pinned and hash-locked toolchain,
  guarded GitHub Pages deploy (`.github/workflows/docs.yml`).
* Five tutorials (first agent-safe repository, team server, policy cookbook, migrating a Git repository, troubleshooting a
  blocked integration), FAQ, glossary and an honest comparison with Git, GitHub, Gerrit and Jujutsu. Every command in the tutorials
  is executed against the real binaries in CI and its output compared (`scripts/doc_snippets.py`); every policy in the cookbook goes
  through the real parser.
* Generated reference pages: help topics, `--json` shapes, shell completions, man pages (kept current by `gen_cli_docs.py --check`);
  a `shells` CI job parses the completion scripts in zsh, fish and PowerShell.
* CLI: `--stream` accepted next to the positional stream argument (`policy show|history`, `queue list|run`, `predict`);
  `integrate --idempotency-key`; with `--quiet`, `validate keygen`, `agent start`, `lease acquire` and `push` print the bare
  key, identity, fencing token and proposal id.
* CI: `scripts/check_docs.py` (links, test names, commands, environment variables, refusal messages) now runs in CI;
  `scripts/check_no_stubs.py` fails on TODO/FIXME/XXX/HACK, `todo!()`, "for now", placeholder, stub and mock/fake/dummy code in
  product source, with an allow-list that needs a reason per entry; the README transcript is checked against the demo's output.

## README / ROADMAP notes

* README docs table: add `docs/tutorials/`, `docs/faq.md`, `docs/glossary.md`, `docs/comparison.md`,
  `docs/reference/{cli-json,configuration,completions,help-topics}.md`, and the site (once Pages is enabled: link it).
* README claim "the docs are checked": now true for links, anchors (site build), test names, `ag` commands, environment variables and
  the refusal table; say "tutorial commands are executed in CI" and keep "Windows: not covered by the tutorials" in mind.
* ROADMAP: "Documentation site and tutorials" can move to done, except "deployed to Pages" (owner action). "Windows tutorials" and
  "external link check" are open.
