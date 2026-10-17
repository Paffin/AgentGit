# Windows (and other platforms)

`ag` (the CLI), `ag-server` and the client libraries are built and tested on **Windows 10/11 and Server 2022+
(`x86_64-pc-windows-msvc`)** in the [Windows workflow](../../.github/workflows/windows.yml), next to Linux and
macOS in CI. The execution plane (`ag-sandbox`, `ag-worker`) is **Linux only**. This page says what that means:
what works, what is adapted, what is refused, and which tests prove each statement.

> Status honesty: the first Windows run compiled the whole workspace and passed most suites; the failures it found
> were fixed in the same series of commits. Statements marked **(unverified)** have not yet been observed on a
> Windows runner. The workflow is not yet a required check; it becomes one after it has been green repeatedly
> (see [`dev/handoff/windows.md`](../../dev/handoff/windows.md) for the exact state).

## Support matrix

| Component | Windows | Notes |
|---|---|---|
| `ag` CLI: init, snapshot, status, sync, log/show/diff/blame, proposals, integrate, policy, agents, leases, GC, backup, verify | works | whole `ag-cli` / `ag-integrate` / `ag-storage` suites run in CI |
| Git bridge (`ag git import/export`) | works, needs Git for Windows on `PATH` | hostile-content tests run on Windows; see [path rules](#paths-and-names) |
| `ag clone/fetch/pull/push`, `ag remote` | works | TLS is rustls with bundled roots (no OpenSSL, no Windows certificate store) |
| `ag-server` (HTTP/JSON + gRPC, SQLite metadata, filesystem objects) | works | console process; Ctrl+C / Ctrl+Break / closing the console stop it gracefully |
| PostgreSQL metadata, S3 objects | client code compiles and its offline tests run; **not exercised against a server on Windows** | the backend suites need a PostgreSQL / S3 test server (Linux job) |
| Web UI, SDKs, Go orchestrator | not part of this page | not run by the Windows workflow |
| `ag-sandbox`, `ag-worker` | **not supported** | the sandbox is Linux namespaces/seccomp/cgroups (or a container runtime). `ag-worker.exe` builds, but every invocation prints *"not supported on this platform"* and exits 2 (`crates/ag-worker/tests/unsupported.rs`). `ag validate run` still works: it runs a command **without** sandboxing, as it does everywhere. |

## Install

Build from source (a release installer is the release track's job, see [Releasing](releasing.md)):

1. Rust 1.89+ (`rustup`), the *MSVC* toolchain (`x86_64-pc-windows-msvc`).
2. Visual Studio Build Tools with the **Desktop development with C++** workload (SQLite and the TLS libraries
   are compiled from source). The CI runner also has NASM on `PATH`; whether a clean machine needs it is **(unverified)**.
3. `cargo install --path crates/ag-cli` and `cargo install --path crates/ag-server`.
4. Git for Windows only if you use `ag git ...`.

A built binary needs no installer; whether it needs the Visual C++ redistributable depends on the CRT linkage
(**unverified**: build with `-C target-feature=+crt-static` to avoid it).

The demo from the README runs on Windows as [`examples/quickstart.ps1`](../../examples/quickstart.ps1) (PowerShell 7
and Windows PowerShell 5.1); CI runs it in both.

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
(**unverified** on hosted runners; see the handoff note). Running it as a Windows *service* is not supported;
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

## Other platforms

macOS is in CI and gets the same path rules and the case/normalisation checks (APFS is case-insensitive and
normalisation-insensitive by default). The namespace sandbox needs Linux; on macOS only the container and
limits-only sandboxes exist. 32-bit and ARM64 Windows are not built or tested.
