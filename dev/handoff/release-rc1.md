# Handoff: first release candidate `v0.2.0-rc.1`

State: **prepared and green on `main`, tag NOT pushed.** `git push origin v0.2.0-rc.1` fails through the agent proxy
(`HTTP 403`, "unexpected disconnect while reading sideband packet", twice). Nothing was published; no tag exists on the
remote (`GET /repos/Paffin/AgentGit/tags` is empty). Steps (e) and (f) of the plan (watch the Release run, verify the
published archive, `SHA256SUMS`, `install.sh`, attestations, GHCR image) were therefore **not done**.

## Part 1: Quality green

Root cause of the red `Quality` run [39237558707](https://github.com/Paffin/AgentGit/actions/runs/39237558707):
`agentgit-sandbox --test oci` failed with `ExecutableFileBusy` ("Text file busy", ETXTBSY) when starting the fake
container runtime script that the test had just written. The tests of that binary run on several threads; a child
forked by another thread while the script's write descriptor was open keeps a copy of it until its own `exec`. The
instrumented build widens the window. It is a test race, not a coverage-versus-container problem and not product code.
Fix (`f2f545d`): `Fake::new` waits until one run of the script is accepted (retry on ETXTBSY, 10 s deadline) and
forgets the probe call. The test is not skipped or excluded. `Quality` at `bd0442e`:
[39247617775](https://github.com/Paffin/AgentGit/actions/runs/39247617775) success (coverage and floors).

## Part 2: what is on `main` at `bd0442e5671be178bad6b237587b252eab020f79`

* Version `0.2.0-rc.1` in: workspace version and the 18 inter-crate pins (`Cargo.toml`, `Cargo.lock`,
  `fuzz/Cargo.lock`), `apps/web` and `sdk/typescript` (package and lock), `sdk/python` (`pyproject.toml`,
  `__version__`, `uv.lock`), `THIRD_PARTY_LICENSES.md`, Compose (`compose.yaml`, `.env.example`), Kubernetes
  (`deployment.yaml`, `kustomization.yaml` `newTag`), the version assertions in `containers.yml`, the docs defaults
  (`getting-started.md`, `deployment.md`, `releasing.md`) and the regenerated man pages. The OpenAPI golden keeps the
  literal `0.1.0` (the test passes it in; not a release version).
* `CHANGELOG.md`: `## [0.2.0-rc.1] — 2026-10-19` (the former Unreleased plus the crate renaming), fresh empty Unreleased.
* `scripts/check_release.py`: a rehearsal with an empty `[Unreleased]` falls back to the workspace version's section
  (was an error); unit test added.
* Docs: `install.sh` without `--version` resolves `latest`, which skips pre-releases, so `getting-started.md` now says
  `sh -s -- --version 0.2.0-rc.1`.
* Local gates: fmt, clippy `-D warnings`, `check_release.py v0.2.0-rc.1 --notes`, `check_docs`, `gen_cli_docs --check`,
  `gen_readme_assets --check`, `check_no_stubs`, `check_workflows --strict`, `check_publish_metadata`,
  `sync_crate_files --check`, `test_release_scripts` (36), Python SDK `generate.py --check`, ag-cli version/completions
  tests, server observability test: all pass. `check_deploy.py --strict` fails only for missing `kustomize` and
  `promtool` (CI runs it). TypeScript `generate:check` could not run (no `node_modules`); the SDK CI job does.
* All 11 workflows green on `bd0442e`: CI 39247617786, CodeQL 39247617725, Containers 39247617780, Docs
  39247617763, Fuzz 39247617731, OCI runtimes 39247617863, Packaging 39247617736, Quality 39247617775, Security
  39247617768, Web UI 39247617792, Windows 39247617804 (all `https://github.com/Paffin/AgentGit/actions/runs/<id>`).

## Owner actions

1. Push the tag from a machine with tag-push rights, on the green commit:
   `git tag -a v0.2.0-rc.1 -m "AgentGit 0.2.0-rc.1" bd0442e5671be178bad6b237587b252eab020f79 && git push origin v0.2.0-rc.1`
   (or create it in the GitHub UI). If more commits landed on `main` first, tag the newest green one instead, but then
   the tag must still equal the workspace version (`check_release.py` in the `verify` job enforces it).
2. Watch the `Release` run; then verify the published result (`docs/operations/releasing.md`, "Verifying what you
   downloaded"): `sha256sum -c`, `gh attestation verify`, `install.sh --version 0.2.0-rc.1`, `docker pull`.
3. After the first push to GHCR the packages `agentgit-server` and `agentgit-cli` are private by default: make them
   public (package settings) before the deploy defaults can resolve anonymously.
4. If the Release run fails before anything was published, fix on `main` and tag `v0.2.0-rc.2` (the agents cannot
   dispatch or re-run, 403); never move a tag after assets were published.
