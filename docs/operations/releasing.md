# Releasing

A release has two distinct stages. **Preparation** builds and qualifies a candidate for an exact main commit. **Publication** promotes those already tested bytes; it never compiles or repeats the Rust suite.

## Prepare a candidate

[Prepare release](../../.github/workflows/prepare-release.yml) runs the complete [Qualification](../../.github/workflows/qualification.yml) matrix alongside four native Linux/macOS builds and both container architectures. Native archives are unpacked and exercised, SBOMs and checksums are generated, and every file is attested. The installer then runs against the assembled archives on three fresh runners.

Successful preparation retains `release-dist` for 14 days. `candidate.json` binds its version, source SHA and preparation run ID; `candidate-images.json` records immutable container manifest digests. The candidate image tags use the source commit, not a public version or `latest`.

```sh
gh workflow run prepare-release.yml --ref main
```

Use `-f force=true` to rehearse preparation of an already published version. This does not replace a GitHub release or move its version tags. Preparation can take tens of minutes on a cold cache, particularly Intel macOS; it runs separately from continuous development checks.

## Publish a ready candidate

```sh
# Fails immediately if this exact main commit has no complete, unexpired candidate.
gh workflow run request-release.yml --ref main -f ready_only=true
```

The request creates the immutable version tag only after validating a successful preparation for that exact commit. [Release](../../.github/workflows/release.yml) downloads that run's immutable artifact, verifies its complete file list, checksums, source/version/run binding and attestations, and promotes the recorded container digests. It uploads the same files to GitHub and verifies installation from the public release URL on Linux x86_64, Linux arm64 and macOS arm64.

The publication job has a **3-minute** execution limit and its parallel installation jobs have **2-minute** limits. The target is under five minutes for publication of a **ready** candidate. Runner queue time and cold preparation are separate; a missing candidate or failed check is an error, never an excuse to build silently during publication. A timeout is a failed release run, not a successful five-minute release.

To verify the fast path without publishing:

```sh
gh workflow run release.yml --ref main -f version=X.Y.Z -f publish=false
```

This downloads and verifies the prepared candidate but does not move public image tags, create a release, or run public-install checks. For actual publication, `publish=true` is accepted only on the matching `vX.Y.Z` tag. Directly pushing an unprepared tag fails closed.

## Cutting a release

1. Update the workspace, npm and Python versions and lock files; set deployment defaults, regenerate CLI/protocol documentation and dependency notices.
2. Add a nonempty, dated version section in `CHANGELOG.md` and merge the reviewed candidate to `main`.
3. The automatic Request release workflow starts or reuses preparation in the background, then publishes when ready. Ordinary commits continue through fast, component-specific CI. If main moves before the tag is made, the old request stops.
4. Observe preparation and publication separately. Only a complete Release run, including public-install checks, is a verified release.

A manually requested `ready_only=true` publication never waits for preparation. Published versions are skipped and existing tags are never moved. An expired artifact must be prepared again. Retry a failed publication on its existing tag using the same prepared source commit.

## Verifying what you downloaded

```sh
sha256sum -c SHA256SUMS --ignore-missing                 # macOS: shasum -a 256 -c SHA256SUMS --ignore-missing
gh attestation verify agentgit-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz --repo Paffin/AgentGit
gh attestation verify oci://ghcr.io/paffin/agentgit-server:X.Y.Z --repo Paffin/AgentGit
```

The SBOMs (`agentgit-X.Y.Z-<binary>.cdx.json`) list every crate each binary was built from, with versions and
licenses; they are in `SHA256SUMS` and attested like the archives.

## Installing a release

* **`scripts/install.sh`** — POSIX `sh`, `shellcheck`-clean (CI checks). It detects OS and architecture
  (Linux/macOS, x86_64/aarch64; refuses musl, 32-bit and Windows with a message saying what to do instead),
  downloads the archive and `SHA256SUMS`, **refuses to install on a checksum mismatch or a missing entry**,
  verifies the attestation when `gh` is installed and logged in (`--require-attestation` makes that mandatory),
  and copies the binaries atomically into `<prefix>/bin` (default `~/.local`, no root). `--version`, `--prefix`,
  `--dry-run`, `--target` and `--base-url` (a mirror, or a `file://` directory) exist for scripts and tests.
  Unit-tested in `scripts/test_release_scripts.py`, and run against the rehearsal's assembled release on three
  operating systems in CI.
