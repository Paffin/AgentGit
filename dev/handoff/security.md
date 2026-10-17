# Security, supply chain and quality gates - handoff

State at the checkpoint. Everything below is either on `main` (marked **main**) or on branch
`wip/security-track` (marked **wip**). Nothing is claimed as working that was not run; unverified items say so.

## Fuzzing (`fuzz/`, **main**)

A stand-alone cargo-fuzz workspace (`fuzz/Cargo.toml`, nightly, libFuzzer) with 20 targets:

| light (builds in ~1 min) | heavy (`--features heavy`: SQLite, PostgreSQL, gRPC) |
|---|---|
| object_envelope, canon_reader, changeset_decode, tree_decode, tree_ops, repo_path, diff_myers, merge_text, merge_trees, rename_detect, policy_eval, policy_json, glob_match, sandbox_spec | agwp_json, agwp_proto (all 185 AGWP messages, `gen_agwp_types.py`), job_spec (ValidationTask -> SandboxSpec -> container args), signed_json (attestation/provenance), git_parse (git bridge parsers), ignore_rules |

* Seeds: `fuzz/corpus/` (9 KB, `fuzz/gen_seeds.py`; binary formats come from `tests/conformance/vectors.json`);
  dictionaries: `fuzz/dictionaries/`; runner: `fuzz/run.sh <seconds> [targets]` (also used for CI).
* Local runs (no sanitizer; debug assertions and overflow checks on): the 14 light targets 15-30 s each, the heavy
  targets 45 s each. **Not** the 5-minute runs the brief asks for: the container's disk (a few GB, shared) could not
  hold the 2 GB heavy build alongside everyone else's, and the window closed. Do these first (see next steps).

### Bugs found and fixed (all on **main**, each with a regression test that fails on the old code - verified by reverting the fix)

| Bug | Found by | Test |
|---|---|---|
| `ag_diff::unified_diff` overflowed for a huge context (`2 * context`): panic in debug, wrong header in release (`ag diff -U <huge>`) | `diff_myers` fuzz crash | `ag-diff` `unified_diff_survives_an_absurd_context` |
| `ag_policy::evaluate`: `now - issued_at` overflowed before the signature was checked (forged attestation naming a trusted issuer) | `policy_eval` fuzz crash | `ag-policy` `extreme_timestamps_are_blocked_not_a_panic` |
| `max_age_secs` above `i64::MAX` wrapped negative and made every attestation "too old" | review while writing `policy_eval` | `a_huge_max_age_means_no_limit` |
| git import: message over 60 000 bytes cut with `String::truncate` inside a multi-byte char -> panic | review while writing `git_parse` | `crates/ag-git/tests/hostile.rs` |
| git import: author date > 9.2e15 s (git accepts 1e16) overflowed `* 1000` | same | same |
| git export: path starting with `"` came out of `fast-import` as a different file name | same, reproduced against real git | same |
| (finding, fixed in a separate commit) `services/agent-orchestrator/Dockerfile` built on `golang:1.24` (out of support): govulncheck reports **36 reachable stdlib advisories**; with go1.27.2 it reports none; `go build/vet/test` pass on 1.27.2 | govulncheck | - (image build itself unverified: no Docker daemon) |

Harness bugs found along the way (not library bugs): canon_reader, rename_detect (a "similar" match can be 100 %
for reordered lines), signed_json (`split_at` on a non-boundary - **still to fix in `fuzz/fuzz_targets/signed_json.rs`**:
split at a char boundary, then re-run).

## Compatibility fixtures (**main**: repository; **wip**: server control DB)

* `tests/compat/generate.sh <label>` builds a repository with the CURRENT `ag` (merges, conflict resolution, a
  rename-aware merge, two policy versions, signed attestations, agent runs/provenance, tasks, leases, pin, audit
  chain); `expected.py` re-derives every changeset id, tree id and the whole audit chain with the independent
  Python encoder (`tests/conformance/gen_vectors.py`) and refuses to write the fixture if Rust and Python disagree.
* `crates/ag-integrate/tests/compat.rs` (7 tests, pass): fsck clean, goldens (changeset/tree ids, history, streams),
  every merge reproduced by today's `merge_trees`, blame follows the rename, policy/attestation/provenance signatures
  verify, agents/tasks/leases/pins readable, new work can be published on top. **Fixtures are append-only.**
