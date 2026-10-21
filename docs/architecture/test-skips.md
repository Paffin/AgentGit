# Test skips

A test that cannot reach the component it tests must not pass. This page says how a test of this repository admits
that a component is missing, what CI does about it, and lists every test that can skip, with the reason and the CI job
that runs it for real. The motivation was an outside validation run: the Go orchestrator suite skipped its integration
tests without `AG_SERVER_BIN`, five live-model tests were skipped, the OCI tests returned early without `AG_OCI_IMAGE`,
and a green Rust run proved nothing about container isolation. A skip that nobody reads is a pass.

## The mechanism

One helper per language, one vocabulary of component names, one environment variable.

| Language | Helper | Used as |
|---|---|---|
| Rust | crate `ag-testkit` (`crates/ag-testkit`, a dev-dependency, never published) | `let Some(url) = ag_testkit::require_here("postgres", "AG_TEST_POSTGRES_URL is not set", postgres_url()) else { return };` |
| Go | `internal/testutil/components.go` | `testutil.RequireComponent(t, "ag-server-binary", err)` |
| Python | `scripts/ag_testkit.py` (and its twin `sdk/python/tests/components.py` for the separately published SDK) | `ag_testkit.require_component("dash", self.id(), shutil.which("dash"), "dash is not installed")` |
| TypeScript | `apps/web/e2e/support/components.ts` | `requireComponent(info, "keycloak", KEYCLOAK ? null : "AG_E2E_KEYCLOAK is not set")` |

When the component is there, nothing happens. When it is missing:

* if `AG_REQUIRE_COMPONENTS` names it (a comma or space separated list, or `all`) the test **fails**, with a message
  that says which component and why;
* otherwise the test **skips** and leaves a marker: one line `AG-SKIP component=<c> test=<t> reason=<r>` as the skip
  message (and on stderr for Rust, because the Rust harness swallows `eprintln!`), and, when `AG_SKIP_REPORT` names a
  file, one JSON object per line appended to it. The report file is what CI reads; it does not depend on how a test
  runner prints skips.

`oci` and `oci:docker` / `oci:podman` cover each other (requiring `oci:docker` makes a missing runtime a failure even
when no runtime was found to name). Two more kinds of marker never fail, but are still reported and reviewed:
`n/a:<kind>` (the test, or half of it, does not apply under this configuration: a root-only branch on a non-root runner,
an S3 clock a test cannot move) and `platform:<os>`.

The older switches `AG_REQUIRE_BACKENDS`, `AG_REQUIRE_SANDBOX`, `AG_REQUIRE_OCI`, `AG_REQUIRE_CGROUP`,
`AG_REQUIRE_SHELLS` and `AG_REQUIRE_DOC_SNIPPETS` still work as aliases of the corresponding components.

### Component names

| Component | What it is | Where it is required |
|---|---|---|
| `postgres`, `postgres:tls` | a PostgreSQL server (`scripts/pg-test.sh`), one with `ssl=on` | CI *backends* |
| `s3`, `s3-tls` | an S3 server (`scripts/s3-test.sh`: moto, SeaweedFS), one that serves TLS with a private CA | *backends* (`s3-tls`: the SeaweedFS legs only) |
| `oci:docker`, `oci:podman` | a container runtime plus the digest-pinned image (`AG_OCI_IMAGE`) | *OCI runtimes* workflow, per leg |
| `cgroup-v2` | a delegated cgroup v2 subtree | *OCI runtimes*, cgroup job |
| `namespace-sandbox` | unprivileged user namespaces | CI *test* on Ubuntu, Quality, the cgroup job |
| `keycloak` | a Keycloak with the test realm | *Web UI*, identity provider job |
| `ag-server-binary` | the built `ag-server` | *test* (both OSes), Quality, the Go orchestrator job |
| `model` | a live language-model endpoint | the live step of the Go orchestrator job |
| `shell:<name>` / `shell` | bash, zsh, fish, pwsh | *shells* job |
| `bash`, `python3`, `git`, `dash`, `mkdocs` | tools the harness drives | *test*, *shells*, packaging, docs |
| `embedded-ui` | an `ag-server` built with the Web UI inside | web end-to-end (also fails on `CI=true`, as before) |
| `source-tree` | the repository around a packaged crate | never skipped in CI |

## The gate

