# Testing

## Continuous feedback and qualification

Normal PRs and main commits use path-aware fast CI: Linux Rust unit/core-storage contracts, SDK checks against one shared server build when API/SDK files change, Go contracts when the executor changes, and offline workflow checks when tooling changes. Web changes run lint, types, unit tests and a production build; documentation changes build the site. Unrelated changes do not start Rust or cross-platform builds. Unknown paths or unavailable Git history conservatively select all fast components.

The full matrix runs through `qualification.yml` on demand, nightly, and during release preparation: macOS, Windows, Linux coverage (which also executes the full Linux suite), PostgreSQL/S3/HA, all SDK runtimes, CLI examples, all browser engines/viewports, CodeQL, fuzzing, OCI runtimes, packaging and performance checks. SDK/runtime jobs share one verified server/CLI/MCP build. Coverage supplies Linux full-suite correctness, so it is not also run as a separate identical Linux test job.

Stale branch runs are cancelled. Release publication promotes prepared, attested bytes and does not add a third full Rust run. See [Releasing](../operations/releasing.md) for the ready-candidate contract and publication time budget.


What the tests of AgentGit are, which of them run where, how much of the code they reach, how many wrong
versions of the code they notice, and what a green build still does not tell you. The commands are the ones CI runs;
every number below was measured on the date given and is reproducible with the command next to it.

## What is tested, and where it runs

