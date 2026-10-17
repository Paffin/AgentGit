# Releasing

A release is a tag `vX.Y.Z` (or `vX.Y.Z-rc.1`) on `main`. Pushing the tag runs
[`.github/workflows/release.yml`](../../.github/workflows/release.yml), which builds, checks and **publishes** it.
The same workflow can be started by hand as a **rehearsal** that does everything except publish; that is how it
is tested, and how you should test changes to it (see [Rehearsing](#rehearsing-a-release-without-publishing)).

## What a tag does

| Stage | What happens |
|---|---|
| **verify** | `scripts/check_release.py` fails the run unless the tag equals the workspace version in `Cargo.toml` **and** `CHANGELOG.md` has a non-empty `## [X.Y.Z]` section. |
| **release tooling tests** | `scripts/test_release_scripts.py`: the version check, the archive builder, the Homebrew/Scoop renderer and `install.sh` (with a tampered download, a missing checksum, a wrong platform …). |
| **tests** | `cargo test --workspace --locked` (with unprivileged user namespaces enabled and the sandbox suites required, as in CI). |
| **binaries** | `ag`, `ag-server` and `ag-worker` built `--release --locked` natively on Linux x86_64 (`ubuntu-22.04`, so glibc ≥ 2.35), Linux aarch64 (`ubuntu-22.04-arm`), macOS aarch64 and macOS x86_64, and packed by `scripts/package_release.py` into `agentgit-X.Y.Z-<target>.tar.gz` (the three binaries, `LICENSE`, `NOTICE`, `README.md`; the archive is deterministic). Every archive is then **unpacked and run**: the three `--version` strings must carry the version and this commit, and the end-to-end smoke ([`scripts/e2e_smoke.py`](../../scripts/e2e_smoke.py): create a repository, grant, mint a credential, `ag clone`, snapshot, `ag push`, integrate, read back over HTTP, check metrics) runs against the unpacked `ag-server`. |
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

For the first release use an `-rc` tag (`v0.1.0-rc.1`): it publishes a pre-release and finds what only a real
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

Nothing is published to crates.io and nothing in CI can publish there. Every crate carries complete metadata
(description, license, repository, homepage, readme, keywords, categories, rust-version —
`scripts/check_publish_metadata.py` enforces it), and **ten crates are publishable today**; `Packaging` runs
`cargo publish --dry-run` on them, which packages each from its tarball and compiles it against the others'
tarballs. Publish order (dependencies first):

```
ag-types  ag-object  ag-changeset  ag-crypto  ag-dag  ag-diff  ag-tree  ag-merge  ag-policy  ag-sandbox
```

The other nine crates are marked `publish = false`, each with the reason in its `Cargo.toml`. They are blocked by
two things that need a decision, not just a tag:

1. **Four names are already taken on crates.io by an unrelated project** (checked 2026-10-17): `ag-agent`,
   `ag-git`, `ag-protocol` and `ag-worker`. Publishing means renaming packages (for example `agentgit-protocol`).
   A package can be renamed without touching Rust code — keep `[lib] name = "ag_protocol"` — but `-p ag-protocol`
   appears in CI, the Dockerfile and the docs, so it is a coordinated change. The remaining `ag-*` names are free
   today and unclaimed names can be taken by anyone at any time.
2. **Two crates read files outside their own directory**, which `cargo package` does not include:
   `ag-protocol/build.rs` compiles `../../proto`, and `ag-storage` embeds `../../migrations`. Both directories would
   move into their crates (the migrations are also read by the HA work, so do this when it is quiet).

Everything that depends on those two (`ag-agent`, `ag-integrate`, `ag-git`, `ag-client`, `ag-server`, `ag-cli`,
`ag-worker`) follows. Until then `cargo install` works from a checkout (`cargo install --path crates/ag-cli`).

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

* `docker push` of each architecture by digest to GHCR, the registry-side merge there, the `:latest` tag, and
  the attestation of the images (`push-to-registry`). The merge logic itself is exercised against a local
  registry; GHCR permissions (package visibility, `packages: write` for the workflow) are not.
* `gh release create` with the assembled files, and the release page that users see.
* `install.sh` and `gh attestation verify` against real release URLs (the rehearsal uses a `file://` base URL;
  `releases/latest` redirect lookup is untested because no release exists).
* npm and PyPI trusted publishing (needs the one-time registry setup above), and any crates.io publish.
* A Homebrew tap, a Scoop bucket, a Windows archive, musl/Alpine, 32-bit and FreeBSD builds.

Also not done: actions are referenced by major version tag, not by commit SHA (Dependabot keeps them current; pin
by SHA if your threat model includes a compromised action tag); no signing of Git tags or of the SBOM beyond
GitHub's attestations; shell completions and man pages are not generated yet (the archive builder includes
`completions/` and `man/` as soon as they exist).

## Evidence

Rehearsal and container runs on `main` are listed in the project's change log entry for this work; the workflow
runs themselves (`Release`, `Containers`, `Packaging`) are in the Actions tab with their uploaded artifacts.
