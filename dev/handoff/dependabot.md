# Dependabot triage (2026-10-19)

Rule applied: a PR is merged only when its CI is green on a base that matches `main` (Dependabot rebases on request or
on conflict); majors that need work are done by hand on `main` and the PR is closed with a link; the rest is closed with
a reason and an ignore rule in `.github/dependabot.yml`. Two jobs were red on **every** PR at first and are not PR
evidence: *Security / dependency review* ("Dependency graph is not enabled", a repository setting, since fixed) and the
macOS/Windows `Test` jobs of PRs opened before the flake fixes. Evidence below is from
`api.github.com/repos/Paffin/AgentGit/actions/runs?head_sha=<sha>`.

| PR | Bump | Decision | Evidence |
|---|---|---|---|
| #26 | docker/build-push-action 6.19.2 -> 7.4.0 | merged `d60205f` | CI, Containers, Packaging, Docs, Windows green on the PR; Containers green on main afterwards |
| #25 | actions/attest-build-provenance 2.4.0 -> 4.2.2 | merged `dbb7f6d` | release.yml only (publishing is inert unless dispatched); CI, Packaging green |
| #23 | docker/login-action 3.7.0 -> 4.6.0 | merged `7c6b192` | release.yml only; CI, Packaging green |
| #22 | docker/setup-buildx-action 3.12.0 -> 4.4.1 | migrated by hand `12c5812`, closed | PR's red "image layer cache" job was a stale-base failure; Containers green with buildx 4.4.1 at `813b250` |
| #24 | actions/setup-node 4.4.0 -> 7.1.0 | migrated by hand `12c5812`, closed | PR red on stale base ("No stubs" gate, Windows test); Web UI, Packaging, Docs green on main with it |
| #6 | grafana 11.5.2 -> 13.2.4 | migrated by hand `17ecf1d`, closed | Containers (compose + `scripts/e2e_observability.py`: Prometheus, Grafana, collector) green at `813b250`; provisioning and dashboard load unchanged |
| #10 | otel-collector-contrib 0.121.0 -> 0.162.0-386 | migrated by hand `17ecf1d` as `0.162.0`, closed | `-386` is the 32-bit-only tag; multi-arch `0.162.0` used; same Containers run |
| #11 | prometheus v3.2.1 -> v3.15.0 | migrated by hand `17ecf1d`, closed | same Containers run |
| #12 | postgres 16-alpine -> 18-alpine | closed, ignore rule | suites are verified on 16 only; the 18 image stores data under `/var/lib/postgresql/18/`, an existing volume needs pg_upgrade or dump/restore. `ignore: postgres semver-major` |
| #18, #19 | typescript 5.9.3 -> 7.0.2 (web, sdk) | closed, ignore rule | `npm ci` fails ERESOLVE: openapi-typescript 7.13 peers `typescript ^5.x` |
| #16, #17 | @types/node 22 -> 26 (web, sdk) | closed, ignore rule | runtime/CI are Node 22 (`engines >=22.18`); types follow the runtime |
| #15 | tsup 8.5.1 -> 9.0.0 (sdk) | merged `8d1f27c` | CI (client SDKs) and Packaging (SDK packages) green |
| #21 | hatchling 1.27.0 -> 1.32.4 (python sdk) | merged `c916d42` | all workflows green on the rebased head |
| #31 | actions/upload-pages-artifact 3.0.1 -> 5.0.0 | merged `51ed3e1` | docs.yml only; Docs, CI green (deploys only from main, not exercised) |
| #30 | actions/configure-pages 5.0.0 -> 6.0.0 | merged `790e2c6` | same |
| #28 | helm/kind-action 1.14.0 -> 1.15.1 | merged `e924b97` | kind job green on the PR; Containers green on main |
| #32 | crazy-max/ghaction-github-runtime 3.1.0 -> 4.0.0 | merged `07f908d` | Containers green on main after #28; PR's own red job was the same cache-test flake |
| #20 | vitest 3.2.7 -> 5.0.4 (web) | merged `7ae5771` | Web UI, CI green on the PR and on main |

Open items are recorded at the end of the run in the final report (see the PR list on GitHub).

Notes
* Local `npm install vitest@5` fails with an arborist bug in node 22's bundled npm (`edgesOut`); the PR's lockfile,
  produced by Dependabot, installs fine in CI.
* The *image layer cache* job of Containers is red on some Dependabot PRs and green on others with identical content
  (26 green, 28/32 red); on `main` it is green. Treat it as cache-state dependent for pull requests.
