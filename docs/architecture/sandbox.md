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
| Verified in CI | yes, from inside the sandbox, on a Linux runner and again on a delegated cgroup v2 ([below](#what-was-tested-and-where)) | yes, against real Docker and real Podman (rootful and rootless), from inside the container | the limits only (Linux and macOS); it claims little |
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
| `cpu_s` | `RLIMIT_CPU` **per process** → `CpuLimit` | same | `--ulimit cpu=N:N+1` per process → `CpuLimit`, and `--cpus` (a CPU-time quota: it slows the container, it does not end it). A container's own PID 1 is not signalled at the soft limit: a busy PID 1 ends at the hard limit with exit status 137 |
| `memory_bytes` | **sum over the sandbox** of resident anonymous + shared memory, sampled every 10 ms (1 ms above half the limit) → `MemoryLimit`; `RLIMIT_DATA` at 1.25× + 32 MiB per process as backstop; **exact** if a delegated cgroup v2 with the memory controller is writable (the kernel kills the group, reported `MemoryLimit`) | `RLIMIT_DATA` per process only | `--memory` and an equal `--memory-swap` (kernel, exact). The kill is reported `MemoryLimit` when the runtime's record of the container says "OOM killed": Docker's does, **Podman 4.9's does not** (the same kill shows as exit status 137, and the notes say so) |
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
* **Classification is best effort.** A process that was *killed by a signal* while the sampler saw the sandbox
  at or above 90% of the limit is reported as `MemoryLimit`; one that died by `SIGKILL` within 50 ms of its
  CPU budget as `CpuLimit`. A command that merely *exits* with a non-zero status is the command's own result
  and is never reclassified (`supervise.rs` has the thresholds as named constants and unit tests).

**cgroup v2.** With a cgroup v2 directory the worker's user may write, the namespace sandbox puts each run in a
cgroup of its own (`memory.max`, `memory.swap.max`, `memory.oom.group`, `pids.max`), which makes memory and task
limits exact and kernel-enforced (`Enforcement::Cgroup`). The directory is, in this order: the one given to
`NamespaceSandbox::with_cgroup_base` (`ag-worker run --cgroup-base`), `AG_SANDBOX_CGROUP_BASE`, or the worker's
own cgroup or its parent when one of them is writable and has the controllers enabled (a systemd unit started
with `Delegate=yes`). The kernel's "no internal processes" rule means a cgroup that enables controllers for its
children cannot hold processes itself, so an explicit base must be an **empty** directory next to the worker:

```sh
mkdir /sys/fs/cgroup/<unit>/sandboxes && chown worker /sys/fs/cgroup/<unit>/sandboxes
echo "+memory +pids" > /sys/fs/cgroup/<unit>/sandboxes/cgroup.subtree_control
```

Without one, nothing is created, the report says `cgroup: none` with the reason in its notes, and the rlimits and
the sampler stay in force. That is the common case on a laptop or a cgroup v1 host, and it is what every run
reports unless the operator prepared a base. `ag-worker doctor --cgroup-base DIR` tells you whether it works.

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

**The image is the operator's to provide.** The sandbox never pulls, so the digest-pinned image must already be
in the runtime's local store (`docker pull name@sha256:…` once, on the worker's machine). `ag-worker run
--sandbox oci --oci-image …` checks the daemon and the image at start-up and refuses to start without them, and
`ag-worker doctor --oci-image …` shows the same; a policy `image` that is not in the store fails that job (it
goes back to the queue) before the tree is downloaded.

**What is read back, and what is not.** Every container is `inspect`ed before it is removed, and the runtime's own
*record* of it (`HostConfig`, `Config`, `Mounts`, `State`) is compared with what was asked for, property by
property (`oci_record.rs`, a pure function tested against the output of the real runtimes):

* a property the runtime recorded **differently** (privileged, a writable root, a network, no memory limit,
  capabilities kept, another user, a mount that is not the one asked for) means the check did not run as
  promised: the run is a `SandboxError::Runtime`, the job goes back to the queue, nothing is signed;
* a property the record does not contain is *unread*: the report does not claim it and `verified` stays `false`;
* whether a seccomp profile and a user namespace are in force comes from the daemon's own `info`, and so does
  `host_root` (container root is host root unless the runtime maps it to a user namespace).

So for OCI runs `verified: true` means "read back from the runtime's own record of this container". It does
**not** mean that the kernel's enforcement was observed during this run; that is what the real-runtime tests below
do, once per CI run, from inside a container.

**Cancelling and cleaning up.** A timeout, an output flood or a stop request kills the *container* through the
runtime (waiting for it to exist if it is still being created: killing only the client would leave a container
that was created and never started) and removes it, volumes included. Containers carry the label `ag-sandbox=1`
and the host name; a worker that was killed outright leaves its containers behind, and the next worker on that
host removes those whose starting process is gone (`reap_orphans`, run at start-up).

**Who the check is inside the container.** The container user must be able to write the checked-out tree. With a
rootful runtime that means the tree is owned by the user (a worker that is root gives it to 65534; a worker that
is not runs the container as its own ids); with a rootless runtime container root *is* the worker's user, so the
default is `0:0`. `--oci-user` overrides that and must be numeric.

**Two things the real runtimes taught.** (1) Podman 4.9 does not record the kernel's OOM kill (`State.OOMKilled`
stays false): a memory hog is killed exactly as under Docker, but the run reports exit status 137 instead of
`MemoryLimit`, and says why in the notes. The sandbox does not guess: 137 is also what any other `SIGKILL` looks
like. (2) A container's PID 1 does not receive `SIGXCPU` for `--ulimit cpu`, so a busy PID 1 is ended at the hard
limit (`N+1` seconds) with exit status 137, while a child of PID 1 gets the signal and is reported as `CpuLimit`.
Both are asserted by `tests/oci_real.rs`.

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
OS and CPU architecture, the report's `verified` flag and **every protection flag**, the cgroup that was
used, all limits, the network mode, the `/tmp` size, the environment variables, the read-only mounts (by
`Mount::identity` when given — a toolchain version or content hash — otherwise only by host path, which is a
weaker statement) and the OCI image and runtime. It does *not* cover the notes, `memory` and `tasks` (*how* a
limit was enforced; the limits and whether a cgroup held them are covered), `argv`, `stdin` or the tree: the
last three say *what* was checked, which the attestation signs separately. So the same check under a namespace
sandbox with a seccomp filter, and under a process sandbox, have different digests, and a policy can pin the
digest it accepts. Format: `sha256:<64 hex>`, the same shape `ag validate run` uses.