* **Homebrew** — [`packaging/homebrew/agentgit.rb`](../../packaging/homebrew/agentgit.rb) is a template;
  `scripts/render_packaging.py --version X.Y.Z --sums SHA256SUMS --out dir` fills in the version and the four
  macOS/Linux checksums and refuses to write a formula with a missing hash. The release attaches the rendered
  `agentgit.rb`. **There is no tap yet** (`github.com/Paffin/homebrew-agentgit` would be the place); until one
  exists nothing installs through `brew`, and the formula has not been exercised by `brew install`.
* **Scoop** — [`packaging/scoop/agentgit.json`](../../packaging/scoop/agentgit.json) is a template for a Windows
  archive (`ag.exe`, `ag-server.exe`) that **the release workflow does not build yet**, so no release contains it.
  The renderer is tested on a fixture that includes a Windows entry; adding `x86_64-pc-windows-msvc` to the
  `binaries` matrix (as a `.zip`) is what makes the manifest real. Treat it as a placeholder.

## SDKs (npm and PyPI)

[`publish-sdks.yml`](../../.github/workflows/publish-sdks.yml) is started **by hand only** (`workflow_dispatch`).
Without `publish: true` it builds `@agentgit/client` (`npm run check`, `npm pack`) and `agentgit-client`
(`uv build`, `twine check --strict`) and uploads the packages as an artifact. With `publish: true` — honoured only on
the tag `vX.Y.Z` that equals the packages' version — it publishes with **OpenID Connect trusted publishing**:
no token is stored in the repository or the workflow. One-time setup on each registry (nothing here has been done):

* npm → the package's *Settings → Trusted publisher → GitHub Actions*: repository `Paffin/AgentGit`, workflow
  `publish-sdks.yml`, environment `npm`. The job publishes with `npm publish --provenance`.
* PyPI → the project's (or a "pending publisher" for a new project's) *Publishing* page: owner `Paffin`,
  repository `AgentGit`, workflow `publish-sdks.yml`, environment `pypi`.

Both names were free on 2026-10-17 (`scripts/check_publish_metadata.py --online`). The
[`Packaging`](../../.github/workflows/packaging.yml) workflow runs the build-and-check half during qualification.

## crates.io

Nothing is published to crates.io and nothing in CI can publish there. The workspace is ready to be: **21 crates are
publishable** and `Packaging` runs `cargo publish --workspace --dry-run --locked` on all of them (each is packaged,
unpacked and built from its own tarball, in dependency order, against the tarballs of the crates before it), after
`scripts/check_publish_metadata.py` has checked the names and metadata. The twenty-second, `ag-bench`, stays
`publish = false` (a measurement tool for this repository: nothing depends on it and nobody installs it).

### Names

The unprefixed names are not ours: `ag-agent`, `ag-git`, `ag-protocol` and `ag-worker` belong to unrelated projects on
crates.io (checked 2026-10-19 against `https://crates.io/api/v1/crates/<name>`: HTTP 200). So **every** crate is
published as `agentgit-<x>`: the package `agentgit-<x>` lives in `crates/ag-<x>/`, keeps the library name `ag_<x>`
(`[lib] name`), and is declared in `[workspace.dependencies]` as
`ag-<x> = { package = "agentgit-<x>", path = "crates/ag-<x>", version = "…" }`, so no Rust source says `agentgit_`.
All 22 `agentgit-*` names were free on 2026-10-19 (HTTP 404 from the API and from the sparse index
`index.crates.io`); `scripts/check_publish_metadata.py --online` repeats the check for the publishable ones. Names that
are free can be taken by anyone at any time: whoever publishes first should publish the whole set in one sitting.

| Directory | Package (`cargo … -p`) | Binary |
|---|---|---|
| `crates/ag-cli` | `agentgit-cli` | `ag` (`cargo install agentgit-cli`) |
| `crates/ag-server` | `agentgit-server` | `ag-server` |
| `crates/ag-worker` | `agentgit-worker` | `ag-worker` |
| `crates/ag-mcp` | `agentgit-mcp` | `ag-mcp` |
| `crates/ag-<x>` (the other libraries) | `agentgit-<x>` | none |

