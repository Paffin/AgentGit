# The sandbox

> Crate: [`ag-sandbox`](../../crates/ag-sandbox). Introduced for the execution plane
> ([ADR-0001](../adr/0001-final-architecture.md) §8: *"isolated sandbox executors with CPU, RAM, network and
> file-system limits"*). This page says what each mechanism **stops, what it does not stop, and what has and
> has not been verified**. If you only read one section, read [What this is not](#what-this-is-not).

AgentGit never runs an agent's code inside the engine. A **check** (build, test, lint, ...) is run by a
trusted *worker* against the exact candidate tree, and the worker signs an attestation saying so. The
sandbox is what lets that attestation mean something when the code under test is hostile: a check that can
read the worker's signing key, reach the internal network, or fill the disk is not a check.

`ag-sandbox` takes a materialized tree and a command and returns what happened plus an
[`IsolationReport`](#what-an-attestation-can-claim) — the protections that were actually in force.

## Choosing a sandbox

| | `NamespaceSandbox` | `OciSandbox` | `ProcessSandbox` |
|---|---|---|---|
| `IsolationLevel` | `Namespaces` | `Container` | `None` |
| Needs | Linux; unprivileged user namespaces | `docker` or `podman`, a **digest-pinned** image | any Unix |
| Isolation done by | this crate, with raw system calls | the container runtime | nothing |
| File system | private root, read-only except `/work` and `/tmp` | image + bind mounts (runtime) | **the host's** |
| Network | empty network namespace | `--network none` | **the host's** |
| Other processes | invisible (PID namespace) | invisible | **visible and signalable** |
| Seccomp | deny-list (below) | the runtime's default profile | none |
| Memory limit | sampled over the sandbox, per-process backstop | `--memory` (kernel, exact) | per process only |
| Verified here | yes, from inside the sandbox | **no**, no runtime on the dev host | yes (it claims little) |
| Use for | the default execution plane | hostile or multi-tenant workloads | trusted code on your own machine |

`best_available()` returns the namespace sandbox when this host can run it and the process sandbox
otherwise. **A caller that needs isolation must check `Sandbox::isolation()` (or
`Outcome::isolation.verified`) and refuse to proceed on `IsolationLevel::None`** — falling back silently is
exactly the failure this crate exists to prevent. The worker does this.

## What this is not

* **Not a virtual machine.** All three share the host kernel. A kernel vulnerability reachable through a
  system call the command is allowed to make is a way out. The seccomp filter below shrinks that surface; it
  does not remove it. For code you do not trust at all, on a machine that holds anything you care about,
  run the worker inside a VM or a microVM (Firecracker, gVisor, Kata) and treat this crate as the second
  layer, not the first.
* **Not a defence against side channels** (cache timing, speculative execution), nor against a command that
  burns CPU on cores it is allowed to use, within its limits.
* **Not a check that the command is *correct*.** A sandbox bounds what a check can do to the world; it says
  nothing about whether the check's verdict is right.
* **`ProcessSandbox` is not isolation.** It exists for local runs of trusted code, and says so in its report.

## The namespace sandbox

### What is created

For every `run` the parent process `clone`s a child directly into new **user, mount, PID, IPC, UTS and
(unless `Network::Host`) network** namespaces. No helper binary is executed and no privilege is needed
([`spawn`](../../crates/ag-sandbox/src/spawn.rs) explains why a bare `clone` is sound in a multi-threaded
host). The child becomes the PID namespace's init and, in this order:

1. clears supplementary groups, optionally switches to the mapped user (see [root hosts](#root-hosts));
2. sets the hostname, brings `lo` up (network namespace only);
3. makes every mount private, mounts a fresh `tmpfs` as the new root, `pivot_root`s into it;
4. builds the file system (below), then lazily unmounts and removes the old root — after this the host's
   file system **does not exist** for the command, not even as a path;
5. remounts the root read-only;
6. drops every capability (bounding, ambient, inheritable, permitted, effective);
7. sets `PR_SET_NO_NEW_PRIVS`;
8. installs the seccomp filter;
9. forks the command and stays behind as init: it reaps orphans and relays the command's exit status.

When the command is gone, init exits, and the kernel kills every other process in the PID namespace. There
is no way for a descendant — a daemon, a `setsid` escapee, a double fork — to outlive the run.

### What the command sees

Exactly this, and nothing else:

| Path | What | Mount options |
|---|---|---|
| `/` | a `tmpfs` | read-only, `nosuid`, `nodev` |
| `/work` | the materialized tree (`SandboxSpec::workdir`), also the current directory | read-write, `nosuid`, `nodev` |
| `/tmp` | a private `tmpfs`, `SandboxSpec::rw_tmp_bytes` big, with an inode budget | `nosuid`, `nodev` |
| `/dev` | a 64 KiB `tmpfs` with `null`, `zero`, `full`, `random`, `urandom` bound from the host, the `fd`/`stdin`/`stdout`/`stderr` links and a `shm` | `nosuid`, `noexec` |
| `/dev/shm` | a `tmpfs`, at most 64 MiB | `nosuid`, `nodev`, `noexec` |
| `/proc` | procfs **of the new PID namespace** (only the command's own processes) | `nosuid`, `nodev`, `noexec`; some entries hardened, below |
| each `ro_mounts` entry | a host directory or file (toolchains, system libraries) | read-only, `nosuid`, `nodev` |

The environment is **exactly** `SandboxSpec::env`: nothing is inherited. Standard input is a pipe carrying
`SandboxSpec::stdin`; standard output and error are pipes. No other descriptor of the host process survives
(`close_range` before the command starts; tested).

The command runs as user and group `1000` *inside* its user namespace. The kernel maps that to **one** host
id (see below). It has no capabilities even there.

### `/proc` hardening

`/proc` is procfs of the sandbox's PID namespace, but several of its files are **not namespaced**: they
control the whole host. A process whose *kernel* uid is 0 may write them without any capability, because
the permission check is by file ownership. The sandbox therefore makes the following read-only (bind mounts
onto themselves, remounted `ro`) or hides them (bind of `/dev/null`, or an empty read-only `tmpfs` for
directories), wherever the running kernel has them:

* read-only: `/proc/bus`, `/proc/fs`, `/proc/irq`, `/proc/sys`, `/proc/sysrq-trigger`;
* hidden: `/proc/acpi`, `/proc/asound`, `/proc/kcore`, `/proc/keys`, `/proc/kmsg`, `/proc/latency_stats`,
  `/proc/sched_debug`, `/proc/scsi`, `/proc/timer_list`, `/proc/timer_stats`.

This is the list container runtimes use. It was added after the test run showed that, on a root host with
the default mapping, the command could rewrite host sysctls such as `vm.overcommit_ratio` through
`/proc/sys` (see `procfs_cannot_be_used_to_reach_the_host`, which now fails if that is possible again).

### Seccomp deny-list

A classic-BPF filter, assembled by hand in [`seccomp.rs`](../../crates/ag-sandbox/src/seccomp.rs) and unit
tested against a small BPF interpreter. It is a **deny-list**: anything not named is allowed, because a
useful build environment needs hundreds of system calls and an allow-list that is not tested against the
real workload breaks builds. (Container runtimes ship an allow-list; the OCI sandbox gets theirs.)

* **Architecture gate.** A different ABI than the native one (the 32-bit compat ABI on x86-64) and the x32
  ABI are killed outright: system call numbers mean something else there and every filter below would be
  bypassable. Supported: little-endian x86-64 and aarch64; on other architectures the namespace sandbox
  reports itself unavailable instead of running without a filter.
* **Denied with `EPERM`:**
  * debugging and cross-process memory: `ptrace`, `process_vm_readv`, `process_vm_writev`, `kcmp`,
    `pidfd_getfd`;
  * changing the file-system namespace after setup (capabilities are already gone; this is the second
    barrier): `mount`, `umount2`, `pivot_root`, `chroot`, `move_mount`, `open_tree`, `fsopen`, `fsconfig`,
    `fsmount`, `fspick`, `mount_setattr`;
  * joining other namespaces: `setns`;
  * machine administration: `kexec_load`, `kexec_file_load`, `reboot`, `init_module`, `finit_module`,
    `delete_module`, `swapon`, `swapoff`, `acct`, `quotactl`, `settimeofday`, `clock_settime`,
    `clock_adjtime`, `adjtimex`, `sethostname`, `setdomainname`, `syslog`, `lookup_dcookie`;
  * kernel key management (keyrings cross user namespaces): `keyctl`, `add_key`, `request_key`;
  * large, historically vulnerable kernel interfaces that builds do not need: `bpf`, `perf_event_open`,
    `userfaultfd`, `io_uring_setup`, `io_uring_enter`, `io_uring_register`, `open_by_handle_at`,
    `name_to_handle_at`, `fanotify_init`;
  * x86-64 only: `iopl`, `ioperm`, `modify_ldt`, `uselib`, `_sysctl`, `sysfs`, `nfsservctl`.
* **Denied with `ENOSYS`:** `clone3`. Its flags live in a user-space struct that seccomp cannot read, so it
  could create namespaces unfiltered. `ENOSYS` makes libc fall back to `clone`, whose flags *are* checked.
* **Argument filters.** `clone` and `unshare` with any `CLONE_NEW*` flag (and `CLONE_NEWTIME` for
  `unshare`) fail with `EPERM`: a command cannot create nested namespaces, which are the usual first step
  of kernel exploits that need `CAP_SYS_ADMIN` somewhere. `socket` is allowed only for `AF_UNIX`, `AF_INET`,
  `AF_INET6` and `AF_NETLINK`; every other family (`AF_ALG`, `AF_PACKET`, `AF_VSOCK`, `AF_BLUETOOTH`,
  `AF_KEY`, ...) fails with `EAFNOSUPPORT`.

A unit test fails if a syscall in the filter is missing from this page, and an integration test makes the
real calls from inside the sandbox and checks the errno.

**What the deny-list does not do:** it does not stop a command from using any of the several hundred
system calls it leaves open, so a bug in any of them is reachable. It is a reduction of surface, not a
proof.

### Network

By default the command has an empty network namespace containing only a loopback interface (up). It cannot
reach the host's services, the internal network, cloud metadata endpoints or the host's abstract Unix
sockets (those are per network namespace). Tested: a TCP connection to a listener on the host's loopback
is refused and never arrives, connections to routable addresses fail with `ENETUNREACH`, the only
interface is the sandbox's own `lo`, and the host's abstract `AF_UNIX` sockets are unreachable.

`Network::Host` shares the host's network namespace. **That is an explicit opt-in that removes the network
isolation**: the command can reach everything the host can, including loopback listeners and metadata
services. It is reported (`net_ns: false`, `network: host`) and changes the environment digest. Use it only
for code and destinations you already trust — for example to let a trusted build download dependencies.

### Root hosts

A user namespace maps the command's inner id to **one host id**. By default that is the id of the process
that called `run`. For an ordinary user that is the right answer (files written to `/work` belong to that
user). For a worker that runs as **root** — which is common in containers — it has three consequences:

1. The command's *kernel* uid is 0. It has no capabilities, but anything that checks file ownership by
   kernel id treats it as the owner of every root-owned file it can reach (this is what made the
   [`/proc`](#proc-hardening) hole exploitable).
2. The kernel does not apply `RLIMIT_NPROC` to uid 0, so the `pids` limit cannot be enforced by the kernel.
   The supervisor falls back to **sampling** the sandbox's task count every 1–10 ms and killing it
   (`ExitStatus::PidsLimit`). A fork bomb can briefly exceed the limit between samples; the report says
   `tasks: sampled`.
3. A hypothetical escape from the namespaces would be an escape as root.

`NamespaceSandbox::with_host_ids(uid, gid)` maps the command to an unprivileged host user instead (only
possible when the caller is root; otherwise `run` returns an error). The command then really *is* that user
in the kernel's eyes: `RLIMIT_NPROC` is exact (`tasks: rlimit`), root-owned files are out of reach, and
`environment_digest` records `host_root: false`. The price: the user must be able to write the working
tree, so the worker `chown`s the materialized tree to it. **A worker that runs as root should always do
this.** The `ag-worker` binary does when it starts as root (`--sandbox-uid`, default 65534).

### Resource limits

All limits are in [`Limits`](../../crates/ag-sandbox/src/spec.rs); every one must be non-zero. How each is
enforced differs, and the report says how (`Enforcement`).

| Limit | Namespace sandbox | Process sandbox | OCI sandbox |
|---|---|---|---|
| `wall_ms` | supervisor timer; kills the whole sandbox → `TimedOut` | timer; kills the **process group** (a `setsid` escapee survives) | timer; `<runtime> kill`, then client kill |
| `output_bytes` | each of stdout and stderr is capped; the first byte over kills the sandbox → `OutputLimit` | same | same |
| `cpu_s` | `RLIMIT_CPU` **per process** → `CpuLimit` | same | `--ulimit cpu` |
| `memory_bytes` | **sum over the sandbox** of resident anonymous + shared memory, sampled every 10 ms (1 ms above half the limit) → `MemoryLimit`; `RLIMIT_DATA` at 1.25× + 32 MiB per process as backstop; **exact** if a delegated cgroup v2 with the memory controller is writable | `RLIMIT_DATA` per process only | `--memory` (kernel), OOM kill detected via `inspect` |
| `pids` (processes **and threads**) | exact via `RLIMIT_NPROC` for a non-root host id (the count is per user namespace, so it does not see the rest of the host); **sampled** for host uid 0; exact under cgroup v2 with the pids controller | `RLIMIT_NPROC` for non-root callers; **not enforced for root** (reported `tasks: none`) | `--pids-limit` |
| `open_files` | `RLIMIT_NOFILE` | same | `--ulimit nofile` |
| `file_size_bytes` | `RLIMIT_FSIZE` (the writer gets `SIGXFSZ`) | same | `--ulimit fsize` |
| core dumps | disabled (`RLIMIT_CORE` 0) | same | `--ulimit core` |
| `/tmp`, `/dev/shm` size | `tmpfs` `size=` and `nr_inodes=` | the host's `/tmp` (unbounded) | `--tmpfs` size |

Three things to know about the sampled limits:

* **Sampled is not exact.** Memory can overshoot between two samples. The tests pin the behaviour:
  a hog is stopped well before it reaches twice the limit; processes that each stay under the limit but
  together exceed it are caught (`MemoryLimit`), which `RLIMIT_DATA` alone cannot do.
* **File-backed pages are not counted** (program text, mapped libraries): the kernel can drop them under
  pressure and counting them would charge every process for the same shared libraries. A command that maps
  a huge file and touches all of it is bounded by `RLIMIT_DATA` only on anonymous memory.
* **Classification is best effort.** A process that died of an allocation failure while the sampler saw it
  above 90% of the limit is reported as `MemoryLimit`; one that died by `SIGKILL` after using nearly all of
  its CPU budget as `CpuLimit`. A command that merely *exits* non-zero is never reclassified.

The cgroup code was **not** exercised against a real cgroup v2 hierarchy on the development host (it has
cgroup v1 plus a v2 mount without the `memory` or `pids` controllers). The file handling is covered by unit
tests against a fake cgroup directory; the kernel-facing behaviour is untested. When no cgroup is
available, which is the common case, nothing is created and the report says `cgroup: none`.

### When setup fails

`NamespaceSandbox::availability()` performs the **complete** setup once (up to, not including, executing a
command) and caches the answer, so "available" means the whole mount, capability and seccomp sequence
works, not that a flag is set. Typical reasons for `Unavailable`, each with the reason text: user namespaces
disabled (`kernel.unprivileged_userns_clone`, `user.max_user_namespaces`, AppArmor's
`unprivileged_userns` restriction), a container runtime's seccomp profile or LSM policy forbidding
`clone(CLONE_NEWUSER)` / `mount` / `pivot_root`, no procfs, an unsupported architecture. `run` then returns
`SandboxError::Unavailable` — never a half-isolated run.

## The process sandbox

A plain child process with a scrubbed environment and resource limits. **It is not isolation**, its report
says so, and its `IsolationLevel` is `None`. What it adds over `std::process::Command`: nothing is
inherited (environment, descriptors, signal dispositions), `no_new_privs` is set, resource limits apply, the
command leads its own session and process group so a timeout kills its descendants, output is capped.
What it does not do, by test (`process_sandbox.rs`): the command reads any file the caller can read, sees
and signals the caller's processes, connects to the network, and a deliberate `setsid` escapee outlives a
timeout. Use it for local runs of code you trust; a worker must not accept it for untrusted code.

## The OCI sandbox

Builds a `docker run` / `podman run` command line and supervises the client; the isolation is the runtime's.
It requests: `--network none`, `--read-only`, `--cap-drop ALL`, `--security-opt no-new-privileges`,
`--user 65534:65534`, `--pids-limit`, `--memory` (and equal `--memory-swap`), `--cpus`, `--ulimit`s, a
size-capped `--tmpfs /tmp`, the tree bound read-write at `/work`, read-only bind mounts, `--pull never`,
and `--entrypoint` set to `argv[0]` (so the image's own entrypoint cannot change what runs). The
environment travels in a private `--env-file` (mode 0600, removed afterwards), never on a command line. The
image **must be pinned by digest** (`name@sha256:<64 hex>`) unless `allow_unpinned_image` is set, so that
"the environment a check ran in" names one exact image; the report says when it is not pinned. A timeout
kills the *container* through the runtime, not just the client, and the container is always removed.

> **Untested against a real runtime.** No Docker or Podman exists on the machine this was written on. The
> argument construction, the supervision (timeouts, output caps, stdin, cleanup) and the mapping of runtime
> failures are covered by tests that run a **fake runtime script** (`tests/oci.rs`). Whether a real
> `docker run` accepts and enforces exactly these arguments has **not** been verified. For that reason the
> report sets `verified: false` for OCI runs: the flags are *requested*, not *observed*, and the notes say
> so. Treat the first real deployment as a test.

## What an attestation can claim

`Outcome::isolation` is an [`IsolationReport`](../../crates/ag-sandbox/src/outcome.rs): one boolean per
protection (user/mount/PID/network/IPC/UTS namespaces, private root, read-only root, `no_new_privs`,
capabilities dropped, seccomp, rlimits), `host_root`, how memory and task limits were enforced
(`Enforcement`: `rlimit`, `sampled`, `cgroup`, `runtime`, `none`), the cgroup actually used, the network
mode, the image and runtime for OCI, and free-text notes. The rule is: **it never claims more than was
set up.** `verified: true` means the namespace and process sandboxes confirmed each step took effect (the
setup aborts if any step fails), and the integration tests read the kernel's own view from inside —
`/proc/self/status` (`NoNewPrivs`, `Seccomp`, `CapEff`, `CapBnd`), `/proc/self/mountinfo`, `/proc/self/ns/*`
— and compare it with the report.

`environment_digest(sandbox, report, spec)` hashes, under a versioned domain string: the sandbox name, the
OS and CPU architecture, **every report field except the notes**, all limits, the network mode, the `/tmp`
size, the environment variables, the read-only mounts (by `Mount::identity` when given — a toolchain
version or content hash — otherwise only by host path, which is a weaker statement) and the OCI image. It
does *not* cover `argv`, `stdin` or the tree: those say *what* was checked, which the attestation signs
separately. So the same check under a namespace sandbox with a seccomp filter, and under a process sandbox,
have different digests, and a policy can pin the digest it accepts. Format: `sha256:<64 hex>`, the same
shape `ag validate run` uses.

## What was tested, and where

Everything below ran on the development host: Linux 6.18 in a Firecracker microVM, x86-64, cgroup v1 with a
bare v2 mount, no container runtime. Every suite passes as **root** and again as an unprivileged user (uid
65534, started with `setpriv`); the few tests that only make sense for one of the two print a `NOTE` and
check what they can (the root-only ones: the set-user-ID demonstration, the sampled task limit,
`with_host_ids`; the user-only one: that a non-root caller may not ask for other ids).

| Suite | What it proves |
|---|---|
| unit tests | the BPF program against an interpreter; the allocator never runs after `clone`; digest stability; spec validation; cgroup file handling against a fake directory |
| `tests/isolation.rs` | from **inside** the namespace sandbox: no route to the network or the host's abstract sockets; a secret outside the mounts cannot be read or found; host processes are invisible and unsignalable; `/proc` sysctls cannot be written; read-only mounts reject writes and remounts; only `/work` and `/tmp` persist; the file system holds exactly what was asked; the environment is exactly the spec's; no inherited descriptors; the kernel's view (`NoNewPrivs`, `Seccomp`, capability sets) matches the report; denied calls fail with the documented errno; IPC and UTS are private; a set-user-ID file cannot raise privileges; descendants die with init |
| `tests/limits.rs` | timeouts kill every descendant including `setsid` daemons; fork and thread bombs are bounded; a memory hog and processes that are only together too big are stopped; CPU, output, file-size, descriptor limits; large stdin never deadlocks; exit and signal mapping; a killed host takes its sandbox with it; many concurrent runs from a multi-threaded process |
| `tests/process_sandbox.rs` | the fallback is honest: what it enforces and, as loudly, what it does not |
| `tests/oci.rs` | the OCI argument list, supervision and cleanup — **against a fake runtime** |

Tests that need a capability the host does not have (user namespaces, root) report `SKIP <name>: <reason>`
and pass, so a developer machine that cannot run the namespace sandbox does not turn red — but also does not
silently count as having verified it. Setting `AG_REQUIRE_SANDBOX=1` turns every skip into a failure; the
Linux CI job sets it (after lifting Ubuntu's AppArmor restriction on user namespaces), so a runner that
silently lost its isolation cannot go green. **That CI configuration was written without being able to run
GitHub Actions: its first run is its test.**

**Not tested, and why you should not assume it works:**

* a real Docker or Podman (none available);
* cgroup v2 limits against a real delegated hierarchy;
* macOS and other non-Linux Unix: the crate is `cargo check`ed for `x86_64-apple-darwin` (all targets,
  warning-free), the namespace sandbox is compiled out, and `ProcessSandbox` is the only implementation
  (macOS does not enforce `RLIMIT_DATA`, so it reports memory as unlimited). Three of its tests are meant to
  run on macOS CI; they have **never been run on a Mac**, nor has `ag-worker`, whose dependencies cannot be
  cross-compiled here;
* aarch64: `cargo check`ed for `aarch64-unknown-linux-gnu`, but the seccomp filter was never run on it;
* a non-root caller on a kernel with `kernel.apparmor_restrict_unprivileged_userns=1` or similar policies,
  which make `clone(CLONE_NEWUSER)` fail — the sandbox reports `Unavailable`, and there is nothing to run.

## `unsafe`

`ag-sandbox` is the only AgentGit crate that uses `unsafe` (the workspace forbids it everywhere else, and
this crate does not inherit the workspace lint table for that reason). It is `deny` crate-wide and
re-allowed in three modules — `sys` (thin libc wrappers), `spawn`/`nsinit` (the code between `clone` and
`exec`) and `seccomp` (installing the filter). Clippy's `undocumented_unsafe_blocks` is denied, so every
block carries a `SAFETY` comment. The child runs only raw system calls over data prepared before the
`clone`: glibc wrappers such as `setgroups` and `setresuid` must not be used there, because they signal and
wait for the parent's other threads, which do not exist in the child (this deadlocked under concurrent test
runs until it was fixed, and `concurrent_runs_from_a_multi_threaded_process` is the regression test).
