# Windows (and other platforms)

`ag` (the CLI), `ag-server` and the client libraries are built and tested on **Windows 10/11 and Server 2022+
(`x86_64-pc-windows-msvc`)** in the [Windows workflow](../../.github/workflows/windows.yml), next to Linux and
macOS in CI. The execution plane (`ag-sandbox`, `ag-worker`) is **Linux only**. This page says what that means:
what works, what is adapted, what is refused, and which tests prove each statement.

> Status honesty: the Windows workflow was first green on `main` at `3d03bd9`
> ([run 39093843818](https://github.com/Paffin/AgentGit/actions/runs/39093843818)): the whole workspace builds,
> `cargo test --workspace --no-fail-fast` passes on `windows-latest` (100 test targets), and the README demo runs under
> PowerShell 7 and Windows PowerShell 5.1. The next push was green too
> ([run 39094567535](https://github.com/Paffin/AgentGit/actions/runs/39094567535)); two runs after several red ones (see
> "What the first runs found" below) are not a track record. Statements marked **(unverified)** have not been observed on a Windows runner. The
> workflow is not yet a required check; it becomes one after it has been green repeatedly (see
> [`dev/handoff/windows.md`](../../dev/handoff/windows.md)).

## Support matrix

| Component | Windows | Notes |
|---|---|---|
| `ag` CLI: init, snapshot, status, sync, log/show/diff/blame, proposals, integrate, policy, agents, leases, GC, backup, verify | works | whole `ag-cli` / `ag-integrate` / `ag-storage` suites run in CI |
| Git bridge (`ag git import/export`) | works, needs Git for Windows on `PATH` | hostile-content tests run on Windows; see [path rules](#paths-and-names) |
| `ag clone/fetch/pull/push`, `ag remote` | works | TLS is rustls with bundled roots (no OpenSSL, no Windows certificate store) |
| `ag-server` (HTTP/JSON + gRPC, SQLite metadata, filesystem objects) | works | console process; Ctrl+C / Ctrl+Break / closing the console stop it gracefully |
| PostgreSQL metadata, S3 objects | client code compiles and its offline tests run; **not exercised against a server on Windows** | the backend suites need a PostgreSQL / S3 test server (Linux job) |
| Web UI | embedded build and HTTP first-use smoke in development Windows packaging | interactive browser qualification on Windows remains open |
| SDKs, Go orchestrator | not part of this page | no native executor is included in the Windows ZIP |
| `ag-sandbox`, `ag-worker` | **not supported** | the sandbox is Linux namespaces/seccomp/cgroups (or a container runtime). `ag-worker.exe` builds, but every invocation prints *"not supported on this platform"* and exits 2 (`crates/ag-worker/tests/unsupported.rs`). `ag validate run` still works: it runs a command **without** sandboxing, as it does everywhere. |

## Install

**The published beta.7 release has no Windows archive.** For that release, build from source using the
instructions below. The development release workflow now prepares a native x86_64 ZIP with `ag.exe`,
`ag-server.exe` (including the Web UI) and `ag-mcp.exe`. It excludes the Linux-only worker and the
executor that has not been qualified on Windows. A native Windows build, isolated install and first-use
smoke test must pass before the ZIP can be included in a future tagged release.

To try an unpublished build, download the `windows-install-rehearsal` artifact from a successful
[Windows workflow](../../.github/workflows/windows.yml) run and extract it into a folder you own.
It contains the distribution ZIP, `SHA256SUMS` and `install.ps1`. From that folder, use the version in
the ZIP filename (replace `VERSION` below):

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Version VERSION -BaseUrl . -NoAttestation -Prefix "$env:LOCALAPPDATA\AgentGit-preview"
& "$env:LOCALAPPDATA\AgentGit-preview\bin\ag-server.exe" dev --open
```

Stop any running AgentGit programs before upgrading an existing installation.
The installer verifies the ZIP checksum before writing executables. It only extracts the three named
programs and does not change your PowerShell profile or permanent `PATH`. Its final output includes
commands for adding the installation to the current terminal and opening the demo. A custom directory
can contain spaces or brackets. Remove that installation directory to uninstall; demo data remains in
`ag-dev-data` beneath the directory where you launched the server.

A public release's download can additionally be verified with `gh attestation verify` when GitHub CLI is
installed and signed in; `-RequireAttestation` makes that mandatory. Rehearsal artifacts are not attested,
so use `-NoAttestation` when testing one with a signed-in GitHub CLI. Without an explicit version the
installer selects a published release that actually contains a Windows ZIP. Until one exists, it says
so without installing anything. The executables are not Authenticode-signed; this flow does not bypass
Windows application trust decisions.

### Build from source today

1. Install Rust 1.89+ (`rustup`), using `x86_64-pc-windows-msvc`.
2. Install Visual Studio Build Tools with **Desktop development with C++**. SQLite and TLS libraries
   compile from source. CI also has NASM on `PATH`; whether a clean machine needs it remains unverified.
3. Run `cargo install --path crates/ag-cli --locked` and `cargo install --path crates/ag-server --locked`.
4. Install Git for Windows only if you use `ag git ...`.

The future release ZIP builds with a static C runtime to avoid requiring a separate Visual C++
redistributable. That packaging path still needs its first Windows release-run qualification.
The CLI demo also runs as [`examples/quickstart.ps1`](../../examples/quickstart.ps1), tested in
PowerShell 7 and Windows PowerShell 5.1.

## What AgentGit does with your files on Windows

### Paths and names

Repository paths are always `/`-separated UTF-8 internally; a backslash is not a valid path character
(`RepoPath`). Conversion happens at the working-directory boundary only. Because a tree built on Linux may hold
names Windows cannot write, **checkout refuses, before it writes anything,** a tree that contains:

* Windows device names with or without an extension, any case: `CON PRN AUX NUL COM0-9 LPT0-9 COM¹²³ LPT¹²³ CONIN$ CONOUT$`;
* `< > : " | ? *` in a name (`:` is an alternate data stream or a drive prefix);
* a name ending in a dot or a space (Windows drops it, so it names a different file);
* two names that differ only in case, or (macOS) only in Unicode normalisation: they would be one file.

`ag portability [REV] --target windows|macos|linux` lists exactly these for any revision, from any platform
(exit status 4 if there is anything). `ag checkout` and the workspace operations apply the rules of the machine they
run on; case sensitivity is **measured on the destination directory**, not assumed (a case-sensitive APFS volume or
a WSL directory is honoured). Nothing is silently overwritten and nothing is partially written; the fix is to
rename the path in the repository.

Independently of the platform, `RepoPath::new` never accepts a spelling of `.git` or `.ag` that a file system folds
into the real thing: `.GIT`, `.git.`, `.git ` , `.git::$INDEX_ALLOCATION`, `git~1`, and names that differ only by
code points HFS+ ignores. These are the checkout-escape defences next to the existing symlink and `.git` ones.
Tests: `crates/ag-types/src/portable/tests.rs`; Git-import attacks in
`hostile_windows_and_macos_content_cannot_plant_dot_git_or_overwrite_a_sibling`
(`crates/ag-git/tests/roundtrip.rs`); refusal and "nothing written" in
`crates/ag-integrate/src/workdir/portability_tests.rs` and `crates/ag-cli/tests/portability.rs`, which run on
Linux, macOS and Windows.

A directory junction planted in a working directory is treated like a symlink and never written through
(`checkout_refuses_to_write_through_a_junction`, Windows only). Long paths (> 260 characters) work because
checkout uses absolute paths, which `std` turns into `\\?\` paths (`deep_paths_beyond_the_legacy_windows_limit_round_trip`).

### Line endings

Bytes are bytes: `ag` never converts CRLF/LF, neither on snapshot nor on checkout
(`line_endings_are_bytes_never_converted`). This repository's own `.gitattributes` pins LF so that Windows checkouts
of *this* project compare byte-for-byte with the golden files. The Git bridge copies blobs as they are in Git; Git's
`core.autocrlf` and `.gitattributes` conversions apply to *Git's* working trees, not to the objects `ag git import` reads.

### Executable bit and symlinks

Windows has no executable bit and symlinks need a privilege most users lack. Policy, deliberately the same as
`core.symlinks=false` in other tools:

* a **symlink** is checked out as a plain file holding the link target (the target is still validated, so absolute
  targets, `..` escapes, drive prefixes and backslashes are refused);
* the **executable** flag is not applied, but the repository keeps it: `ag status` / `ag snapshot` take the flag, and
  the identity of an untouched placeholder, from the tree the directory was checked out from
  (`scan_against`; `symlinks_become_placeholder_files_on_platforms_without_them_and_status_stays_clean`,
  `the_executable_bit_is_remembered_from_the_tree_where_the_platform_has_none`);
* after `checkout`, `clone`, `pull`, `sync` and `workspace create`, `ag` prints a `note:` on stderr saying how many
  symlinks and executables were adapted. You cannot create a *new* symlink or set an executable flag from Windows.

### Atomic replace, locking, SQLite

Files are written to a temporary name and renamed over the target. On Windows a destination that is open elsewhere
(virus scanner, indexer) or read-only is retried briefly / has its read-only attribute cleared
(`ag-storage::fsutil`, tests in that module). GC and writers coordinate through `LockFileEx` on `gc.lock`
(`std::fs::File::lock`); the metadata database is SQLite in WAL mode on a local disk. Do **not** put a repository on a
network share: SQLite WAL needs shared memory that SMB does not provide.

Durability differs: `std` cannot flush a directory entry or do a write-through rename on Windows. The object's data is
flushed before the rename, the rename itself rides on the NTFS journal. After a power loss the newest objects can be
missing; `ag verify` reports them. This is stated by `FsObjectStore::durability()`; power-loss behaviour on Windows
has **not** been tested.

### Secrets

Key files and stored remote credentials get an owner-only ACL (`icacls /inheritance:r`), see
`ag_crypto::private_file`. Windows has no cheap way to read an ACL back, so on Windows a key file whose ACL somebody
later loosened is **not** detected (Unix refuses files with group/other access).

### Home, config, temp, identity, terminal

* AgentGit writes **nothing** under `%USERPROFILE%`, `%APPDATA%`, `%LOCALAPPDATA%` or `$HOME`: all state is in the
  repository's `.ag` directory (and `AG_KEY_DIR` if you set it). Temporary files use `%TEMP%`.
  Test: `the_cli_keeps_everything_in_the_repository_and_leaves_the_users_profile_alone`.
* The default identity is `user:<login>` from `USER`, `LOGNAME` or `USERNAME`, with every character an identity cannot
  hold replaced by `_` (`John Smith` becomes `user:John_Smith`).
* `ag` prints text only. `ag-server` colours its log only on a terminal that accepts it, never in a file or pipe, never
  with `NO_COLOR` (`a_redirected_log_is_plain_text`, `crates/ag-server/tests/console.rs`).

### Server as a console process

`ag-server serve` stops gracefully on Ctrl+C, Ctrl+Break, closing the console window and logoff/shutdown, draining
requests and flushing telemetry. The Unix equivalents (SIGINT/SIGTERM) are tested; the Windows ones are tested by
`ctrl_c_stops_a_console_server_gracefully` and `ctrl_break_stops_a_console_server_gracefully`, which need a console
and pass on the hosted runner (run 39093843818). Running it as a Windows *service* is not supported;
use a service wrapper such as NSSM.

### TLS and DNS

TLS is rustls only; there is no OpenSSL, `native-tls` or Schannel in the Windows dependency graph (`cargo tree
--target x86_64-pc-windows-msvc --workspace -i openssl-sys` finds nothing). Roots come from the bundled Mozilla
store, **not** the Windows certificate store: a corporate TLS-inspection CA must be added with a remote's
`ca_file`. Name resolution is the system resolver.

## Tests that are not run on Windows

Every other test runs. These are deliberately Unix-only, each for a recorded reason:

| Test or area | Reason |
|---|---|
| everything in `ag-sandbox`, `ag-worker` (`e2e.rs`) | Linux sandbox; Windows gets `unsupported.rs` instead |
| `the_binary_exports_with_headers_from_the_environment_and_flushes_on_sigterm` | delivers `SIGTERM` with `kill`; Windows has `console.rs` |
| `0600`/`0700` mode assertions (`ag-crypto`, `remote.rs`) | no mode bits; the Windows variants check the ACL with `icacls` |
| real-symlink tests in `workdir/tests.rs` | symlinks need a privilege; placeholders and junctions are tested instead |
| symlink/exec-bit steps of the Git round trip | the history is built without them on Windows (plain file stand-ins) |
| `examples/quickstart.sh` | bash and python3; `quickstart.ps1` runs instead |
| `ag-worker` `tests/version.rs` | the Windows binary refuses every invocation with status 2, `--version` included (`unsupported.rs` pins it) |
| `a_path_that_starts_with_a_quote_survives_the_export` (`ag-git/tests/hostile.rs`) | a `"` is not allowed in a Windows file name and Git for Windows refuses such a path itself, so there is no repository to import; the quoting rule it protects (`a_path_that_starts_with_a_quote_is_quoted_for_fast_import`) runs everywhere |
| second half of `a_tree_with_windows_device_names_is_refused_whole_before_anything_is_written` | checks the same tree out under the POSIX rules onto the real file system, which on Windows cannot hold `NUL` or `a:b`; the refusal under the Windows rules, the half that matters here, runs |

### What the first runs found

One real portability bug in the product: `ag doctor` did not compile on Windows (a key-permission list that was only
filled in a `#[cfg(unix)]` block, which broke the whole Windows build); on Windows it now checks no modes, and says so
instead of claiming "permissions private". Everything else the runs found was an assumption a test made about the
host, fixed without weakening what is asserted: the CLI printed `.ag\config.json` where a test compared text with
`.ag/config.json` (now compared as a path); a refused loopback connection takes about two seconds to be reported on
Windows (its SYN retries), which equals the 2 s timeout of the `ag-mcp` unreachable-server test, so that test gives
Windows 6 s and still asserts that the client reports `unavailable`; and the three rows above.

## Other platforms

macOS is in CI and gets the same path rules and the case/normalisation checks (APFS is case-insensitive and
normalisation-insensitive by default). The namespace sandbox needs Linux; on macOS only the container and
limits-only sandboxes exist. 32-bit and ARM64 Windows are not built or tested.
