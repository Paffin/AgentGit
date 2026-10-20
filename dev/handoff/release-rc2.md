# Handoff: release candidates `v0.2.0-rc.1` (failed) and `v0.2.0-rc.2` (published, verified)

Current state: **`v0.2.0-rc.2` is published and verified**, see the last section. The rest is the history of rc.1.

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

## Update: the tag was pushed, the run failed, rc.2 is prepared

Run [39299147314](https://github.com/Paffin/AgentGit/actions/runs/39299147314) on `v0.2.0-rc.1` (`bd0442e`):
* success: verify, release tooling tests, tests, SBOMs, all four `binaries`, `image (amd64)`, `image (arm64)`,
  `multi-arch manifest`.
* **failure: `assemble the release`**, step 7 *Release notes from the CHANGELOG*:
  `error: '0.2.0-rc.1' is not a version tag like v1.2.3 or v1.2.3-rc.1`. Root cause: `release.yml` called
  `check_release.py "$VERSION" --notes` (a bare version is valid only with `--dry-run`, which every rehearsal used).
  The `install.sh` jobs were skipped. No release assets and no notes exist. A GitHub release page `v0.2.0-rc.1` does
  exist (created 18:01:53Z, empty, pre-release), presumably made together with the tag in the UI.
* Side effect: both images were pushed to GHCR as `:0.2.0-rc.1`. Anonymous pull works (the packages are already
  public); the indexes list linux/amd64 and linux/arm64. Image attestation not verified (no logged-in `gh` here).
* Fix on `main`: `202fbdd` (notes use `v$VERSION`, regression test, an existing release page is filled in with
  `gh release upload --clobber` + `gh release edit`). The bump to `0.2.0-rc.2` and its CHANGELOG section are the next
  commit. Local gates: `check_release.py v0.2.0-rc.2 --notes`, check_docs, `gen_cli_docs --check`,
  check_publish_metadata, `check_workflows --strict`, test_release_scripts (38).

### Owner actions for rc.2

1. Take the newest commit on `main` (subject `release: 0.2.0-rc.2`, or later) once its CI is green.
2. Do not move or delete `v0.2.0-rc.1`; optionally delete the empty `v0.2.0-rc.1` release page in the UI.
3. `git tag -a v0.2.0-rc.2 -m "AgentGit 0.2.0-rc.2" <that commit> && git push origin v0.2.0-rc.2`.
4. Ask for the verification pass (archives, `SHA256SUMS`, `install.sh --version 0.2.0-rc.2`, `gh attestation verify`
   with a logged-in `gh`, image attestation, SBOM parse). GHCR visibility needs no action.

## Result: `v0.2.0-rc.2` published and verified (2026-10-20)

Run [39434938489](https://github.com/Paffin/AgentGit/actions/runs/39434938489) on tag `v0.2.0-rc.2`
(commit `0ad475333ee2`): **all 15 jobs success** (verify, tooling tests, tests, SBOMs, 4 binaries, 2 images, manifest
merge, assemble, `install.sh` on ubuntu x86_64, ubuntu arm64, macOS). The release
<https://github.com/Paffin/AgentGit/releases/tag/v0.2.0-rc.2> is a pre-release with notes cut from the CHANGELOG and
9 assets: 4 archives (linux x86_64/aarch64, macOS aarch64/x86_64), 3 SBOMs (`ag`, `ag-server`, `ag-worker`),
`SHA256SUMS`, `agentgit.rb`. Verified from this environment, with the published files only (downloaded to `/dev/shm`):

* `sha256sum -c SHA256SUMS`: all 7 listed files OK (4 archives, 3 SBOMs). `agentgit.rb` is not in `SHA256SUMS`.
* SBOMs: valid JSON, `bomFormat` CycloneDX, `specVersion` 1.5, 305 / 333 / 209 components for `ag` / `ag-server` /
  `ag-worker`, root component version `0.2.0-rc.2`. There is no SBOM for `ag-mcp` (the workflow builds three).
* Linux x86_64 archive: contents `ag ag-server ag-worker ag-mcp LICENSE NOTICE README.md`. `ag`, `ag-server`,
  `ag-worker` report `0.2.0-rc.2 (0ad475333ee2 2026-10-19)` (the tag's commit), `ag-mcp` `0.2.0-rc.2`.
  `ag doctor`: 4 ok, 1 skip (not in a repository), exit 0.
* README quick start with the released `ag`: `init`, `snapshot`, `propose`, `integrate` (fast-forward), `log`: all work.
* Server flow with the released `ag-server`: `admin create-repo/grant/mint`, `serve`; `/healthz` 200 `ok`,
  `/readyz` 200 `ready`; `ag clone`, `snapshot`, `ag push --title …` opened a proposal; `/metrics` answers; the Web UI
  is embedded and served at `/`. Note the README's server snippet says `ag push` but `--title` is required.
* `scripts/install.sh --version 0.2.0-rc.2 --prefix <scratch>` against the real release URL: `--dry-run` prints the
  right URL and installs nothing; the real run verifies the checksum, installs four binaries that run. Also works
  piped from `raw.githubusercontent.com/…/main/scripts/install.sh`. Without `--version` it fails with "no published
  release found" because `releases/latest` skips pre-releases (expected, documented).
* GHCR, anonymous token + manifest GET: `agentgit-server:0.2.0-rc.2` (`sha256:ac40b0ca5a5e…`) and
  `agentgit-cli:0.2.0-rc.2` (`sha256:94b055a70989…`) are OCI indexes with linux/amd64, linux/arm64 and two
  attestation manifests each. No image was pulled or started.
* Attestations: `gh` is not authenticated here (its token is rejected), so `gh attestation verify` could not run. The
  unauthenticated attestations API returns one SLSA provenance v1 statement for the archives (subjects include
  `SHA256SUMS` and the archives, whose digests match), and one for each image digest; each names workflow
  `.github/workflows/release.yml` at `refs/tags/v0.2.0-rc.2` in `Paffin/AgentGit` and has a transparency-log entry.
  **Signatures and certificates were not verified here.** The workflow itself verified every one in its run.

Not verified: the macOS and aarch64 archives were not run by me (the `install.sh` CI jobs did that); Homebrew
formula not installed; `:latest` does not exist for a pre-release; `gh attestation verify` by a person; pulling and
starting the images from GHCR.

### Owner actions

None are required for rc.2. Optional: delete the empty `v0.2.0-rc.1` release page; run
`gh attestation verify agentgit-0.2.0-rc.2-<target>.tar.gz --repo Paffin/AgentGit` and
`gh attestation verify oci://ghcr.io/paffin/agentgit-server:0.2.0-rc.2 --repo Paffin/AgentGit` with a logged-in `gh`.
