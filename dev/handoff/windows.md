# Windows track — handoff

Status when this was written: the code and `docs/operations/windows.md` are landed (main c4ebdb2); the Windows CI
workflow (`.github/workflows/windows.yml`) was **not yet green** — its runs for c4ebdb2:
<https://github.com/Paffin/AgentGit/actions/runs/39077344233> (Windows), <https://github.com/Paffin/AgentGit/actions/runs/39077344272> (CI).
See "Verified" for exactly what was observed.

## Verified (with evidence)

* `x86_64-pc-windows-msvc` **builds** the whole workspace on `windows-latest`
  (run <https://github.com/Paffin/AgentGit/actions/runs/39073407925>, first Windows run, commit 4c30244):
  `cargo build --workspace --all-targets` succeeded in ~5 min; aws-lc-sys/ring/rusqlite compile (the runner has
  NASM from Strawberry Perl; whether a clean machine needs it is **not verified**).
* In that same run `cargo test --workspace --no-fail-fast` passed every crate except 10 targets, all of which were
  Windows bugs fixed in the following commit (`ef20d10`): object-store `set_modified` needed
  `FILE_WRITE_ATTRIBUTES` ("Access is denied" — the root of 8 of the 10), CRLF checkout of golden files
  (`.gitattributes` now forces LF), Unix-only tests (`kill -TERM`, backslash file names), the `ag-sandbox`
  doc-test, the ACL assertion (a path containing `Users` matched a "no Users" check).
* No OpenSSL / native-tls / schannel in the Windows dependency graph (`cargo tree --target x86_64-pc-windows-msvc
  --workspace -i openssl-sys` -> "did not match any packages"); TLS is rustls (the `security.rs` TLS test passed on Windows).
* Locally on Linux: `cargo clippy --workspace --all-targets -D warnings`, `cargo fmt --check`, the tests of every
  crate touched, and a Windows **type-check** with a fake C compiler (`cargo check/clippy --target
  x86_64-pc-windows-msvc`, see below) are clean. The Windows *runtime* behaviour of anything added after run 1 has
  **not** been observed: the commit `ef20d10` has not yet had a Windows CI result when this was written.

## What landed (commit ef20d10 on main, plus 4c30244)

* `ag-sandbox` / `ag-worker` are Unix-only: tests gated, `ag-worker` binary refuses with a tested message (exit 2).
* `ag-crypto::private_file` (0600 on Unix; owner-only ACL through `icacls` on Windows, not readable back).
* `ag-types::portable` + `RepoPath`: `.git`/`.ag` aliases (NTFS dots/spaces, `::$INDEX_ALLOCATION`, `git~1`, HFS+
  ignorable code points) are never valid paths; per-target hazards (device names, `<>:"|?*`, trailing dot/space) and
  case/normalisation collisions are computed by pure functions testable anywhere.
* `ag-integrate::workdir::platform`: `Platform` (probes case sensitivity of the destination), checkout/update refuse
  an unrepresentable tree **before writing**, symlinks become placeholder files and the exec flag is remembered
  (`scan_against`), `std::path::absolute` for long paths, `ag-storage::fsutil` (retrying rename/remove, read-only
  files), `ag portability [REV] --target windows|macos|linux`.
* `ag-server`: Ctrl+C/Ctrl+Break/close/shutdown handling, ANSI only on a terminal; `ag`: `%USERNAME%` identity,
  `validate run` keeps `SystemRoot`/`ComSpec`; `examples/quickstart.ps1` (PowerShell 7 and 5.1).
* Tests: `crates/ag-types/src/portable/tests.rs`, `crates/ag-integrate/src/workdir/portability_tests.rs`,
  `crates/ag-cli/tests/portability.rs`, `crates/ag-server/tests/console.rs`, the Windows/macOS hostile-content
  test in `crates/ag-git/tests/roundtrip.rs`.

## Gated / known

* Unix-only on purpose: `ag-sandbox` (empty crate), `ag-worker` (stub binary), SIGTERM test
  (`tracing_export.rs`, `console.rs` covers Windows), 0600 mode assertions, Unix symlink tests.
* Not verified: `ctrl_c_stops_...` / `ctrl_break_stops_...` in `console.rs` (they drive `GenerateConsoleCtrlEvent`
  through a PowerShell helper and may not work on a runner without an interactive console — if they fail there,
  keep them `#[ignore]` with that reason and say so in docs); `quickstart.ps1` under both shells; ACL test output.
* Durability on Windows: the rename is not flushed (std has no write-through rename) — stated in
  `FsObjectStore::durability()`.

## Next steps (exact)

1. Look at the Windows run for `ef20d10`: Actions -> "Windows". Download logs with the artifact tooling or
   `get_job_logs`; fix failures (likely candidates: `quickstart.ps1` quoting, `console.rs` helper, ACL test).
2. `docs/operations/windows.md` is written (landed in c4ebdb2); after step 1, flip every **(unverified)** in it to
   verified or fix it, and update the "Status honesty" paragraph. Touch `docs/getting-started.md` (Windows install
   line) and `docs/architecture/adr-traceability.md` only with facts from step 1.
3. Once green twice in a row, tell the orchestrator that "Windows" can become a required check. For the release
   track: ship `ag.exe` and `ag-server.exe` only; build with `RUSTFLAGS=-C target-feature=+crt-static` to avoid
   the VC++ redistributable (**not yet verified** — add a CI step using `dumpbin /dependents`); `ag-worker.exe` is
   a stub; `git` (Git for Windows) is needed only for `ag git ...`.
4. Optional: a CI step that removes `nasm` from PATH to prove whether a clean machine needs it.

## Local Windows type-check without Windows (Linux)

`rustup target add x86_64-pc-windows-msvc`; build scripts for ring/aws-lc/sqlite need a C compiler, so point
`CC_x86_64_pc_windows_msvc`, `AR_x86_64_pc_windows_msvc` at a script that creates the requested output file and
exits 0, then `cargo check --target x86_64-pc-windows-msvc --workspace --all-targets` (and `clippy`). It catches
`cfg(windows)` errors only; it runs no code.
