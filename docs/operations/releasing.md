# Releasing

A release is a tag `vX.Y.Z` (or `vX.Y.Z-rc.1`) on `main`. Pushing the tag runs
[`.github/workflows/release.yml`](../../.github/workflows/release.yml):

1. **verify** — `scripts/check_release.py` fails the run unless the tag equals the workspace version in
   `Cargo.toml` **and** `CHANGELOG.md` has a non-empty `## [X.Y.Z]` section.
2. **test** — `cargo test --workspace --locked`.
3. **binaries** — `ag` and `ag-server`, built with `--locked --release` on Linux x86_64 and aarch64 and macOS
   x86_64 and aarch64, as `agentgit-X.Y.Z-<target>.tar.gz` (with `LICENSE`, `NOTICE`, `README.md`).
4. **images** — `ghcr.io/paffin/agentgit-server:X.Y.Z` and `…/agentgit-cli:X.Y.Z` (linux/amd64, distroless,
   non-root) with SBOM and max-mode provenance attached.
5. **publish** — `SHA256SUMS`, GitHub build-provenance attestations for the archives and images, and a GitHub
   release whose notes are the CHANGELOG section (pre-release if the tag has a `-suffix`).

## Cutting a release

```sh
# 1. bump the version in Cargo.toml ([workspace.package]) and refresh Cargo.lock
cargo update -w
# 2. move [Unreleased] into a dated [X.Y.Z] section of CHANGELOG.md
# 3. check what the workflow will check
python3 scripts/check_release.py vX.Y.Z --notes
cargo test --workspace --locked
# 4. tag after the commit is on main
git tag -s vX.Y.Z -m "AgentGit X.Y.Z" && git push origin vX.Y.Z
```

## Verifying what you downloaded

```sh
sha256sum -c SHA256SUMS --ignore-missing
gh attestation verify agentgit-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz --repo Paffin/AgentGit
gh attestation verify oci://ghcr.io/paffin/agentgit-server:X.Y.Z --repo Paffin/AgentGit
```

## Reproducible builds — status

The inputs are pinned (`Cargo.lock` with `--locked`, a fixed Rust image tag in the Dockerfile), absolute paths
are remapped out of the binaries (`--remap-path-prefix`) and symbols are stripped. **Bit-for-bit
reproducibility has not been demonstrated**: nobody has yet built a release twice on different machines and
compared digests, and C dependencies (`ring`, `aws-lc`, bundled SQLite) are compiled by whatever toolchain the
builder has. Until that is done, verify releases through the attestations above, not by rebuilding.

## Things the workflow does not do yet

* It has never run. Expect to fix small things on the first tag (use a `-rc.1` tag first).
* Actions are referenced by major version tag, not by commit SHA; Dependabot keeps them current. Pin them by
  SHA if your threat model includes a compromised action tag.
* No arm64 container image (an emulated Rust build would take hours; build per-architecture on native runners
  and merge the manifests when it matters).
* No signing of Git tags or of the SBOM beyond GitHub's attestations; no publishing to crates.io.