* **wip** `generate_server.sh`, `fixtures/control-0.1.0.*`, `crates/ag-server/tests/compat_control.rs`: the control
  fixture in the branch was generated with another agent's binary (schema v2) and is REJECTED by main's server
  (schema 1). **Regenerate it with a server built from main** (`cargo build -p ag-server`, copy the binary out of the
  shared target dir first - other agents overwrite it - then `AG_SERVER=<copy> tests/compat/generate_server.sh 0.1.0`)
  before landing; this also proves the "newer schema is refused" check.

## Supply chain (**wip** unless noted)

* `deny.toml` (**main**): `cargo deny check` passes locally (advisories, licenses, bans incl. 9 documented version
  splits, sources); the fuzz manifest additionally needs `NCSA` in `allow` (libfuzzer-sys) - not yet added.
* `security/exceptions.toml` (**main**): two tinypool (vitest, dev-only) advisories, expire 2027-01-15.
* `scripts/check_workflows.py` (**main**, not wired in CI): pinned actions / permissions / triggers / injection lint;
  `--pin` pins by SHA (resolved via `git ls-remote`). It fails today on ci.yml/release.yml/web.yml (unpinned) - run
  `--pin` over all workflows, in one commit, as the last step.
* `.github/workflows/security.yml` (**wip**, never run): cargo-deny, npm audit, pip-audit, govulncheck,
  dependency-review, hygiene. It references files that DO NOT EXIST yet: `scripts/security_exceptions.py`
  (subcommands `check|npm|pip|go`: parse the tool JSON, fail on high/critical or any pip/go finding not in
  `security/exceptions.toml`, fail on expired/over-92-day entries, cross-check `deny.toml` ignores),
  `scripts/test_security_exceptions.py`, `scripts/third_party_licenses.sh` (cargo-about; `about.toml`, `about.hbs`,
  `THIRD_PARTY_LICENSES.md`). govulncheck v1.9.0 works locally; the job should pick the Go version from the
  Dockerfile's `FROM golang:X`. **Do not land security.yml until those exist and the job was run via
  workflow_dispatch.**
* Action SHAs already resolved (peeled): checkout v7.0.1 `3d3c42e5...`, cache v6.1.0 `55cc8345...`, upload-artifact
  v7.0.2 `cf430e03...`, setup-node v7.1.0 `949feb24...`, setup-go v7.0.0 `b7ad1dad...`, setup-uv v10.3.0
  `1c37ad07...`, rust-cache v2.9.2 `6323deb1...`, cargo-deny-action v2.1.1 `3c634983...`, dependency-review v5.0.0
  `a1d282b3...`, codeql-action v4.39.0 `6f2ac81f...`, install-action v2.87.29 `8cca0918...`,
  dtolnay/rust-toolchain master `683b588c...` (full SHAs are in `security.yml`).

## Not started

`fuzz.yml`, `quality.yml` (coverage floor, cargo-mutants), CodeQL workflow, dependabot for npm/pip/gomod/docker,
`scripts/sbom.sh`, `THIRD_PARTY_LICENSES`, docs (`docs/architecture/security.md` "Fuzzing", `testing.md`,
`compatibility.md`, `docs/operations/supply-chain.md`, operator hardening checklist, SECURITY.md reconcile),
coverage numbers, mutation runs. Tools were installed under the (ephemeral) scratchpad: cargo-fuzz, cargo-deny,
cargo-llvm-cov, cargo-mutants, cargo-cyclonedx, govulncheck, nightly toolchain - reinstall as needed
(`cargo install --locked cargo-fuzz cargo-mutants cargo-llvm-cov cargo-about`; `rustup toolchain install nightly
--profile minimal`).

## Next steps, in order

1. Fix the `signed_json` harness; `fuzz/run.sh 300 <each high-value target>` (diff_myers, merge_text, merge_trees,
   changeset_decode, tree_decode, policy_json, agwp_json); fix whatever they find, with a test each; record minutes.
2. Write `fuzz.yml` (smoke 60-90 s per PR touching the crates, nightly longer, upload `fuzz/artifacts` on crash;
   `python3 fuzz/gen_agwp_types.py --check && python3 fuzz/sync_targets.py --check`; build with
   `cargo +nightly fuzz build --features heavy`; SHA-pin actions).
3. Write the three missing scripts, run `security.yml` via workflow_dispatch, fix, land.
4. Regenerate + land the server control fixture; `quality.yml`; CodeQL; dependabot; docs; pin all actions; SBOM script.

Disk: this container has a few GB for everybody; the heavy fuzz build is 2 GB - delete `fuzz/target`/the cargo-fuzz
target dir after each session.