`cargo build -p …`, `cargo test -p …`, `cargo clippy -p …` take the **package** name, so they say `-p agentgit-server`;
paths, binaries, `use ag_x::` and the directory names did not change. `cargo build --bin ag-server` and
`cargo install --path crates/ag-cli` work as before.

### Files outside a crate

`cargo package` includes only files inside the crate directory, and symlinks do not survive it. Three things lived
outside:

| Needed by | From | Solution |
|---|---|---|
| `agentgit-protocol` (`build.rs` compiles the protocol) | `proto/agwp/` | copy in `crates/ag-protocol/proto/agwp/` |
| `agentgit-storage` (`include_str!`) | `migrations/{sqlite,postgres}/` | copy in `crates/ag-storage/migrations/` |
| `agentgit-server` (`include_str!`) | `migrations/control/` | copy in `crates/ag-server/migrations/control/` |
| `agentgit-ui-assets` (embeds the built Web UI) | `apps/web/dist/` (build output) | `scripts/sync_crate_files.py --ui` copies it into the git-ignored `crates/ag-ui-assets/dist/`, which `include` ships; `build.rs` prefers `apps/web/dist`, then `dist/` |

The top-level `proto/` and `migrations/` stay the source of truth (docs, the Dockerfile and the SDK generators use
them). After changing one, run `python3 scripts/sync_crate_files.py`; `--check` (in `Packaging`) and a test in each of
the three crates (`proto_copy`, `migrations_copy`; they run in `CI`) fail when a copy differs, is missing or is left over.
A published `agentgit-ui-assets` made **without** the `--ui` step embeds nothing, and `agentgit-server` then serves
the "UI not built" page: the release procedure below always runs it, and `Packaging` asserts that the package lists
`dist/index.html`.

### Publish order (dependencies first)

```
agentgit-types  agentgit-object  agentgit-changeset  agentgit-crypto  agentgit-dag  agentgit-policy  agentgit-tree  agentgit-storage  agentgit-agent  agentgit-protocol  agentgit-diff  agentgit-merge  agentgit-integrate  agentgit-ui-assets  agentgit-server  agentgit-client  agentgit-git  agentgit-cli  agentgit-mcp  agentgit-sandbox  agentgit-worker
```

`python3 scripts/check_publish_metadata.py --list` prints it (dev-dependencies count: they keep their version in
the published manifest, so crates.io must already know them). `cargo publish --workspace` computes the same order
and waits for each crate to appear in the index before the next one.

### Publishing (not done; needs the owner's `cargo login` token or trusted publishing)

```sh
git switch --detach vX.Y.Z                                   # publish from the tag
bash scripts/build_ui.sh && python3 scripts/sync_crate_files.py --ui && python3 scripts/sync_crate_files.py --check
python3 scripts/check_publish_metadata.py --online           # the names are still free
cargo publish --workspace --dry-run --locked --allow-dirty   # exactly what Packaging runs
cargo publish --workspace --locked --allow-dirty             # for real
```

`--allow-dirty` is needed only because cargo counts the git-ignored `crates/ag-ui-assets/dist/` it was told to include
as an uncommitted change; check that `git status` is otherwise clean first. An `-rc` version can be
published too; crates.io cannot delete a version (only yank it), so rehearse with the dry run first.

### Limits

* Every crate shows the workspace `README.md` on crates.io; per-crate READMEs would be better and do not exist.
* `cargo publish --workspace` needs Cargo 1.90+; the workspace MSRV (1.89) is for building, not for publishing.
* The dry run proves packaging and the build from the tarballs. It does not prove that crates.io accepts the
  upload (name ownership, rate limits for a burst of new crates). Size is not a concern: the largest package is
  `agentgit-ui-assets` with 242 files, 880 KiB (560 KiB compressed) in the local dry run of 2026-10-19; the limit is 10 MB.
  Nothing has been published.
* The binaries built from the published crates are not the release archives: those come from the workflow above.

## Reproducible builds — status

The inputs are pinned (`Cargo.lock` with `--locked`, a fixed Rust image tag and cargo-chef version in the Dockerfile),
absolute paths are remapped out of the binaries (`--remap-path-prefix`), symbols are stripped, the version string
carries the *commit's* date rather than the wall clock (`SOURCE_DATE_EPOCH` is honoured) and the archives are
byte-deterministic given their inputs (tested). **Bit-for-bit reproducibility of the binaries has not been
demonstrated**: nobody has yet built a release twice on different machines and compared digests, and C
dependencies (`ring`, `aws-lc`, bundled SQLite) are compiled by whatever toolchain the builder has. Until that
is done, verify releases through the attestations, not by rebuilding.

