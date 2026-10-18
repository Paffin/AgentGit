# Supply chain and vulnerability gates

What AgentGit checks about the code it depends on, where each check runs, how to reproduce it on a laptop, and what
is *not* covered. The workflow is [`.github/workflows/security.yml`](https://github.com/Paffin/AgentGit/blob/main/.github/workflows/security.yml);
it runs on every push to `main`, on every pull request and once a day (the advisory databases change without any
commit of ours), and can be started by hand.

## The gates

| Job | Checks | Fails when | Reproduce locally |
|---|---|---|---|
| `cargo-deny` (2 manifests) | RustSec advisories (also *yanked*, *unmaintained*, *unsound*), licences, duplicate and wildcard dependencies, registries. The workspace and the fuzz harness (`fuzz/`, a workspace of its own) | a dependency has an advisory not accepted in [`security/exceptions.toml`](https://github.com/Paffin/AgentGit/blob/main/security/exceptions.toml), a licence outside `deny.toml`'s allow list, a second version of a crate that `deny.toml` does not explain, or any source but crates.io | `cargo install cargo-deny --locked && cargo deny check` (and `--manifest-path fuzz/Cargo.toml`) |
| `npm audit` (`apps/web`, `sdk/typescript`) | production dependencies, then all dependencies | an advisory of severity **high** or **critical** that is not accepted. Lower severities are printed, not failed | `npm audit --json > r.json; python3 scripts/security_exceptions.py npm r.json` |
| `pip-audit` (`sdk/python`) | the locked runtime dependencies (`uv export --frozen --no-dev`) | **any** advisory not accepted: pip-audit reports no severity | `uv export --frozen --no-dev --no-emit-project --no-hashes -o r.txt && uvx pip-audit -r r.txt --no-deps --disable-pip --format json -o p.json; python3 scripts/security_exceptions.py pip p.json` |
| `govulncheck` (`services/agent-orchestrator`) | the orchestrator, with the Go version **of its Dockerfile** (`FROM golang:X`), not the one in `go.mod` | a vulnerability whose vulnerable *function our code can reach* and that is not accepted. Vulnerabilities in a package we merely import are counted and printed | `go install golang.org/x/vuln/cmd/govulncheck@v1.9.0; govulncheck -format json ./... > g.json; python3 scripts/security_exceptions.py go g.json` |
| `dependency review` | the dependency changes a pull request makes, with GitHub's advisory database | a change adds a dependency with a **high** advisory. Pull requests only, and only while the repository's [dependency graph](#not-covered) is on (it is off today: the job warns and checks nothing) | - |
| `SBOMs build` | `scripts/sbom.sh` produces the CycloneDX 1.5 documents the release publishes, and the working tree stays clean | the tool fails, or a document lacks the components that must be in it | `scripts/sbom.sh --out /tmp/sbom` |
| `workflows, exceptions, licence file` | [`scripts/check_workflows.py --strict`](#workflows), [`scripts/security_exceptions.py check`](#accepting-a-finding), the tests of those scripts, and that `THIRD_PARTY_LICENSES.md` matches the lock files | an unpinned action, a workflow without `permissions:`, a risky trigger, an expression injected into a shell script, an expired or malformed exception, a stale licence file | `python3 scripts/check_workflows.py --strict; python3 scripts/security_exceptions.py check; scripts/third_party_licenses.sh --check` |

A scanner that crashed or could not reach its database must never look like a clean scan: `security_exceptions.py`
exits 2 on an empty, missing or malformed report (an `npm audit` registry error, a `govulncheck` stream without its
`config` message), and the job fails.

The Go version matters. `go.mod` says `go 1.24`, the image is built with `golang:1.27`, and the standard library the
binary contains is the image's. govulncheck v1.9.0 reports **36 reachable standard-library vulnerabilities** with
go1.24.7 and none with go1.27.2 (recorded in `scripts/testdata/security/`), so the job scans with the Dockerfile's
version.

## Accepting a finding

A finding that cannot be fixed now is listed in [`security/exceptions.toml`](https://github.com/Paffin/AgentGit/blob/main/security/exceptions.toml)
and nowhere else. Each `[[exception]]` carries the advisory id, ecosystem (`rust`, `npm`, `python`, `go`), package,
scope, a reason that says why it is acceptable and what would change that, an owner, `added` and `expires`.

* An entry lives **at most 92 days** (`expires - added`). To keep it, renew it: a new `added`, a new reason. It is
  deliberately not possible to extend it quietly.
* An expired entry fails `security_exceptions.py check` *and* stops covering its finding, so the scan fails again.
* An entry covers a finding only when ecosystem, package and advisory id (or one of its aliases: a `GHSA-` entry
  covers the `PYSEC-` id pip-audit reports) all match.
* Rust advisories are also listed in `deny.toml`'s `[advisories] ignore`, because cargo-deny enforces them itself;
  `security_exceptions.py check` fails when the two lists disagree.
* An entry that no scan reports any more is printed as a notice: delete it.

Today the file accepts two `tinypool` advisories (`GHSA-5gmw-xhrv-c9v3`, `GHSA-85c8-ppgw-ccpr`), a dependency of
`vitest`, which is a development tool of `apps/web` and `sdk/typescript` and part of no shipped bundle. They expire
on 2027-01-15; the fix is vitest 5, a breaking upgrade. It also accepts seven [CodeQL](#static-analysis-codeql) alerts,
all in test tooling, each read before it was accepted (expiry 2027-01-16).

The scripts are tested against **recorded output of the real scanners** (`scripts/testdata/security/`, see its
README): `python3 scripts/test_security_exceptions.py`.

## Workflows

`scripts/check_workflows.py` holds every file in `.github/workflows/` to four rules, offline:

* every `uses:` is pinned to a **full commit SHA** with the release in a comment (`actions/checkout@3d3c42e... # v7.0.1`);
  a tag can be moved by whoever owns the action. `scripts/check_workflows.py --pin` rewrites tags to SHAs (it
  resolves them with `git ls-remote`); Dependabot's `github-actions` ecosystem then updates SHA and comment together;
* a top-level `permissions:` block (or one on every job), so the repository default token is never used. Every
  workflow asks for `contents: read` at the top; write scopes sit on single jobs (`pages`/`id-token` on the docs
  deploy, `id-token` on the two SDK publish jobs, `packages`/`attestations`/`contents: write` on three release jobs);
* no `pull_request_target` and no `workflow_run` trigger (they run with secrets on code a pull request controls);
* no expression an outsider controls (`github.event.pull_request.title`, `head_ref`, `inputs.*`) inside a `run:` script
  - it goes through `env:`.

`actions/checkout` is always `persist-credentials: false`, so the job token is not left in `.git/config` for later
steps and uploaded artifacts to find (`--strict` enforces it). `dtolnay/rust-toolchain` has no releases to pin to: it
is pinned to a commit of `master` and selects the toolchain with its `toolchain:` input, which is what `@stable` did.

## Static analysis (CodeQL)

[`.github/workflows/codeql.yml`](https://github.com/Paffin/AgentGit/blob/main/.github/workflows/codeql.yml) analyses
the four languages of the repository (Rust, JavaScript/TypeScript, Python, Go) with the `security-extended` query suite
on every push to `main`, every pull request and once a week (the queries change without any commit of ours). Rust,
TypeScript and Python are analysed without a build; the Go module is built by hand because it sits in a
subdirectory. Generated files are excluded (`.github/codeql/codeql-config.yml`), tests are not.

* Results go to *Security -> Code scanning* **and** into the job log (`scripts/codeql_summary.py`: alerts per
  language, rule and severity), so a run can be read without the Security tab.
* The job **fails on a medium or worse alert** that `security/exceptions.toml` does not accept
  (`scripts/security_exceptions.py codeql sarif-results --fail-on medium`; the exception's `id` is the rule, its
  `package` the file). The first run found ten alerts: two real quadratic-backtracking regular expressions in the
  TypeScript SDK (fixed, with regression tests that time 100 000-character inputs), one weak pattern in the web
  UI's inline-script test (fixed), and the seven in the file above.
* A CodeQL alert that is fixed or accepted here still shows as **open in the Security tab** until someone with access
  dismisses it there; the workflow cannot do that. The CI gate, not the tab, is what this repository relies on.

## Dependency updates (Dependabot)

[`.github/dependabot.yml`](https://github.com/Paffin/AgentGit/blob/main/.github/dependabot.yml) covers every
ecosystem the repository has a manifest for: `cargo` (the workspace and `fuzz/`), `npm` (`apps/web`,
`sdk/typescript`), `uv` (`sdk/python`), `gomod` and `docker` (`services/agent-orchestrator`), `docker` (`deploy/docker`),
`docker-compose` (`deploy/compose`) and `github-actions`. Minor and patch updates of one directory arrive as one
pull request, major updates one by one. `scripts/check_dependabot.py` (run by the Security workflow) fails when a
manifest is not covered by an entry of its ecosystem, or an entry names a directory that does not exist. A
Dependabot pull request is a proposal: the
Security, CI and CodeQL runs on it are the evidence, a maintainer merges.

Two deliberate gaps: `docs/requirements.txt` is hash-locked by `uv pip compile` and refreshed by hand (a `pip`
update would rewrite versions without their hashes), and the toolchain pins in workflows (`toolchain: 1.89.0`,
`FUZZ_NIGHTLY`) are selected by an input, not a `uses:` ref, so Dependabot leaves them alone.

The open-pull-request limits are low on purpose (5 for the workspace, 1 for `fuzz/`, 2 or 3 for each JavaScript,
Python and Go directory): the first run over a repository whose dependencies had never been updated opened eighteen
pull requests at once, each of which starts every workflow its paths touch, and the roughly hundred runs that queued
delayed the checks of everything else.

## Licences

`deny.toml` is the policy: Apache-2.0-compatible permissive licences only (the allow list is in the file, with the
reason for each exception). [`THIRD_PARTY_LICENSES.md`](https://github.com/Paffin/AgentGit/blob/main/THIRD_PARTY_LICENSES.md)
is the notice for what the binaries and bundles we distribute contain: the Rust crates of `Cargo.lock` for the release
targets (cargo-about, no dev- or build-dependencies), the production packages of `apps/web` (embedded in
`ag-server`), and the Go modules and standard library of the orchestrator. `scripts/third_party_licenses.sh` writes
it, `--check` (run by CI) fails when it differs from what the lock files produce. A package without a licence text
fails the script; the one npm package that declares its licence in `package.json` and ships no file (`@vue/devtools-api`
6.6.4) is listed with that declaration instead of an invented text. Not covered: the operating-system layers of the
container images, which carry their own notices.

## SBOMs

`scripts/sbom.sh` writes a CycloneDX 1.5 document per shipped binary (`ag`, `ag-server`, `ag-worker`, from
`Cargo.lock` by cargo-cyclonedx 0.5.9) and one for the web UI (`scripts/sbom_npm.py`, the production packages of
`package-lock.json`; `npm sbom --omit dev` lists 13 of the 37). The release workflow calls it for the three binary
documents, which become release assets with a build-provenance attestation ([Releasing](releasing.md)); the web UI one
is checked on every change but is **not yet a release asset**.

## Not covered

* **`dependency review` is not checking anything today.** It needs the repository's *dependency graph*, a setting
  that only the owner can switch on (Settings -> Advanced Security -> Dependency graph). With it off, the action
  failed every pull request it ran on ("Dependency review is not supported on this repository"; eleven Dependabot
  pull requests, seen in the first run), so the job asks GitHub first (the graph's SBOM endpoint answers 404 while it
  is off), prints a warning and skips the action; once the graph is on it judges every pull request by itself.
  That the action then passes has not been seen. It also only ever runs on pull requests, and changes land on
  `main` directly.
* The same switch decides whether GitHub's own *Dependabot alerts* and the *Security* tab show anything; this
  repository's gates (the table above) do not depend on it.
* Container base images and the packages inside them are not scanned for vulnerabilities by a CI job here;
  Dependabot proposes base-image updates (`.github/dependabot.yml`).
* The Python SDK is audited on its locked *runtime* dependencies; build and test tooling is not.
* There is no reproducible-build proof: the release binaries are attested to the workflow that built them, not
  bit-for-bit rebuilt.
