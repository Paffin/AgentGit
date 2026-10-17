# Handoff: release & distribution track

Branch with everything: `wip/release-track` (also `wip/dist-track`, `wip/dist-track-2`: scratch branches used to
iterate CI; safe to delete). What passed its gates is on `main`.

## State

| Deliverable | State |
|---|---|
| 1. `containers.yml` | **Works.** image job (build via cargo-chef Dockerfile, 49 MiB server / 38 MiB CLI, non-root, no shell, HEALTHCHECK, `--version` carries commit, runs read-only + cap-drop ALL, e2e smoke + restart), `image layer cache` job (fresh runner restores the cargo layers from the gha cache), **kind** (k8s 1.33, `deploy/kubernetes/overlays/ci`, PSA restricted, smoke through port-forward, rollout restart keeps data) all green. **Compose** (server + PostgreSQL + SeaweedFS): up, smoke with offline-admin grants, data verified in PostgreSQL and the bucket, restart verified: green up to the observability step. That step (Prometheus scrape OK; the rest unproven) is `continue-on-error: true` on main. |
| 2. `release.yml` rehearsal | Written (`workflow_dispatch` inputs `version`, `publish`; publish only on a matching `v*` tag). On the wip branch: verify, tooling tests, workspace tests, SBOMs, linux x86_64/aarch64 + macOS arm64 binaries (archive unpacked and e2e-smoked), amd64 image and multi-arch merge in a local registry were green; macOS x86_64 (`macos-15-intel`) and arm64 image were still running. **Not yet run from `main`**: do that (below). |
| 3. install paths | `scripts/install.sh` (shellcheck clean in CI, 34 python tests incl. tamper / missing checksum / dash), `packaging/{homebrew,scoop}`, `scripts/render_packaging.py`; docs in getting-started.md and releasing.md. install job in release.yml (3 OS, `--require-attestation`) not yet seen green. |
| 4. publish readiness | Metadata on all crates; 10 publishable (`cargo publish --dry-run` green in `Packaging`), 9 `publish = false` with reasons; `publish-sdks.yml` manual + OIDC only; `scripts/check_publish_metadata.py`. |
| 5. `--version` | Done: `ag 0.1.0 (a1b2c3d4e5f6 2026-10-17)`; shared `build.rs`; tests incl. no-git fallback. |

## Evidence (workflow runs on the wip branches; nothing was published)

* Containers: https://github.com/Paffin/AgentGit/actions/runs/39076286726 (image, cache, kind success; compose failed only at the observability step: the check that counted integrations ran after a server restart reset the counters, since removed)
* Packaging: https://github.com/Paffin/AgentGit/actions/runs/39076286738 (all green: tooling, `cargo publish --dry-run`, npm pack, uv build + twine check)
* Release rehearsal: https://github.com/Paffin/AgentGit/actions/runs/39076286737

## Defects found by running it (all fixed unless noted)

1. `minio/minio:RELEASE.2025-…` and `minio/mc` no longer exist on Docker Hub: Compose could not start. Replaced by SeaweedFS 4.48 (passes `crates/ag-storage/tests/s3.rs` 12/12 locally); `deploy_configs` test updated.
2. `ag-server admin create-repo` ignored the config's `[repos.storage]` template: repos created by the documented path were SQLite+fs on the volume. Fixed (`admin` now reads `--config` / `AG_SERVER_CONFIG`, the image sets it); test `crates/ag-server/tests/admin_config.rs`.
3. **Not fixed (crates/ag-server, HA track):** the `repo.create` response (and `list`) hardcodes `meta_backend: "sqlite"`, `object_backend: "fs"` (`row_to_wire` in `crates/ag-server/src/api/repository.rs`); only `describe` is truthful. The smoke uses `describe`.
4. `ag-worker` did not compile on macOS (`NamespaceSandbox::with_host_ids` missing in the stub): fixed in `namespace_stub.rs`.
5. crates.io: `ag-agent`, `ag-git`, `ag-protocol`, `ag-worker` are taken by another project; `ag-protocol/build.rs` (`../../proto`) and `ag-storage` (`../../migrations`) read outside their crate. See releasing.md#crates-io.
6. An offline `admin grant` reaches a running server after its 5 s binding cache expires (the smoke waits for it).