| Layer | What it proves | Run it | Where it runs in CI |
|---|---|---|---|
| Unit and property tests | one function or type; invariants over generated inputs (`proptest`: determinism, round trips, "applying the diff gives the target") | `cargo test --workspace` | [CI](https://github.com/Paffin/AgentGit/blob/main/.github/workflows/ci.yml) *test*, Ubuntu and macOS; [Windows](../operations/windows.md) for the CLI, client and server |
| Crate integration suites | a crate through its public API: the sandbox against the real kernel (`AG_REQUIRE_COMPONENTS=namespace-sandbox`), the engine against a real store, the policy cookbook against the real parser | the same | the same |
| Cross-implementation conformance | the byte formats against an independent encoder written in Python (`tests/conformance/`) | the same | the same |
| Compatibility | data written by **older builds** still opens and means the same ([Compatibility](compatibility.md)) | `cargo test -p agentgit-integrate --test compat; cargo test -p agentgit-server --test compat_control` | the same |
| Backends | every suite on PostgreSQL 16 and on S3 objects (moto, and SeaweedFS, a real S3 server) | `scripts/pg-test.sh scripts/s3-test.sh cargo test --workspace` | CI *PostgreSQL + S3 backends* |
| End to end | the README demo, every command of the tutorials, the docs against the code | `examples/quickstart.sh`; `python3 scripts/check_docs.py` | CI *generated docs + demo*, *shells* |
| Clients | the TypeScript and Python SDKs and the Go orchestrator against a **real** `ag-server`; the web UI in a real browser | `npm test` in `sdk/typescript`, `pytest` in `sdk/python`, `go test ./...` | CI *client SDKs*, *agent orchestrator*; [Web UI](https://github.com/Paffin/AgentGit/blob/main/.github/workflows/web.yml) |
| Images and manifests | the images build and start, the Compose stack serves, the Kubernetes manifests validate | - | [Containers](https://github.com/Paffin/AgentGit/blob/main/.github/workflows/containers.yml) and CI *deployment files* (path-filtered) |
| Supply chain | advisories, licences, pinned actions, CodeQL ([Supply chain](../operations/supply-chain.md)) | - | [Security](https://github.com/Paffin/AgentGit/blob/main/.github/workflows/security.yml), [CodeQL](https://github.com/Paffin/AgentGit/blob/main/.github/workflows/codeql.yml) |
| Fuzzing | the parsers and decoders against hostile bytes ([Fuzzing](security.md#fuzzing)) | `fuzz/run.sh 300` | [Fuzz](https://github.com/Paffin/AgentGit/blob/main/.github/workflows/fuzz.yml): 60 s per target on changes, 15 minutes nightly |
| Coverage and mutation | how much of the code the tests reach, and whether they notice a wrong version of it (below) | `cargo llvm-cov --workspace`; `cargo mutants -p agentgit-merge` | [Quality](https://github.com/Paffin/AgentGit/blob/main/.github/workflows/quality.yml): coverage on changes and weekly, mutation weekly and by hand |

**How many.** `cargo test --workspace` ran **1 036 tests** (and ignored 3, which are measurements run on request, not
tests) in the CI *test* job on Ubuntu for commit `148a98c`
([run 39120767641](https://github.com/Paffin/AgentGit/actions/runs/39120767641)); the same suite runs on macOS. Count
them with `cargo test --workspace -- --list`. The TypeScript, Python and Go suites, the web UI's unit and end-to-end
tests and the tests of the scripts are counted by their own runners and are not in that number.

The scripts under `scripts/` that guard the repository have tests of their own, and those tests use **recorded output
of the real tools** (`npm audit`, `pip-audit`, `govulncheck`, `cargo llvm-cov`): `scripts/testdata/`. They run in the
Security and Quality workflows.

## Web UI feedback time

The unit suite keeps each file isolated in its own VM and limits itself to four workers (or the available CPU count, if smaller), with a 512 MB recycling limit per worker. Pure API and data tests use Node; component, browser storage and DOM tests keep happy-dom. Timers and mocks are restored after each test. Mutation workers materialize project source files while sharing dependencies, so a relative import cannot escape to an unmutated source file through a symlink. Changing the worker pool must preserve the complete test list, not only the final pass count; shuffled runs check for order dependencies.

A local comparison on 2026-10-27 used Node 24.19.0, two fixed CPU cores and the same 478 existing tests. Three baseline runs took 30.358 / 27.569 / 33.328 seconds; the isolated-VM/Node configuration took 6.050 / 5.856 / 5.660 seconds. Median wall time fell from 30.358 to 5.856 seconds (80.7%). Test names and outcomes matched in all six runs. This is a unit-suite measurement, not a claim about total CI or browser time. Three additional tests cover immediate heartbeat-worker shutdown, stopping before the first interval is scheduled, and the absence of requests after shutdown.

The browser pipeline builds the checked production UI and embedded server once, preserves executable modes in an archive, and checks binary hashes after download. Chromium, Firefox, WebKit, phone, tablet and documentation screenshots then run as separate projects against that same build. The browser image is pinned by version and digest and must match the Playwright package. The embedded-server check requests every module preload as well as the entry script and CSS, so generated names with multiple dots cannot silently lose immutable caching. Screenshot scenarios run once; they are not repeated as a dependency in three matrix jobs. The real Keycloak job remains a separate check. Its browser tests use the same version- and digest-pinned Playwright image, with host networking to reach the real provider and dynamically started server. Rust token-validation tests still run on the host. The browser container runs as the runner user and writes its reviewed-skip report into the mounted evidence directory; it does not reinstall Ubuntu browser packages. Lazy imports keep their dependency and CSS preloads; the native import loads its own entry without a duplicate hint. The resolver participates in chunk hashing so a preload-policy change cannot reuse an immutable URL with changed bytes.

For a focused local browser iteration after building the current UI, run `npx playwright test --project=chromium --no-deps e2e/draft-recovery.spec.ts` in `apps/web`. The ordinary full run still includes the screenshot project. Do not use a focused run as evidence that the whole matrix passed. Console, crash, authorization and reviewed-skip gates apply in both modes.

## A test cannot pass by not running

A test that needs an external component (PostgreSQL, S3, a container runtime, Keycloak, the `ag-server` binary, a
model, a shell) asks one helper per language (`ag-testkit` for Rust, `internal/testutil` for Go, `scripts/ag_testkit.py`,
`apps/web/e2e/support/components.ts`) instead of returning early. A missing component **fails** the test when the job
sets `AG_REQUIRE_COMPONENTS` for it, and otherwise skips it with a machine-readable `AG-SKIP` marker. Every CI job
that runs tests collects the markers (`AG_SKIP_REPORT`), prints a table of what skipped and why in its summary, and
**fails when a skip is not on the reviewed list** `tests/skip-allowlist.toml` (reason, owner and the job that runs it for
real, per entry; live-model tests are there because they are opt-in by design). A source scan fails the build on any
skip construct outside the helpers. The mechanism, the component names, the full list of skipping tests and what the
gate does not cover are on [Test skips](test-skips.md).

The jobs and what each requires: *test* (Ubuntu: `namespace-sandbox`, `git`, `bash`, `python3`, `ag-server-binary`; macOS the
same without the sandbox), *backends* (`postgres`, `s3`, and `namespace-sandbox` on the moto leg, which runs the whole workspace; `s3-tls` on SeaweedFS), the *OCI runtimes* legs (`oci:docker`
or `oci:podman`; the cgroup job `cgroup-v2`, `namespace-sandbox`), *Web UI* identity provider (`keycloak`), *shells*
(`shell`), the Go orchestrator job (`ag-server-binary`; the live step adds `model`). What still skips in each job is
listed in the allow-list, not assumed. The root-only halves of the sandbox tests run in no CI job.

The lightweight scripts also run on Python 3.9 (*scripts on Python 3.9*, `scripts/test_py39_compat.py` plus the unit
tests of the scripts); the scripts that read TOML need 3.11 (`tomllib`).

## Coverage

`cargo llvm-cov --workspace` runs the whole test suite once with instrumentation. The
[Quality](https://github.com/Paffin/AgentGit/blob/main/.github/workflows/quality.yml) workflow prints a table per
crate in the job summary (`scripts/coverage_summary.py`), uploads the `coverage.json` and `lcov.info` files as an
artifact, and fails when a crate falls under its **floor** in
[`scripts/coverage_floors.toml`](https://github.com/Paffin/AgentGit/blob/main/scripts/coverage_floors.toml).

A floor is not a target. It sits about five points under what the suite reaches, rounded down to a multiple of five,
so that deleting tests or landing a large untested module fails the build while an ordinary change does not.

The numbers below are the first green run of the workflow on `main`
([run 39120196135](https://github.com/Paffin/AgentGit/actions/runs/39120196135), commit `43fb349`, 2026-10-18, Linux, the
whole workspace test suite with the namespace sandbox and the documentation snippets required to run): **79.1 % of
37 285 lines**. That run checked the floors of the nine core crates as they were then (all held); the floors in the
table were set from its numbers and are checked from the next run on.

| Crate | Lines | Floor | Crate | Lines | Floor |
|---|---:|---:|---|---:|---:|
| `ag-object` | 100.0 % | 95 | `ag-server` | 86.1 % | 80 |
| `ag-merge` | 98.9 % | 90 | `ag-protocol` | 86.3 % | 80 |
| `ag-policy` | 98.9 % | 90 | `ag-integrate` | 87.5 % | 80 |
| `ag-tree` | 98.6 % | 90 | `ag-client` | 87.8 % | 80 |
| `ag-diff` | 97.9 % | 90 | `ag-agent` | 89.0 % | 80 |
| `ag-dag` | 95.0 % | 90 | `ag-cli` | 85.0 % | 80 |
| `ag-crypto` | 93.9 % | 85 | `ag-sandbox` | 81.8 % | 75 |
| `ag-changeset` | 93.6 % | 85 | `ag-worker` | 74.2 % | 65 |
| `ag-types` | 90.6 % | 85 | `ag-storage` | 73.6 % | 65 |
| `ag-git` | 90.0 % | 85 | `ag-mcp` | 46.6 % | 40 |

(`ag-ui-assets` is four lines of generated glue and has no floor.) The deterministic core, where a silent bug corrupts
data, is `ag-object`, `ag-merge`, `ag-policy`, `ag-tree`, `ag-diff`, `ag-dag`, `ag-crypto`, `ag-changeset` and `ag-types`;
the floors of most of them were raised because the tests that the mutation runs below led to raised the coverage.
What the numbers say and do not say:

* **Line coverage says that a line ran, not that anything checked its result;** that is what the next section is for.
* **`ag-mcp` is the weak spot.** Most of its 5 129 lines are tool handlers that its tests reach by running the
  `ag-mcp` binary as a child process; whether the profile of that child is fully counted was not investigated, so the
  figure is a lower bound of unknown looseness. Its floor records where it is, not where it should be.
* `ag-storage` and `ag-worker` are lower because their PostgreSQL, S3 and container paths run only in the jobs that
  have those services, not in this one.
* Rust only: the TypeScript, Python and Go code has no coverage number.

## Mutation testing

Coverage cannot tell a test that checks from a test that merely runs the code. [cargo-mutants](https://mutants.rs)
can: it replaces a `+` with `-`, negates a condition, or replaces a function body with a constant, and runs the
crate's tests. A change the tests do not notice (a *survivor*) is either a missing test or a change that cannot
alter the behaviour (an *equivalent mutant*).

It is run on the five crates whose bugs are silent: `ag-object` (the byte formats), `ag-diff`, `ag-tree`, `ag-merge`
and `ag-policy` (the gate in front of every integration). Locally: `cargo mutants -p agentgit-merge`; in CI the
[Quality](https://github.com/Paffin/AgentGit/blob/main/.github/workflows/quality.yml) workflow runs the five weekly
and by hand (it takes an hour or more; not on pull requests) and **fails when a mutant survives**.

**Results** (cargo-mutants 27.1.0, one worker, the tree of 2026-10-18; "caught" includes the mutants that made a test
loop until the time-out, which is how a test notices an infinite loop):

| Crate | Mutants | Caught | Do not compile | Equivalent, listed in `.cargo/mutants.toml` | Survivors the first runs found |
|---|---:|---:|---:|---:|---:|
| `ag-object` | 90 | 83 | 6 | 1 | 13 |
| `ag-diff` | 210 | 190 | 2 | 18 | 25 |
| `ag-tree` | 44 | 34 | 9 | 1 | 0 |
| `ag-merge` | 163 | 147 | 12 | 4 | 35 |
| `ag-policy` | 135 | 131 | 4 | 0 | 19 |

Every mutant that compiles is caught, except the 24 that cannot change what the code does (an early return that is
only a shortcut, a loop the caller makes unreachable, a bound that never binds, a guard that repeats what the general
rule does), which are listed, each with its reason, in
[`.cargo/mutants.toml`](https://github.com/Paffin/AgentGit/blob/main/.cargo/mutants.toml). The 92 survivors of the first
runs were missing tests, and each got one. A few worth naming, because they are what coverage alone would not have
found:

* **`ag-policy`**: a trusted, correctly signed `pass` for *another check* satisfied a required check (a `||` turned
  into `&&` in `passes` went unnoticed: no test had a valid attestation for the wrong check). Also the issuer list of
  a validation, both edges of the clock-skew and age limits, and what an envelope with no room or no paths contains.
* **`ag-merge`**: the 16 MiB limit for text merges (`TooLarge`) had no test at all; neither had a symlink never being
  renamed, the 200-file and 256 KiB limits and the 60 percent threshold of the rename search, the type-change rule when
  only one of the three sides is a link, the executable bit when only one side changed it, and the markers of a
  conflict in a file without a final newline.
* **`ag-object`**: the fixed-width reads and writes and the exact edge of the bounded lengths.
* **`ag-diff`**: every pair of sequences of up to five elements over three letters is now diffed and checked to be
  correct and minimal (364 x 364 pairs), and the exact text of unified diffs for ranges, context grouping and
  missing final newlines.

**What this does not show.** The mutation score of five crates; the server, the engine, the CLI and the storage have
none. The CI job (weekly, or by hand) **has not run on GitHub yet**: the numbers above are from a 4-core container,
and the job's own settings (two workers, the default time-out) differ from the ones used here. A mutant that the tests
catch only by timing out is counted as caught, so a test that hangs is not told apart from one that asserts. And a
listed mutant is a judgement: read the reason before trusting it.

## Test doubles

Real components are used wherever a test can have them: the SDKs and the web UI's end-to-end suite talk to a real
`ag-server`, the sandbox tests run in a real namespace, the backends are a real PostgreSQL and a real S3 server. What
remains is listed here, each with the reason it cannot be real, and exists only in test code:

| Where | What it stands in for | Why it cannot be real |
|---|---|---|
| `crates/ag-merge/src/tests.rs`, `tests/scale.rs` (`Mem`) and a counting wrapper | the blob store `ag-merge` reads files through | `ag-merge` is pure and does not depend on a store; the real one is used by the `ag-integrate` suites that call it |
| `crates/ag-storage/src/s3/tests.rs` | an S3 server that answers `503`, drops the connection or sends a damaged listing exactly when the test says | a real server cannot be made to fail on demand; the real ones are in the backends job |
| `crates/ag-sandbox/tests/oci.rs`, `src/cgroup.rs` | a container runtime (a script that records its calls) and a cgroup directory | the arguments and the clean-up are checked against a script; whether a real runtime accepts them is checked where one exists ([The sandbox](sandbox.md)) |
| `crates/ag-server/tests/tracing_export.rs` | an OTLP/HTTP collector | the exporter is tested against a local receiver, not a vendor |
| `crates/ag-server/tests/ui.rs` (`fake_dist`) | the built web UI bundle | the server's static-file tests must not need Node |
| `apps/web/e2e/support/idp.ts` | an OpenID Connect identity provider | no real one is available to a test run; it listens on loopback for one run |
| `services/agent-orchestrator/internal/testutil/fakeagwp` | `ag-server`, for the orchestrator's unit tests | to inject rate limits and dropped streams; the orchestrator's integration suite runs against the real server |
| `sdk/typescript/test/harness/faults.ts`, `sdk/python/tests/faults.py` | the network between an SDK and the (real) server, made to lose a response, refuse a connection or answer `429`/`5xx` | a real server cannot be made to fail on demand; every request that is not faulted reaches the real server |
| `scripts/test_security_exceptions.py` (`sarif_dir`) | the SARIF file CodeQL writes | CodeQL cannot run inside a unit test; the gate runs on the real file in the CodeQL workflow |
| the engine's `Failpoints` (product code, off unless a test turns it on) | a crash at a chosen step | recovery after a crash has to be provoked at an exact point |

## What a green build does not tell you

* **Tests are not proof.** The suite is large and has found real bugs ([Fuzzing](security.md#fuzzing) lists them), and
  the mutation runs above show where it was weak and was fixed; nothing here shows the absence of bugs.
* **Coverage is measured for Rust only**; the TypeScript, Python and Go code is tested but has no coverage number.
  Mutation testing covers five crates, not the server, the engine or the CLI.
* **Not run on every push:** full qualification, fuzzing, mutation testing, cross-platform integration and CodeQL. A change
  can be merged between two of them.
* **Platforms:** Linux and macOS run the whole suite; Windows runs the CLI, client and server ([Windows](../operations/windows.md)).
  The namespace sandbox is Linux-only. Real Docker and Podman runtimes are exercised only where
  [The sandbox](sandbox.md) says so.
* **Backends:** PostgreSQL 16, moto and SeaweedFS ([PostgreSQL and S3](../operations/postgres-and-s3.md#honest-limits)).
  Amazon S3, MinIO and other PostgreSQL versions are not tested.
* **No load or soak test** of the server, and no external audit.
