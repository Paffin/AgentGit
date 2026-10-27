# Releasing

A release is a tag `vX.Y.Z` (or `vX.Y.Z-rc.1`) on `main`. Pushing the tag runs
[`.github/workflows/release.yml`](../../.github/workflows/release.yml), which builds, checks and **publishes** it.
The same workflow can be started by hand as a **rehearsal** that does everything except publish; that is how it
is tested, and how you should test changes to it (see [Rehearsing](#rehearsing-a-release-without-publishing)).

## What a tag does

| Stage | What happens |
|---|---|
| **verify** | `scripts/check_release.py` fails the run unless the tag equals the workspace version in `Cargo.toml` **and** `CHANGELOG.md` has a non-empty `## [X.Y.Z]` section. |
| **release tooling tests** | `scripts/test_release_scripts.py` and `scripts/test_sbom_release.py`: the version check, the archive builder, the Homebrew/Scoop renderer and `install.sh` (with a tampered download, a missing checksum, a wrong platform …). |
| **tests** | `cargo test --workspace --locked` (with unprivileged user namespaces enabled and the sandbox suites required, as in CI). |
| **binaries** | `ag`, `ag-server`, `ag-worker` and `ag-mcp` built `--release --locked` natively on Linux x86_64 (`ubuntu-22.04`, so glibc ≥ 2.35), Linux aarch64 (`ubuntu-22.04-arm`), macOS aarch64 and macOS x86_64, and packed by `scripts/package_release.py` into `agentgit-X.Y.Z-<target>.tar.gz` (the four binaries, `LICENSE`, `NOTICE`, `README.md`; the archive is deterministic). Every archive is then **unpacked and run**: the `--version` strings of the first three must carry the version and this commit (`ag-mcp` has no build script: its crate version only), and the end-to-end smoke ([`scripts/e2e_smoke.py`](../../scripts/e2e_smoke.py): create a repository, grant, mint a credential, `ag clone`, snapshot, `ag push`, integrate, read back over HTTP, check metrics) runs against the unpacked `ag-server`. |
| **sbom** | One CycloneDX 1.5 SBOM per shipped binary (`cargo cyclonedx --describe binaries`, from `Cargo.lock`). |
| **images** | `ghcr.io/paffin/agentgit-server` and `…/agentgit-cli`: each architecture is built **natively** (`ubuntu-latest` for amd64, `ubuntu-22.04-arm` for arm64; an emulated Rust build would take hours) from [`deploy/docker/Dockerfile`](../../deploy/docker/Dockerfile) and pushed by digest, then **merged into one multi-arch tag** (`:X.Y.Z`, plus `:latest` for a non-prerelease) with SBOM and max-mode provenance attached. |
| **release** | `SHA256SUMS` over the archives and SBOMs, the rendered Homebrew formula (`agentgit.rb`), GitHub build-provenance attestations for every file and image (and a `gh attestation verify` of each in the same run), release notes cut from the CHANGELOG section, and the GitHub release (a pre-release when the tag has a `-suffix`). |
| **install** | `scripts/install.sh` installs the assembled release on Linux x86_64, Linux aarch64 and macOS with `--require-attestation`; a corrupted archive must be refused and install nothing; the installed binaries run the end-to-end smoke. |

## Rehearsing a release without publishing

*Actions → Release → Run workflow*, or from a terminal:

```sh
gh workflow run release.yml --ref main -f version=0.0.0-rc.dry -f publish=false
```

`version` is only checked for being semver (it need not match `Cargo.toml`; the archives are named after it, the
binaries inside report the `Cargo.toml` version), and release notes fall back to `## [Unreleased]` when the
CHANGELOG has no section of that name. `publish=true` is honoured **only** when the run is on the tag `vX.Y.Z`
that `version` names — on a branch it fails immediately with "Nothing was published". Everything that would
write to the outside world (`docker push` to GHCR, `gh release create`) is behind that one flag, which the
`verify` job computes and every later job reads.

What the rehearsal proves, and how it differs from the real thing:

| Step | In a rehearsal | In a real release |
|---|---|---|
| Version, CHANGELOG, workspace tests, four native builds, archives, SBOMs, `SHA256SUMS`, formula rendering | **identical** | identical |
| Attestations | really created and then really verified with `gh attestation verify` (Sigstore, this workflow's identity) | identical |
| Images | built natively for amd64 **and** arm64, started and health-checked; exported as tarballs | pushed by digest to GHCR |
| Multi-arch manifest | merged with `docker buildx imagetools create` in a throw-away **local registry** and inspected: it must list `linux/amd64` and `linux/arm64`, and the merged tag must run | merged on GHCR, then attested |
| GitHub release | the command is printed; all files are uploaded as the workflow artifact `release-dist` | `gh release create` |
| `install.sh` | installs `release-dist` through a `file://` base URL | would download from the release URL |

Evidence (all on `main`'s code, nothing published): see [Evidence](#evidence) at the end of this page.

## Cutting a release

```sh
# 1. bump the version in Cargo.toml ([workspace.package]) and in sdk/typescript/package.json and
#    sdk/python/pyproject.toml (scripts/check_publish_metadata.py insists they agree), refresh Cargo.lock
cargo update -w
# 2. move [Unreleased] into a dated [X.Y.Z] section of CHANGELOG.md
# 3. check what the workflow will check
python3 scripts/check_release.py vX.Y.Z --notes
python3 scripts/check_publish_metadata.py
cargo test --workspace --locked
# 4. rehearse on the exact commit you are about to tag
gh workflow run release.yml --ref main -f version=X.Y.Z-rc.dry -f publish=false
# 5. tag after the commit is on main and the rehearsal is green
git tag -s vX.Y.Z -m "AgentGit X.Y.Z" && git push origin vX.Y.Z
```

For the first release use an `-rc` tag (`v0.2.0-rc.1`): it publishes a pre-release and finds what only a real
publish can find (see [What is not proven](#what-is-not-proven)).

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
[`Packaging`](../../.github/workflows/packaging.yml) workflow runs the build-and-check half on every change.

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

Also not done: actions are referenced by major version tag, not by commit SHA (Dependabot keeps them current; pin
by SHA if your threat model includes a compromised action tag); no signing of Git tags or of the SBOM beyond
GitHub's attestations; shell completions and man pages are not generated yet (the archive builder includes
`completions/` and `man/` as soon as they exist).

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

A reviewed version bump on `main`, with matching package versions and a nonempty changelog section, requests the next release automatically. `Request release` waits for successful CI, CodeQL, Containers, Docs, Packaging, Quality, Security, Web UI and Windows runs for that exact push, plus any other workflows triggered by it. A newer main commit cancels the request; a failed or timed-out check creates no tag.

After the checks pass, the request creates `v<version>` at the checked commit and dispatches the existing Release workflow on that tag. That workflow still builds, tests, attests and verifies the actual archives and images before publication. The request itself is not a successful release. Existing published versions are skipped; an existing tag is never moved. If a request failed after creating its tag, rerun Request release on the same main commit. If main has moved, run Release manually on the existing tag with its matching version. Rehearsals remain available through Release with `publish=false`.

The request uses a write token only on trusted main pushes or a manual run on main; it has no pull-request or workflow-run trigger and does not persist checkout credentials.

Required checks are identified by their workflow file paths, so another workflow using the same display name cannot hide a failed or unfinished check. Optional workflows triggered by the same commit must finish successfully too.
