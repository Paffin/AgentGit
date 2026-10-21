# test-honesty handoff

Goal (ROADMAP 5): a test cannot pass by silently not testing.

State: shipped on main in 51bd78b, 9deabde, a57809b (and the handoff commit after them).

What exists
- One helper per language: `crates/ag-testkit` (Rust, dev-dependency, `publish = false`), `services/agent-orchestrator/internal/testutil/components.go`,
  `scripts/ag_testkit.py` (+ `sdk/python/tests/components.py`, the SDK is published on its own), `apps/web/e2e/support/components.ts`.
  Missing component: FAIL if `AG_REQUIRE_COMPONENTS` names it (or `all`), else skip with an `AG-SKIP` marker, also appended as JSON to `$AG_SKIP_REPORT`.
- `scripts/check_skips.py` (+ `test_check_skips.py`, `test_ag_testkit.py`): reads a job's report, fails on a skip not covered by `tests/skip-allowlist.toml` for that job
  (reason, owner, runs_for_real mandatory); `--scan` fails on any raw skip construct outside the helpers; `--validate` checks the list. Python 3.9 compatible.
- Every CI job that runs tests sets `AG_REQUIRE_COMPONENTS` for what it must have and runs the gate (job names: `ci/test-ubuntu`, `ci/test-macos`, `ci/backends-moto|seaweedfs|ha`,
  `ci/sdk`, `ci/sdk-runtimes-<kind>-<version>`, `ci/orchestrator`, `ci/orchestrator-live`, `ci/shells`, `ci/scripts-py39`, `windows/windows`, `quality/coverage`,
  `oci/<leg>`, `oci/cgroup-v2`, `web/e2e-<project>`, `web/keycloak`).
- Keycloak Rust tests are no longer `#[ignore]`; they run in plain `cargo test` as skips and in `web.yml` identity-provider with `keycloak` required.
- Go: `FindServerBin` also asks `cargo metadata` for the target dir (a shared `~/.cargo/config.toml` target-dir works); no binary prints the exact build command; CI requires `ag-server-binary`.
- `scripts/test_py39_compat.py` + the `scripts-py39` CI job (uv's Python 3.9): `from __future__ import annotations` in the scripts that broke (e2e_smoke, check_docs, check_release, package_release, e2e_observability, examples/agents).
- Docs: `docs/architecture/test-skips.md` (full inventory), `docs/architecture/testing.md`.

Evidence
- OCI runtimes workflow green on 51bd78b with the new gate (docker, podman x2, cgroup); Quality green on 3a20015; Web UI chromium/tablet/phone/webkit e2e green with the gate; the Keycloak
  Rust step green with `keycloak` required; client SDK and shells jobs green; local: Go suite with `AG_REQUIRE_COMPONENTS=ag-server-binary` (5 live skips), `cargo test --workspace` + gate (96 allowed skips).
- Deviation: the brief said setup-python 3.9; its SHA could not be resolved from here, so the job uses the already pinned `astral-sh/setup-uv` to install 3.9.

Not verified / open
- `PostgreSQL + S3 backends` caught itself: it never lifted the AppArmor restriction, so every namespace-sandbox test had been silently skipping there; fixed (sysctl step, `namespace-sandbox` required). On a61336a its moto leg and both gates passed; the SeaweedFS multi-replica step failed once on a port race (`Address already in use`) in `replicas_started_together_migrate_once_and_refuse_a_newer_schema`, a pre-existing flake of that test.
- CI runs of the other commits were often cancelled by concurrent pushes. Seen green with the gate: ci.yml run of 887da45 (all jobs except backends), OCI runtimes, Quality. Windows was not seen to finish with the gate (its step is `python scripts/check_skips.py`, untested on a Windows runner).
- Web UI Keycloak browser job and Firefox e2e were red on 3e77965 for reasons of the UI commit (`No repositories yet` text, a `dev-link` console error), not the skip work.
- The root-only halves of the sandbox tests (`n/a:host-privilege`) run in no CI job.
- Bare `if !cond { return }` in a new Rust test is not detectable by the scan; review aid: `grep -rn "else { return" crates/*/tests`.
- The 4 scripts that read TOML need Python 3.11 (`tomllib`) and are not in the 3.9 job.
