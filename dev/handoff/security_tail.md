# Handoff: security tail (work package `security_tail`)

The CI gates for the supply chain, static analysis, fuzzing, coverage and mutation testing, the control-database
compatibility fixtures, the bugs they led to, and the documents that say what runs where. Everything below is either
something I saw run (with the link) or says that it was not. Run links are
`https://github.com/Paffin/AgentGit/actions/runs/<id>`.

## What landed on `main`

| Commit | Content |
|---|---|
| `9f35ab9` | `security.yml` (cargo-deny x2, npm audit x2, pip-audit, govulncheck, dependency review, SBOMs, hygiene), `codeql.yml`, `fuzz.yml`, Dependabot for cargo/npm/uv/gomod/docker/docker-compose/github-actions, every action pinned by SHA and `persist-credentials: false`, a `permissions:` block everywhere (`scripts/check_workflows.py --strict`), `security/exceptions.toml` + `scripts/security_exceptions.py` (npm/pip/go/codeql, 92-day expiry), `scripts/check_dependabot.py`, `THIRD_PARTY_LICENSES.md` + freshness check, `scripts/sbom.sh`, the `signed_json` harness fix |
| `5d78bb2` | control-database fixtures written by real old servers (schema 2 from `e5a8146`, schema 3 from `b69669d`), `crates/ag-server/tests/compat_control.rs`, a newer SQLite control database is refused with a message (`newer_schema`) |
| `ec289e7` | two quadratic regular expressions in the TypeScript SDK; the web UI's inline-script check |
| `9c84add` | `quality.yml` (cargo-llvm-cov per crate with floors, cargo-mutants on `ag-object`, `ag-diff`, `ag-tree`, `ag-merge`, `ag-policy`), CodeQL alerts gated through the exception file |
| `81efbbd` | `ag-server` creates its data directory 0700, `operator_hardening.rs` + `docs/operations/hardening.md`, dependency review skips (with a warning) while the dependency graph is off, lower Dependabot limits, `supply-chain.md` |
| `8fe6a33` | tests that kill the mutation survivors of four crates, `.cargo/mutants.toml` |
| `983689c`, `ec72d30`, `148a98c` | what the first CodeQL, coverage and Web UI runs showed: the CodeQL gate's per-language scope, the doc-snippet harness under instrumentation, `--no-fail-fast`, the script-tag regex, prettier |
| `68dbbb6` | a coverage floor for every crate, from the first green run |
| the commit that added this file | `docs/architecture/testing.md`, the Fuzzing section of `security.md`, `SECURITY.md` reconciled, nav and index entries |

## Evidence (what I saw run)