## What is not proven

The rehearsal runs everything but the writes. These have therefore **never run**:

* The `:latest` tag on GHCR (a pre-release does not move it) and `releases/latest` lookup by `install.sh` (only
  pre-releases exist). Push by digest, the registry-side merge, `gh release create` and `install.sh` against real
  release URLs ran for real in the rc.2 run, see [Evidence](#evidence).
* A person's `gh attestation verify` of the **archives** (the workflow verifies them in its run). The owner has run it
  for both images, see [Evidence](#evidence).
* npm and PyPI trusted publishing (needs the one-time registry setup above), and any crates.io publish.
* A Homebrew tap, a Scoop bucket, a Windows archive, musl/Alpine, 32-bit and FreeBSD builds.

Actions are pinned to commit SHAs and checked by `scripts/check_workflows.py --strict`. Release tags are not cryptographically signed by the automatic request, and SBOMs have no separate signature beyond GitHub's attestations. Shell completions and man pages are not included in release archives; the archive builder supports optional `completions/` and `man/` inputs, but the release workflow does not pass them.

## Evidence

The first real run, `v0.2.0-rc.1` ([run 39299147314](https://github.com/Paffin/AgentGit/actions/runs/39299147314),
commit `bd0442e`), **failed** in `assemble the release`, step *Release notes from the CHANGELOG*:
`'0.2.0-rc.1' is not a version tag like v1.2.3`. The rehearsal passes `--dry-run`, which accepts a bare
version; a real run does not, and nothing had exercised that combination. Fixed in `202fbdd`: the notes step passes
`v$VERSION`, and a test fails on a bare version in any `check_release.py` call in `release.yml`. `gh release create`
also falls back to `upload --clobber` + `edit` when a release page already exists (the GitHub UI creates an empty one
together with a tag, as happened for rc.1). `v0.2.0-rc.1` stays as it is: no assets, no notes.

The second real run, `v0.2.0-rc.2` ([run 39434938489](https://github.com/Paffin/AgentGit/actions/runs/39434938489),
commit `0ad4753`), **succeeded in all 15 jobs** and published the pre-release
[v0.2.0-rc.2](https://github.com/Paffin/AgentGit/releases/tag/v0.2.0-rc.2): four archives, three SBOMs (`ag`,
`ag-server`, `ag-worker`; none for `ag-mcp`), `SHA256SUMS`, `agentgit.rb`.

What the published rc.2 proved (checked afterwards from outside the workflow, on 2026-10-20, with the downloaded files):

| Check | Result |
|---|---|
| `sha256sum -c SHA256SUMS` on the downloaded files | all 7 listed files OK (`agentgit.rb` is not listed) |
| SBOMs | valid CycloneDX 1.5 JSON, 305 / 333 / 209 components, root version `0.2.0-rc.2` |
| linux x86_64 archive | `ag`, `ag-server`, `ag-worker` report `0.2.0-rc.2 (0ad475333ee2 …)`; `ag doctor` passes; the README quick start (`init`, `snapshot`, `propose`, `integrate`, `log`) works; `ag-server` `create-repo`/`grant`/`mint`/`serve`, `/healthz` 200, `/readyz` 200, `ag clone`, `ag push --title …`, `/metrics` work |
| `scripts/install.sh --version 0.2.0-rc.2` against the real release URL | `--dry-run` installs nothing; the real run checks the checksum and installs four working binaries; also works piped from `raw.githubusercontent.com` |
| GHCR, anonymous | `agentgit-server:0.2.0-rc.2` and `agentgit-cli:0.2.0-rc.2` are OCI indexes with linux/amd64 + linux/arm64 (+ attestation manifests) |
| Attestations | the attestations API returns SLSA provenance v1 for the archives and for both image digests, naming `release.yml` at `refs/tags/v0.2.0-rc.2`; subject digests match the downloaded files |
| `install.sh` in the run | success on Linux x86_64, Linux aarch64 and macOS, against the assembled release |
| `gh attestation verify oci://ghcr.io/paffin/agentgit-server:0.2.0-rc.2` and the same for `agentgit-cli`, **by the owner** with a logged-in `gh` | both verified: SLSA provenance v1, built by `.github/workflows/release.yml` at `refs/tags/v0.2.0-rc.2` in `Paffin/AgentGit` |

Not verified by that pass: `gh attestation verify` of the **archives** (the agent environment has no working `gh`
login; the owner later verified the two images, the archives' signatures were only inspected, and the workflow
verified them inside the run); the macOS and aarch64 archives were not run outside CI; the images were not pulled
or started from GHCR; the Homebrew formula was not installed; there is no SBOM for `ag-mcp`; `install.sh` without `--version` cannot work while only pre-releases exist (`releases/latest`
skips them). Details: `dev/handoff/release-rc2.md`.

Rehearsal and container runs on `main` are listed in the project's change log entry for this work; the workflow
runs themselves (`Release`, `Containers`, `Packaging`) are in the Actions tab with their uploaded artifacts.

## Release requests from main

A reviewed version bump on main starts or reuses Prepare release for that exact commit. Preparation includes the full qualification matrix, native builds, provenance and installation tests. The request stops if main moves, and cannot create a tag after a failed preparation. Publication only consumes an immutable successful candidate; it does not repeat the full test suite or rebuild archives.

Manual publication accepts a version with or without its leading `v`, but validates the actual tag selected in the workflow. A branch or a different version is refused before publishing. Tests execute the workflow’s mode-selection shell block and pass its result to the real version/changelog validator.

Python CI uses `uv sync --locked`; package checks run `uv lock --check` before building. A version bump must therefore include the normalized project version in `sdk/python/uv.lock`, the public Python `__version__`, regenerated CLI reference and third-party licence list. Deployment defaults track the candidate workspace version; verify image availability before using them. Update published installation links only after the new release assets are verified.


## Published archive installation checks

From beta.7, a publishing run installs the version-pinned archive from its public GitHub release URL on fresh Linux and macOS runners, verifies its checksum and attestation, records the OS and binary versions, and runs the remote first-change smoke workflow. Rehearsals install the assembled local archive. This supplies repeatable CI installation evidence; it does not test an interactive macOS download quarantine or Homebrew.


## Beta.5 publication evidence {#beta5-publication-evidence}

`v0.3.0-beta.5` was published on 2026-10-28 from
`a36168b8d79f59b0557b9a9caa99f87b2ff9cb64`. The complete
[release run](https://github.com/Paffin/AgentGit/actions/runs/40738103603) passed, including
Linux/macOS x86_64 and aarch64 native archive smoke flows, multi-architecture images, SBOMs,
attestations and the three installation jobs. Those installation jobs exercise assembled
release files; they are not independent clean-device or Homebrew qualification.

The published Linux x86_64 archive was downloaded separately. Its SHA-256 matched the published
`SHA256SUMS`: `123e924fa8d16cd636d1332c4b3667ed69be2fb6d0f6523e652f851a635e10e2`.
GitHub attestation verification accepted the bundle with the repository, release workflow,
exact source digest and rejection of self-hosted runners specified explicitly.

The downloaded server created a repository and task at schema 6. Opening that data with
development schema 7 preserved existing task/history/policy-related records, replayed the
old task-creation idempotency key, and allowed a new persistent task-source link. The beta.5
binary refused the upgraded schema; it successfully verified the untouched pre-upgrade backup.
This verifies that upgrade path, not production readiness or a general downgrade operation.
External MCP-host, clean macOS, physical-phone, screen-reader and pilot reports remain open in
the [beta programme](../community/beta.md).


Migration regression fixtures keep their source schema pinned but compare the upgraded database
with the current storage schema version. A new migration must retain the fixture's data and
untouched backup assertions; changing the destination version must not disable those checks.
Run the SDK's full lint command before requesting a release: its formatting check is separate
from type checking and live API tests.

### Beta.6 candidate: schema-8 upgrade evidence (2026-10-29)

The published beta.5 Linux archive was used to create a task and failed run before opening
its repository with the beta.6 development server. Schema 6 → 8 preserved existing rows,
run history and the creation-idempotency replay. The new attempt policy then allowed a
second distinct run. The old binary refused schema 8, and restoring the pre-upgrade copy
passed verification with that old binary. This qualifies the upgrade path, not publication
of beta.6. The release workflow still gates and builds the exact tagged commit.