`scripts/check_skips.py` reads the report of a job, prints a table of what skipped and why (also into the job
summary), and **fails the job when a skip is not covered** by `tests/skip-allowlist.toml` for that job. Every entry
there has a component, an optional test pattern, the jobs it is valid in, a **reason**, an **owner** and the CI job
that **runs it for real** (or "none" and why); a missing reason or owner is an error, and so is a reason that says
nothing. The script is standard library only, runs on Python 3.9 and is unit tested (`scripts/test_check_skips.py`).
Every CI job that runs tests runs it: *test* (Ubuntu, macOS), *backends* (moto, SeaweedFS, multi-replica), *client
SDKs*, *agent orchestrator* (and its live step), *shells*, *scripts on Python 3.9*, *Windows*, *Quality* (coverage),
the *OCI runtimes* legs and the cgroup job, and the *Web UI* end-to-end and Keycloak jobs.

The second half of the gate is a source scan, `check_skips.py --scan`, which keeps the mechanism from being bypassed: a
`#[ignore]`, `t.Skip`, `test.skip`, `pytest.skip`, `skipUnless`, `skipTest`, a `testing.Short()` or a printed
"skipping" anywhere outside the helpers fails the *generated docs + demo* job, unless the site is a reviewed `[[native]]`
entry (today: the `scale.rs` measurements). So a new test cannot skip quietly without someone editing the allow-list.

## What skips where

Everything below skips (with a marker) when the component is missing, and fails when the job requires it. "Runs for
real" is the CI job that has the component.

| Tests | Component | Why they can skip | Runs for real |
|---|---|---|---|
| `ag-storage/tests/postgres.rs`, `object_lock.rs`, `ag-storage/src/sqldb.rs`, `ag-integrate/tests/storage.rs`, `ag-cli/tests/cli.rs` (the PostgreSQL part), `ag-server/tests/storage_backends.rs`, `hardening.rs`, `multi_replica.rs`, `ag-server/src/cluster.rs`, and the PostgreSQL legs of `control/dump.rs` and `control/mod.rs` | `postgres` | no server in an ordinary test job | CI *backends* |
| the TLS tests of `ag-storage/tests/postgres.rs` | `postgres:tls` | a server with a private CA | CI *backends* |
| `ag-storage/tests/s3.rs`, `ag-integrate/tests/storage.rs` and `lifecycle.rs` (2 tests), `ag-server/tests/storage_backends.rs`, `multi_replica.rs` | `s3` | no S3 server | CI *backends* (moto, SeaweedFS) |
| `https_works_with_the_private_ca_...` in `ag-storage/tests/s3.rs` | `s3-tls` | moto serves plain HTTP | CI *backends*, SeaweedFS legs |
| the 14 tests of `ag-sandbox/tests/oci_real.rs`, those of `ag-worker/tests/oci_e2e.rs` | `oci:docker`, `oci:podman` | a runtime and an image pulled beforehand | *OCI runtimes* (docker, podman rootful, podman rootless) |
| `ag-sandbox/tests/cgroup_real.rs` | `cgroup-v2` | the subtree must be delegated as root first | *OCI runtimes*, cgroup job |
| `ag-sandbox/tests/isolation.rs`, `limits.rs`, `cgroup_real.rs`, the allocator test in `ag-sandbox/src/spawn.rs`, `ag-worker/tests/e2e.rs` and the profile of every worker test | `namespace-sandbox` | no user namespaces (macOS; some containers); the worker tests fall back to the process sandbox | CI *test* on Ubuntu, Quality, cgroup job |
| `ag-server/tests/oidc_keycloak.rs` (3 tests, formerly `#[ignore]`), `apps/web/e2e/keycloak.spec.ts` (6 tests) | `keycloak` | the Keycloak image is only started by one job | *Web UI*, identity provider job |
| `ag-worker/tests/e2e.rs` `the_real_server_binary_and_the_real_worker_binary_agree`, the Go integration suite (`services/agent-orchestrator/integration`, every test through `testutil.StartServer`) | `ag-server-binary` | no built server | *test*, Quality, *agent orchestrator* |
| the five live tests under `services/agent-orchestrator/internal/{llm,advisor,review,semindex,app}` | `model` | **opt-in by design**: they cost money and need a secret | the live step of *agent orchestrator* on pushes when the key and model are defined |
| `ag-cli/tests/completions.rs` | `shell:bash`, `shell:zsh`, `shell:fish`, `shell:pwsh` | not on every runner image | *shells* |
| `ag-cli/tests/docs_snippets.rs`, `quickstart.rs` (`bash`, `python3`), `portability.rs`, `git_env.rs`, and the pages `docs_snippets.rs` leaves to `git` (`git`) | `bash`, `python3`, `git` | the harness drives them | *test* (required on both OSes) |
| `scripts/test_release_scripts.py` (`install.sh` under dash), `scripts/test_mkdocs_hooks.py` (the real MkDocs build) | `dash`, `mkdocs` | not installed on a laptop | *packaging* tooling job, *docs* site job |
| `apps/web/e2e/embedded.spec.ts` | `embedded-ui` | a binary built without the UI | the web end-to-end jobs build it with the UI |
| `ag-cli/tests/completions.rs` (the shipped copies), `ag-server/tests/ui.rs` (the UI package version) | `source-tree` | a packaged crate has no repository around it | every CI job (full checkout) |