| Gate | Run | Result |
|---|---|---|
| Security | [39113891447](https://github.com/Paffin/AgentGit/actions/runs/39113891447) (`5d78bb2`), [39119646058](https://github.com/Paffin/AgentGit/actions/runs/39119646058) (`ec72d30`), [39120767659](https://github.com/Paffin/AgentGit/actions/runs/39120767659) (`148a98c`) | all jobs green; the scanners ran against the real lock files and the real advisory databases |
| CodeQL (4 languages) | [39113891431](https://github.com/Paffin/AgentGit/actions/runs/39113891431) (`5d78bb2`, before the gate): analysis ran, 10 alerts triaged. [39114581930](https://github.com/Paffin/AgentGit/actions/runs/39114581930) (`9c84add`): the gate **failed** on one real alert (the script-tag regex) and told each language's job to remove the other languages' exceptions. [39119645985](https://github.com/Paffin/AgentGit/actions/runs/39119645985) (`ec72d30`): green in all four languages | the gate works on real SARIF |
| Fuzz | [39113891407](https://github.com/Paffin/AgentGit/actions/runs/39113891407) (`5d78bb2`) and [39119645981](https://github.com/Paffin/AgentGit/actions/runs/39119645981) (`ec72d30`): build of 20 targets with the address sanitizer + four smoke jobs of 60 s per target, green. [39115811126](https://github.com/Paffin/AgentGit/actions/runs/39115811126) (`903499d`): the build job **failed** when another change to the sandbox `Backend` type broke `job_spec` and `policy_eval`; fixed in `28fae3d` | the nightly job has not run |
| Quality (coverage) | [39114581919](https://github.com/Paffin/AgentGit/actions/runs/39114581919) (`9c84add`) failed in the doc-snippet test (instrumented `ag` wrote `default_*.profraw` into the tutorials' directory); [39119646080](https://github.com/Paffin/AgentGit/actions/runs/39119646080) (`ec72d30`, with `--no-fail-fast`) showed the only other failures, two sandbox tests that saw `__LLVM_PROFILE_RT_INIT_ONCE` in the child's environment (fixed in `43fb349` by another change); [39120196135](https://github.com/Paffin/AgentGit/actions/runs/39120196135) (`43fb349`): **green, 79.1 % of 37 285 lines**, table in `docs/architecture/testing.md` | floors for the other twelve crates are first enforced from `68dbbb6` |
| CI (all 10 jobs, incl. PostgreSQL + S3 backends and macOS) | [39120767641](https://github.com/Paffin/AgentGit/actions/runs/39120767641) (`148a98c`) | green; 1 036 Rust tests |
| Containers (the 0700 `/data` in the image), Windows (`operator_hardening`), Web UI, Docs | [39120767666](https://github.com/Paffin/AgentGit/actions/runs/39120767666), [39120767560](https://github.com/Paffin/AgentGit/actions/runs/39120767560), [39120767569](https://github.com/Paffin/AgentGit/actions/runs/39120767569), [39120767644](https://github.com/Paffin/AgentGit/actions/runs/39120767644) (`148a98c`) | green. Web UI was red on `ec72d30` ([39119646011](https://github.com/Paffin/AgentGit/actions/runs/39119646011)): `prettier --check` on my spec; fixed in `148a98c` |

Local, in a 4-core container (not CI): **fuzzing** four campaigns, all 20 targets, 4.3 hours and 130 million
executions, no crash (per-target table in `docs/architecture/security.md#fuzzing`); **mutation testing** all five
crates at 100 % of the mutants that compile, with 24 equivalent mutants listed with reasons (table in `testing.md`);
`govulncheck` with Go 1.24.7 (36 reachable advisories) and 1.27.2 (none), recorded in `scripts/testdata/security/`.

## Found and fixed on the way

| Finding | Found by | Fix |
|---|---|---|
| Two regular expressions in the TypeScript SDK backtracked quadratically on long input (`trimTrailingSlashes`, base64 padding) | CodeQL, first run | loops instead of expressions; tests that time 100 000-character inputs |
| The web UI's inline-script check was case-sensitive, then missed `</script bar>` | CodeQL, first run and the gate's first run | `scriptsOf`, one expression, tested with the odd end tags |
| A control database written by a newer server was opened and failed later with `internal error` | the compatibility fixtures | `newer_schema()`: refused at start-up with both versions named |
| `ag-server` created its data directory with the umask (usually 755) although the control database holds every credential hash | the hardening checklist | created 0700, an existing one with "other" access is called out at start-up; the image's `/data` is 0700 |
| `signed_json` fuzz target split a string inside a multi-byte character; `fuzz/src/agwp_types.rs` was stale | fuzzing | split at a character boundary + seed; regenerated, checked by CI |
| `services/agent-orchestrator` was scanned with Go 1.24 (36 reachable standard-library advisories) | govulncheck | the gate scans with the Dockerfile's Go version (1.27: none) |
| `dependency-review` failed every pull request: the repository's dependency graph is off | the first Dependabot pull requests (11 red runs) | the job asks first (404 from the graph's SBOM endpoint = off), warns and skips |
| The Dependabot configuration opened 18 pull requests at once and queued about 100 workflow runs for an hour | the first Dependabot run | open-pull-request limits lowered |
| `passes()` in `ag-policy` accepted a trusted signed `pass` for another check (as a mutant: no test noticed); `TooLarge` had no test; the type-change, executable-bit, symlink and rename-limit rules were unchecked | cargo-mutants | 92 survivors, each a test (`testing.md`) |

## What is NOT verified

* **The nightly fuzz job** (15 minutes per target, minimised corpus cached between nights) has never run on GitHub;
  it cannot be dispatched from here and its first scheduled run is 02:41 UTC. The smoke jobs look for a cache that
  does not exist until then (the restore falls through to the checked-in seeds).
* **The mutation matrix** of `quality.yml` (weekly, Sunday 03:17 UTC, or by hand) has never run on GitHub. It runs with
  two workers and cargo-mutants' default time-out, not the settings used locally; a mutant that survives fails the job,
  and `.cargo/mutants.toml` has line-anchored entries (`ag-diff/src/lib.rs`, `ag-merge/src/rename.rs`) that stop
  matching when those lines move, which shows up there as survivors to look at.
* **Dependency review** never passed: it is skipped while the dependency graph is off, and the enabled path
  (including the 404 test for "off") has not been seen. **The owner has to switch on** Settings -> Advanced Security ->
  Dependency graph (and "Private vulnerability reporting", whose state I cannot read: `SECURITY.md` says so).
* **CodeQL alerts stay open in the Security tab** until someone dismisses the fixed and accepted ones; the CI gate, not
  the tab, is what I verified.
* **Eighteen Dependabot pull requests** (#6, #10 to #26) opened by the first run are untouched; several upgrade across a
  major version (vitest 5, TypeScript 7, `@types/node` 26, pinia 4) and will not be green. Nobody has decided about them.
* **The floors of the twelve crates added in `68dbbb6`** are enforced for the first time by the Quality run of that
  commit or a later one. `ag-mcp` is at 46.6 % and the reason is not understood (its tests run the binary as a child;
  whether that profile is counted was not looked at).
* **Release SBOM wiring** (`release.yml` calls `scripts/sbom.sh --out dist --no-web`): never run on a tag, none exists;
  the web UI's SBOM is built and checked on every change but is not a release asset.
* **Compatibility fixtures** cover the SQLite control database and the repository metadata; a PostgreSQL-resident
  control database written by an older server is covered by the migration chain's tests, not by a fixture.
* **Fuzzing** is Linux x86_64 with the address sanitizer only; nothing for the HTTP layer of `ag-server`, the CLI
  parser, the web UI, the backends or the isolation itself.
* The test counts of the TypeScript, Python, Go and web suites are not in `testing.md`.

## Next steps

1. Owner: switch on the dependency graph and private vulnerability reporting; dismiss the fixed/accepted CodeQL alerts.
2. Triage the Dependabot pull requests (merge the green ones, `@dependabot ignore` the majors that need work).
3. After the first nightly fuzz and the first weekly mutation run: read both, turn any crash into a regression test,
   re-list moved equivalent mutants in `.cargo/mutants.toml`.
4. Investigate the coverage of `ag-mcp`; run `llvm-cov` also in the PostgreSQL + S3 job so `ag-storage` is measured
   where its backends run; raise floors when coverage rises.
5. Mutation-test the next crates by the same method (`ag-agent`, the authorisation code of `ag-server`).
6. Fuzz what is still untouched: the HTTP request path of `ag-server`, the CLI argument parser.