## What remains

1. Land: if `main` is not up to date with this branch, `git merge origin/wip/release-track`.
2. Run the rehearsal from main. The agents' GitHub token gets `403 Resource not accessible by integration` on `run_workflow` (and on cancel), so use the Actions UI (Release -> Run workflow, version `0.0.0-rc.dry`, publish off) or `gh workflow run release.yml -f version=0.0.0-rc.dry -f publish=false`; or temporarily add `branches: ["wip/**"]` under `on.push` of release.yml and push a wip branch (that is how it was iterated). Last state of that iteration (run 39076286737, commit before the final tidy-up): verify, tooling tests, workspace tests, SBOMs, linux x86_64/aarch64 and macOS arm64 binaries, both image architectures and the multi-arch merge in a local registry all succeeded; `binaries (x86_64-apple-darwin)` on `macos-15-intel` was still building after 25 min (slow/scarce runner: consider cross-compiling it on `macos-14` with `--target x86_64-apple-darwin`), so `release` and `install` had not run. Read failures with `get_job_logs` (fetch the URL it returns with curl and grep; the inline tail is useless). Expect to fix: the `install.sh (…)` job (needs `--require-attestation`; `gh attestation verify` of a rehearsal), `macos-15-intel` runner availability, attestation steps (`id-token`/`attestations: write`), the `release` job's `gh attestation verify --signer-workflow`.
3. Containers: let one run pass the observability step (rules evaluate, Grafana datasource + dashboard, collector), then remove `continue-on-error` in `containers.yml`.
4. Fill the "Evidence" section of `docs/operations/releasing.md` with the green run URLs; rewrite the "What has been verified" table of `docs/operations/deployment.md` (still lists MinIO and says nothing was run; use the Containers workflow results above); mention SeaweedFS and `admin --config` there.
5. `ci.yml`'s `images` job now duplicates `containers.yml` and writes a second gha cache scope: consider removing it (shared file, left alone).
6. Windows: when `windows.yml` is stable, add `x86_64-pc-windows-msvc` (zip: `ag.exe`, `ag-server.exe`; the worker is Linux-only) to the release matrix so `packaging/scoop/agentgit.json` becomes real.
7. Fix finding 3 above; decide on crates.io renames (finding 5).
8. First real release: tag `v0.1.0-rc.1`; unproven pieces are listed in releasing.md "What is not proven".

## Ready-to-paste notes

CHANGELOG (Unreleased):
* `--version` of `ag`, `ag-server` and `ag-worker` prints the semver, git commit and commit date (deterministic without git; `AG_BUILD_COMMIT`/`AG_BUILD_DATE`/`SOURCE_DATE_EPOCH` override).
* Container images are built with cargo-chef layers, a locked allow-list build context and OCI labels; a `Containers` workflow builds them, checks size/user/health/version, proves the gha layer cache, and runs an end-to-end smoke (create repo, grant, mint, clone, push, integrate, read back, metrics, restart) against the container, the Compose stack (PostgreSQL + S3) and a kind cluster.
* Compose uses SeaweedFS instead of MinIO (whose images were withdrawn). `ag-server admin` reads the server config so offline-created repositories use PostgreSQL/S3.
* Release workflow: rehearsal mode (`workflow_dispatch`), native multi-arch images merged into one tag, CycloneDX SBOMs, attestations verified in the run, `scripts/install.sh`, Homebrew/Scoop templates rendered from `SHA256SUMS`, manual-only npm/PyPI trusted-publishing workflow, `cargo publish --dry-run` for the 10 publishable crates.

README/ROADMAP: remove "Prove the deployment story" items already proven (image build/run, compose, kind); keep "publish the SDKs/images" and "cut a first -rc tag"; the README Install section can add the `install.sh` line once a release exists; mention SeaweedFS where README says MinIO.
