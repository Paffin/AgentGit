# Handoff: the `ag` CLI as a product, and the documentation site

Track owner session: `session_01F8AFjCQ5Q47gBZQNqYFtGQ` (docs/CLI agent). State when this was written: CLI work
landed and tested; docs site and tutorials NOT started in code. Nothing below is claimed beyond what was run.

## Done and tested (on `main`)

All in `crates/ag-cli`; `cargo test -p ag-cli` passes (unit + `ux`, `doctor_status`, `exit_codes`, `json_shapes`,
`json_shapes_remote`, `completions`, plus the existing `cli`, `remote`, `quickstart`); clippy `-D warnings`, fmt,
`scripts/gen_cli_docs.py --check`, `scripts/check_docs.py` green.

- `ag doctor` (`--offline`, `--strict`, `--timeout`, `--json`): version, user config, identity, git, repository
  quick-check (stream heads, reflog tails, audit chain), storage, keys (permissions, missing), interrupted
  operations, waiting conflicts, and per remote: credential source, reachability, `whoami` (identity, roles,
  expiry < 24h = warn, scope, bootstrap token = warn), repository visibility. Exit 0, or 1 when a check fails.
- `ag status` = old output + `summary` (identity, ahead/behind, proposals mine/open, leases held, task of the
  run, remote sync state; `--remote` asks the server). JSON additions are under `summary`.
- `ag config list|get|set|unset|path` (user file: `identity`, `color`; `AG_CONFIG`/XDG/HOME), `ag version`,
  `ag help TOPIC` (8 topics, sources in `crates/ag-cli/help/*.md`), `ag completions SHELL`, `ag man [--dir]`.
- Global `--color`, `-q/--quiet` (confirmations off; `snapshot`/`propose`/`task create` print the bare id),
  `-v/--verbose` (stderr diagnostics), NO_COLOR/CLICOLOR_FORCE honoured, no ANSI in pipes, closed stdout is quiet.
- Errors: duplicate causes collapsed, `hint:` line (`errors.rs` table), `--json` => one JSON line on stderr,
  empty listings explain themselves on stderr.
- Tests: golden `--json` shapes (`tests/golden/json-shapes*.json`, regenerate with `AG_UPDATE_GOLDEN=1`); a
  coverage test fails when a new command has no golden shape (another track's `ag portability` was caught so).
  Exit codes 0-4 are exercised per row of `ag help exit-codes`.

## Not done (exact next steps, in order)

1. **Generated reference pages** (extend `scripts/gen_cli_docs.py`, keep `--check`): write
   `docs/reference/completions/{ag.bash,_ag,ag.fish,_ag.ps1,ag.elv}` from `ag completions`, `docs/reference/man/*`
   from `ag man --dir`, `docs/reference/help-topics.md` from `crates/ag-cli/help/*.md`, and
   `docs/reference/cli-json.md` from `crates/ag-cli/tests/golden/*.json` (list of commands with `--json`, shapes,
   the "keys are only added within schema version 1" rule). `tests/completions.rs` already compares the shipped
   copies when the directories exist, so creating them makes drift fail `cargo test`.
   Also hand-write `docs/reference/configuration.md` (content = `ag help config`) and `docs/reference/completions.md`.
2. **Docs site**: MkDocs (+ Material) was chosen over mdBook: no `SUMMARY.md`, strict mode checks links AND anchors
   (`validation:` block, `strict: true`), built-in offline search, light/dark palette, `docs/README.md` becomes the
   index. Needs `mkdocs.yml`, `docs/requirements.txt` (pin `mkdocs==1.6.1`, `mkdocs-material` exact), a small hook
   to rewrite links that leave `docs/` (`../ROADMAP.md`, `../examples/...`) to GitHub URLs and to append pages that
   exist but are not in `nav` (other tracks add pages), and `.github/workflows/docs.yml` (build `--strict` on every
   push/PR; deploy job on `main` only, with `actions/configure-pages` + `deploy-pages`, `continue-on-error`/guarded
   so it is skipped when Pages is off; the owner must enable Settings -> Pages -> GitHub Actions). Nothing of
   this exists yet, so **Pages deployment is not gated, because it is not written.**
3. **Tutorials** under `docs/tutorials/` (first agent-safe repo, team server, policy cookbook, migrating Git,
   troubleshooting a blocked integration) + `docs/faq.md`, `docs/glossary.md`, `docs/comparison.md`; link them
   from `docs/README.md`. Plan for tested snippets: `scripts/doc_snippets.py` runs fenced `bash` blocks preceded by
   `<!-- doctest -->` in one scratch shell per page (`ag` on PATH, `# expect-exit N` per line, a following
   `<!-- expect -->` ```text block must appear in the output, `...` as wildcard); hook it into
   `crates/ag-cli/tests/` (like `quickstart.rs`) and the docs workflow. Policy JSON in the cookbook: a test in
   `crates/ag-policy/tests/` that parses every ```json block marked `<!-- policy -->` with the real parser.
4. CI: a `shells` job with `apt-get install zsh fish` and `AG_REQUIRE_SHELLS=1 cargo test -p ag-cli --test completions`
   (the PowerShell parse check is untested locally: pwsh is not installed here; verify in Actions).

## Known UX defects NOT fixed (would break scripts)

Positional vs `--stream` is inconsistent (`policy show [STREAM]`, `policy set FILE --stream S`,
`queue list [STREAM]`); `integrate --key` is an idempotency key while `validate run --key` names an issuer key
(consider an alias `--idempotency-key`); not-found/conflict errors exit 1, not 4; `ag cat` has no `--json`.

## Notes for the orchestrator

README rows to add once the pages exist: `docs/tutorials/*`, `docs/faq.md`, `docs/glossary.md`,
`docs/comparison.md`, `docs/reference/{cli-json,configuration,completions}.md`. CHANGELOG entries are in the
agent's final report (commit message of `effc89d` summarizes the CLI features).
