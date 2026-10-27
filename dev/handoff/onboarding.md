# Handoff: the five-minute start (ROADMAP items 2 and 3)

Answers the first outside validation (`dev/feedback/validation-2026-10-21.md`): setup needed Rust, a build and hand-made
tokens, and there were too many new words. Everything below is on `main`, in five slices, each pushed on its own.

## What exists

| Piece | Where | Tests |
|---|---|---|
| Policy presets `solo`, `team`, `agents`, `strict` as data | `crates/ag-policy/src/presets.rs` | unit + `crates/ag-policy/tests/presets.rs` (through the real evaluator) |
| Plain-language policy reading, built from the document | `crates/ag-policy/src/explain.rs` | `every_field_of_the_document_is_explained` fails when `Policy` gains a field with no sentence |
| `ag policy preset list|show|apply`, `ag policy explain [FILE|current]`, `ag init --preset`, `ag remote policy --preset/--explain`, `ag-server admin policy-preset` | `crates/ag-cli/src/cmds/policy.rs`, `repo.rs`, `remote.rs`; `crates/ag-server/src/main.rs` | `crates/ag-cli/tests/presets.rs`, `crates/ag-server/tests/admin_config.rs` |
| One explanation per refusal code, with the commands that fix it | `crates/ag-integrate/src/explain.rs` | enumerates `Unmet::CODES`, `ConflictKind::CODES` (both new, each checked against the variants) |
| `ag explain [PROPOSAL]` and the hints under `ag integrate` / `ag remote integrate|prepare` | `crates/ag-cli/src/cmds/explain.rs` | `crates/ag-cli/tests/explain.rs`; tutorials' expected output |
| `ag login`, `ag logout`, the saved credential | `crates/ag-cli/src/credstore.rs`, `cmds/login.rs`; last step of `credential_source` in `cmds/remote.rs` | `crates/ag-cli/tests/login.rs` |
| `ag-server dev` | `crates/ag-server/src/dev.rs`, `main.rs` | `crates/ag-server/tests/dev.rs` (real process), `crates/ag-cli/tests/dev_flow.rs` (the getting-started path) |
| Sign-in link `#token=` in the Web UI | `apps/web/src/lib/login-link.ts`, `main.ts`, `stores/auth.ts` (`signInFromLink`), `SignInView.vue` | `login-link.spec.ts`, `auth.spec.ts`, `apps/web/e2e/dev-link.spec.ts` (real `ag-server dev`, real browser; needs `AG_E2E_UI=embedded`) |
| Docs | `docs/getting-started.md` (top rewritten), `docs/concepts-cheatsheet.md`, `docs/guides/policy.md#start-from-a-preset`, `docs/guides/web-ui.md#signing-in` | `scripts/check_docs.py`; the `Docs` workflow |

## Decisions worth knowing

- **Presets are `Policy` values, not a mode.** `Preset::build(&PresetOptions)` returns what `Policy::parse` accepts; storing one is
  `set_policy`. What a preset cannot know is an option: `--admin` (default: you; `solo` leaves it open) and the CI issuer
  (`--ci-key NAME` uses or creates the repository's key `issuer-NAME`; `--issuer IDENTITY=HEX` names one). With an issuer, `solo`,
  `team` and `agents` also require the check `tests`; `strict` cannot be built without one (it says how to give one).
- **`explain` has no prose about presets.** It reads the document. The test that compares it with the evaluator
  (`what_the_explanation_says_about_agents_is_what_the_evaluator_does`) is the guard against drift.
- **The agent envelope is independent of `auto_integrate`** in the evaluator (`within_autonomy` only needs `max_files > 0` and paths);
  `explain` says so, and the `agents` preset sets it.
- **No new API surface.** Presets are applied through the existing `set_policy`; there is no `/v1/policy/presets` endpoint,
  so OpenAPI and the SDKs did not change. If the Web UI's policy editor (ROADMAP 1) wants a catalogue from the server, add
  `GET /v1/policy/presets` returning `Preset::ALL` with `build()` output and `explain()` sentences, then regenerate the OpenAPI
  golden (`AG_UPDATE_GOLDEN=1 cargo test -p agentgit-server --test observability`) and the SDKs.
- **The sign-in link carries the credential in the fragment.** It is not one-time: making it one needs an exchange endpoint
  (new API surface). The credential is the normal short-lived one (8 h, replaced and the old one revoked at every start). The
  trade-offs (terminal scrollback, the browser's history database, `--open` putting the link in the process list) are written in
  `docs/guides/web-ui.md#signing-in`.
- **`ag-server dev` seeds through the engine**, not HTTP: a virtual workspace, `propose`, `integrate`, then the policy (so the
  content lands under the open default). On restart nothing is re-seeded and a policy you changed stays (`policy_applied` is
  false when a version exists). It also makes `user:reviewer` (reviewer of `demo/welcome`) because nobody's own approval counts
  and a lone evaluator could otherwise never see a review.
- **Credentials file**: `credentials.json` beside `config.json` (`AG_CONFIG`, XDG, `~/.config`), 0600, atomic replace, keyed by
  `scheme://host:port`. Precedence: per-remote `token_env`/`token_file`, `AG_TOKEN_<REMOTE>`, `AG_TOKEN`, then the saved login.
  `ag login` asks the server (`whoami`) before saving.
- **Python 3.9**: already done by `51bd78b` (`scripts/test_py39_compat.py` and the `scripts-py39` job in `ci.yml`); re-run here
  under 3.9.

## Not done / not verified

- `ag explain` works on **local** repositories. For a proposal on a server the refusal output carries the same hints, and
  `ag remote prepare ID` shows what is missing, but there is no `ag remote explain`.
- The Docker one-liner in `docs/getting-started.md` is **not run** (no Docker daemon here); the image of a release that has
  `dev` does not exist yet. The path was exercised with `--host 0.0.0.0 --allow-non-loopback` on the host, and the banner shows
  `127.0.0.1` for an unspecified address.
- `ag-server dev` is not in any published archive; the five-minute path on `0.2.0-rc.2` is the three-step variant documented
  in the same page (run against the real rc.2 Linux archive). macOS/aarch64 archives were not run.
- `--open` was only exercised for its failure message (no browser opener here).
- Windows: `ag login` uses `private_file` (ACL restriction) like the other secret files; not run on Windows.
- The Web UI e2e `dev-link.spec.ts` was run in Chromium only.
- `mkdocs build --strict` currently fails on a link from `docs/design/web-ux-review.md` to an anchor
  (`guides/web-ui.md#plain-words`) that another track has not added yet; this work adds no warning.

## Following up

- ROADMAP item 2 still lists "a setup that makes the first agent credential in one click" (UI) and prebuilt binaries/Docker in the
  README's first lines; item 3's "concepts introduced only when the screen needs them" is UI work.
- A `strict`-like preset for the Web UI's policy editor could reuse `Preset::build` + `explain` through a small endpoint (above).