**What the digest does not say:** it records *which* protections were in place, not how strong each is. A release
that changes the seccomp deny-list or the masked `/proc` paths, or a different kernel, gives the same digest, so
a policy that has to notice that pins the worker's version separately. Hashing a profile version or the filter
itself would change every digest and every `required_environment_digest` already pinned, which needs an ADR
(`docs/adr/`); it is not done here.

## What was tested, and where

Every claim on this page is checked by a named suite in a named place. "Linux CI" is the `test (ubuntu-latest)`
job of `ci.yml`; "the runtimes workflow" is `.github/workflows/oci-runtimes.yml` (**OCI runtimes and cgroup v2**),
which runs on `ubuntu-latest` for a push to `main` or a pull request that changes `ag-sandbox`, `ag-worker`,
`ag-client`, the workflow or `Cargo.lock`. At the time of
writing the runner is Linux 6.17 (`azure`), cgroup v2 with the systemd driver, Docker 28.0.4 and Podman 4.9.3; the
runtimes are whatever the runner image ships, so a new runner image is a new test.

| Claim | Suite | Where it runs | Evidence |
|---|---|---|---|
| The namespace sandbox isolates, read back **from inside**: no route to the network or the host's abstract sockets; secrets outside the mounts unreadable; host processes invisible; `/proc` hardened; read-only mounts; only `/work` and `/tmp` persist; the kernel's `NoNewPrivs`/`Seccomp`/capability view matches the report; denied calls fail with the documented errno; IPC/UTS private; descendants die with init | `tests/isolation.rs` | Linux CI, `AG_REQUIRE_SANDBOX=1` (a runner that cannot run it fails instead of skipping; Ubuntu's AppArmor restriction on user namespaces is lifted first), as the unprivileged `runner` user | [run 39110231888](https://github.com/Paffin/AgentGit/actions/runs/39110231888) |
| Its limits: timeouts kill every descendant, fork and thread bombs, a memory hog, memory spread over processes, CPU, output, file size, descriptors; large stdin; many concurrent runs | `tests/limits.rs` | Linux CI; again under a delegated cgroup (next row) | same |
| **Kernel-exact limits on a real cgroup v2**: `memory.max`/`pids.max` written and honoured, an OOM kill reported as `MemoryLimit`, a fork bomb stopped at `pids.max`, a CPU hog ended by its budget, the cgroup gone afterwards, a base that is not a cgroup directory falls back to the rlimits | `tests/cgroup_real.rs`, and **the whole `ag-sandbox` suite again** with `AG_SANDBOX_CGROUP_BASE` | the runtimes workflow, job `namespace sandbox on a delegated cgroup v2` (`AG_REQUIRE_CGROUP=1`): the job creates the empty delegated directory by hand and runs the tests as the runner user | [run 39111592492](https://github.com/Paffin/AgentGit/actions/runs/39111592492) |
| **The OCI sandbox against a real runtime, checked from inside the container**: no network (with a positive control that reaches the host's listener under `--network host`), read-only root, `NoNewPrivs`, no capabilities, `memory.max`/`pids.max` of the container's own cgroup, a fork bomb stopped at the limit, a CPU quota and the CPU-time limit, descriptor/core/file-size limits, size-capped `/tmp`, read-only mounts, none of the worker's keys or environment visible, exit statuses/stdin, timeout and output-flood cleanup, cancellation, orphan reaping, a missing image refused | `tests/oci_real.rs` (14 tests) | the runtimes workflow, jobs `docker`, `podman (rootful)` and `podman (rootless)` (`AG_REQUIRE_OCI=1`): the job pulls a small digest-pinned BusyBox image, the tests never pull | [run 39111592492](https://github.com/Paffin/AgentGit/actions/runs/39111592492) |
| What the runtimes record for the flags the sandbox passes (the fixtures behind the record check) | `oci_record.rs` unit tests; the workflow prints each runtime's `inspect` output | every CI run; the workflow step "What the runtime records for the sandbox's flags" | same |
| The worker end to end with a container: a check writes its tree, a failing check is a signed `fail` with its own output, a missing image fails the job before the download, stopping the worker kills and removes the container | `ag-worker/tests/oci_e2e.rs` (5 tests) | the runtimes workflow, the same three jobs | same |
| The OCI argument list, supervision, the record check and the mapping of runtime failures | `tests/oci.rs` against a **fake runtime script**, unit tests | every CI run | This is a test double for what cannot be forced on a real runtime (a daemon that hangs, a record that lies). It proves nothing about what a real runtime accepts or enforces: that is the rows above |
| The limits-only fallback is honest about what it does not do | `tests/process_sandbox.rs`, unit tests, the `ag-worker` end-to-end suite | Linux and macOS CI | [run 39110231888](https://github.com/Paffin/AgentGit/actions/runs/39110231888) |

The root-only tests (the set-user-ID demonstration, the sampled task limit, `with_host_ids`) print a `NOTE` and
check what they can when the suite runs as the unprivileged runner user; they were run as root on the original
development host (a Firecracker microVM), which CI does not repeat. A host that cannot run a suite leaves an
`AG-SKIP` marker with the reason and passes: it has verified nothing. `AG_REQUIRE_COMPONENTS` naming
`namespace-sandbox`, `cgroup-v2` and `oci:docker` / `oci:podman` turns each kind of skip into a failure, the CI jobs
set it, and every skip that remains is listed and checked against a reviewed list
([Test skips](test-skips.md)).

**Not verified, and why you should not assume it works:**

* **Podman's and Docker's other versions and configurations**: the runners' versions only; no Docker Desktop, no
  WSL, no SELinux-enforcing host (bind mounts may need labels there), no user-namespace remapping (`userns-remap`),
  no other image than BusyBox, no CRI-O or containerd front end, no gVisor (`runsc`) or Kata runtime.
* **microVMs** (Firecracker, gVisor, Kata) as a boundary around the worker: recommended above, never exercised.
* **Windows**: there is no sandbox. `ag-sandbox` is empty there and `ag-worker` is a binary that refuses to run
  (`tests/unsupported.rs` proves that); the `Windows` workflow builds the whole workspace and runs every other
  suite. Checks cannot be run on a Windows worker.
* **macOS**: the namespace sandbox is compiled out and `ProcessSandbox` is the only implementation (macOS does
  not enforce `RLIMIT_DATA`, so it reports memory as unlimited). The `macos-latest` job runs the crate's unit tests
  (including the allocator guard on the forked child), its portable process-sandbox tests and the `ag-worker`
  end-to-end suite on the limits-only fallback (its isolation tests skip there). That is the whole of the
  evidence: nothing here has run on a developer Mac or Apple Silicon outside that runner.
* **aarch64**: `cargo check`ed for `aarch64-unknown-linux-gnu`, but the seccomp filter was never run on it.
* **cgroup v1** hosts: the cgroup code requires v2; on v1 the report says `cgroup: none` and the rlimits and the
  sampler apply.
* **A root worker with a delegated cgroup under systemd** (`Delegate=yes` on a service): the delegated directory
  in CI is prepared by hand with the same layout; a unit file is not tested.
* A non-root caller on a kernel with `kernel.apparmor_restrict_unprivileged_userns=1` or similar policies, which
  make `clone(CLONE_NEWUSER)` fail: the sandbox reports `Unavailable`, and there is nothing to run.

## `unsafe`

`ag-sandbox` is the only AgentGit crate that uses `unsafe` (the workspace forbids it everywhere else, and
this crate does not inherit the workspace lint table for that reason). It is `deny` crate-wide and
re-allowed in three modules — `sys` (thin libc wrappers), `spawn`/`nsinit` (the code between `clone` and
`exec`) and `seccomp` (installing the filter). Clippy's `undocumented_unsafe_blocks` is denied, so every
block carries a `SAFETY` comment. The child runs only raw system calls over data prepared before the
`clone`: glibc wrappers such as `setgroups` and `setresuid` must not be used there, because they signal and
wait for the parent's other threads, which do not exist in the child (this deadlocked under concurrent test
runs until it was fixed, and `concurrent_runs_from_a_multi_threaded_process` is the regression test).