Not applicable (`n/a:`) markers, which never fail but are listed and reviewed:

| Tests | Marker | Why | Runs for real |
|---|---|---|---|
| the root halves of `ag-sandbox/tests/limits.rs` (`a_root_host_maps_...`, `the_task_limit_is_sampled_...`), `process_sandbox.rs` (the task limit, the set-user-ID file), `isolation.rs` (`setuid_programs_cannot_gain_privilege`) | `n/a:host-privilege`, `n/a:host-filesystem` | they need uid 0, or a file system that honours set-user-ID | **none in CI** (runners are not root): they run when the suite is run as root |
| `rewriting_an_old_unreachable_object_saves_it_from_a_concurrent_gc` in `ag-integrate/tests/lifecycle.rs` | `n/a:s3-clock` | S3 objects age on the server's clock | its `_on_s3` counterpart, in *backends* |
| the pages that need `ag-server` in `ag-cli/tests/docs_snippets.rs` | `n/a:doc-pages-needing-ag-server` | the Rust harness runs the CLI only | *docs* workflow, snippets job |
| the Windows variants of `docs_snippets.rs` and `quickstart.rs` | `platform:windows` | they drive bash | Ubuntu and macOS; Windows has the PowerShell demo |
| `test_the_paginate_overloads_cover_exactly_the_paginated_operations` in the Python SDK | `n/a:python-version` | `typing.get_overloads` is Python 3.11+ | *client SDKs* on 3.11 and later |
| `#[ignore = "measurement"]` in the `scale.rs` files of `ag-merge` (2), `ag-tree`, `ag-integrate` | none (a reviewed `[[native]]` entry) | timings, not tests | on request: `cargo test --test scale -- --ignored` |

## What this does not cover

* **A condition the helper is not asked about.** The scan finds skip *constructs*; a bare `if !cond { return; }` in a
  Rust test is not one. The early returns that were there are converted (each is in the table above); a new one is
  caught in review, not by a tool. Searching for `else { return` in `tests/` is the review aid.
* **Compile-time exclusion.** `#[cfg(windows)]`, `#[cfg(target_os = "linux")]` and Cargo features remove a test from the
  build; no marker is written. The Windows and macOS jobs therefore run fewer tests than Ubuntu
  ([Testing](testing.md#what-a-green-build-does-not-tell-you)).
* **Helper processes** such as `helper_host_process_for_the_host_death_test` return unless a marker variable is set;
  they are not tests of their own.
* **Harnesses that already throw.** The TypeScript and Python SDK harnesses and the web end-to-end harness raise an
  error when no `ag-server` is found; they never skipped.
* **The root-only halves** of the sandbox tests run in no CI job (see above).
* A job that never writes a report (no `AG_SKIP_REPORT`, a step that does not run) prints "No test skipped". The
  gate checks what was reported, not that the report was wired.

## Adding a test that needs a component

1. Use the helper of your language, with a component from the table; invent a new name only for a new kind of component
   and add it to the table.
2. If a CI job is meant to run it, put the component into that job's `AG_REQUIRE_COMPONENTS` and set `AG_SKIP_REPORT`.
3. If some job legitimately cannot, add a `[[skip]]` entry to `tests/skip-allowlist.toml` (reason, owner,
   `runs_for_real`), limited to those jobs.
4. Run `python3 scripts/check_skips.py --validate --scan` and `python3 scripts/test_check_skips.py`.

Locally, `AG_REQUIRE_COMPONENTS=all cargo test` shows what your machine cannot run. The Go suite finds the server by
itself in the cargo target directory (even a shared one set in `~/.cargo/config.toml`) and, when there is none, prints
`cargo build -p agentgit-server` with the directory to run it in.
