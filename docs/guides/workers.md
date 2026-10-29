# Running checks: execution-plane workers

A policy can say not only *that* a change needs a passing check but *how to run it*. The server then queues
a **validation job** for the exact candidate tree, and a **worker** — a trusted process you run — claims it,
runs the command in a [sandbox](../architecture/sandbox.md), and submits a **signed attestation**. The
policy decides whether that attestation counts. This page is for the person running workers.

```
 author ──proposal──▶ ag-server ◀──claim / heartbeat / complete── ag-worker ──▶ sandbox ──▶ the check
                         │                                          │
                 policy: required_validations                signs with its own key
                 + run spec, trusted_issuers                 (never visible to the check)
```

## What you need

| | |
|---|---|
| A server | `ag-server` with the execution plane (AGWP `ExecutionService`, HTTP `/v1/jobs/…`). |
| An identity | e.g. `service:ci-1`, with the **`runner`** role on the repositories it serves. The role lets it claim jobs and submit attestations; it grants nothing else beyond read access. |
| A credential | a token for that identity (`ag-server admin mint --subject service:ci-1`). Give it to the worker through an environment variable or a file, never on a command line. |
| A signing key | created with `ag-worker keygen`. Its public half must be listed in the policy's `trusted_issuers` under the same identity — **otherwise the worker runs checks that count for nothing**. |
| A sandbox | the namespace sandbox on Linux, or a container runtime (Docker or Podman) with a **digest-pinned image already pulled** on the worker's machine. `ag-worker doctor` tells you which work on this machine. |

## Setting it up

```bash
# 1. a signing key (mode 0600; never overwritten) and the policy snippet that trusts it
ag-worker keygen --out /etc/ag-worker/ci-1.key --identity service:ci-1

# 2. on the server: the role and a credential
ag-server admin --data-dir /srv/ag grant --subject service:ci-1 --repo 'acme/*' --role runner
ag-server admin --data-dir /srv/ag mint  --subject service:ci-1 --ttl 2592000     # shown once

# 3. what works on this machine
ag-worker doctor

# 4. run
AG_TOKEN=agt_… ag-worker run --server https://ag.example.com --key /etc/ag-worker/ci-1.key
```

The policy (a stream's, set by an admin) lists the issuer and says how to run each check:

```json
{
  "trusted_issuers": [{ "identity": "service:ci-1", "public_key": "<printed by keygen>" }],
  "required_validations": [
    { "check": "tests",
      "run": { "argv": ["cargo", "test", "--offline"], "timeout_secs": 1200, "memory_mb": 4096,
               "max_pids": 512, "env": { "RUST_BACKTRACE": "1" } } }
  ]
}
```

The command lives in the **policy** — set by administrators, versioned, audited — and never in the repository
under test: the code being checked does not get to choose the command that checks it. `argv` is run without a
shell. Limits left at 0 take the worker's defaults; `network` is off unless the policy turns it on **and** the
worker allows it; `image` pins a container image for container workers.

Jobs are queued when someone asks (`prepare` with `enqueue_validations`, or `RequestValidation` /
`POST …/proposals/{id}/validate`). One job per required check with a `run` spec and no passing attestation,
once per candidate tree and policy version: asking again returns the same job.

## What the worker does with a job

1. **Claims** it (a long poll, with a lease it keeps alive by heartbeat for as long as the check runs).
2. **Refuses what it should not run**, before downloading anything: a request above its ceilings, network it
   does not allow, an image it cannot run.
3. **Fetches and verifies the candidate.** The tree id is recomputed from the manifest the server sends and
   must equal the job's tree; every blob is hash-checked; paths are validated; symlinks may not leave the
   checkout. A worker does not take the server's word for what it is attesting.
4. **Runs the command** in a sandbox with the limits from the policy, a fresh empty file system containing the
   tree at `/work`, a private `/tmp`, read-only system directories, no network and a small constant
   environment (`PATH`, `HOME`, `TMPDIR`, `LANG`, `CI`, `AG_CHECK`, plus the policy's `env`).
5. **Signs** `pass` only if the command exited with status 0. Anything else — a non-zero exit, a signal,
   running out of time, memory, CPU, processes or output — is a signed `fail`: the tree did not pass under the
   limits the policy gave it.
6. **Completes** the job with the attestation, atomically. The job's `result_json` records how it went (status,
   times, peak memory, how each limit was enforced, the last 8 KiB of stdout and stderr). It is **not signed**
   and carries no authority; the attestation does.

### When the worker is at fault, the tree is not blamed

A worker that *cannot* run a check does not sign a `fail`. It fails the **job** with a retry delay so another
worker can try, and the merge is not blocked by a broken machine:

| Situation | What happens |
|---|---|
| sandbox unavailable, toolchain not mounted (`cannot execute …`), disk full | job fails, returned to the queue (10 s delay) |
| the policy asks for more than the worker's ceilings, for network, or for an image | same; the message says what and why |
| the policy pins an `environment_digest` this worker does not produce | same; the message shows both digests (the check has run by then: the digest describes the run, so it cannot be known before it) |
| the candidate has content the worker will not materialize (a symlink that points outside the checkout, an invalid path) | job fails **permanently** with the reason, because no worker can ever check that tree; the proposal stays blocked until the tree changes |
| the runtime did not create the container as asked (it recorded a privilege, a network or a missing limit the sandbox did not request) | job fails, returned to the queue, nothing is signed |
| the candidate moved on (`stale_candidate`) | job completes as `stale`, no attestation |
| the claim was lost (lease expired, job reassigned) | the running check is killed (a container is stopped and removed), the result is dropped; the server hands the job out again |
| the server rejects the credential | the worker stops with the error instead of retrying forever |

A job has a limited number of attempts (five by default); after that it stays failed and `GET …/jobs` shows
why.

## Choosing a sandbox

`--sandbox auto` (the default) uses the namespace sandbox if this machine can run it and **otherwise refuses to
start**; it never downgrades silently. See [the sandbox page](../architecture/sandbox.md) for what each one
stops and does not.

| Flag | Meaning |
|---|---|
| `--sandbox namespace` | rootless user/mount/PID/IPC/UTS/network namespaces, seccomp, dropped capabilities. Linux. |
| `--sandbox oci --oci-runtime docker --oci-image repo/img@sha256:…` | a container runtime (`docker` or `podman`); the policy's `image` is used if it names one. The image must be pinned by digest and **already in the runtime's local store** (`docker pull repo/img@sha256:…` on the worker's machine): the worker never pulls. At start-up the daemon is asked and `--oci-image` is looked for, and the worker refuses to start without them; it also removes containers a killed predecessor left behind. Verified against real Docker and real Podman, rootful and rootless, in CI (see [the sandbox page](../architecture/sandbox.md#what-was-tested-and-where)). |
| `--sandbox process --insecure-allow-process-sandbox` | limits only. **Not isolation**: the check runs with the worker's user's file and network access. For code you already trust, on a machine you do not mind losing. |

For `--sandbox oci`, `--oci-user` (numeric `uid[:gid]`) chooses who the check is inside the container. The default is the user that can write the checked-out tree: 65534 when the worker is root (it gives the tree to that user), the worker's own ids with a rootful runtime, `0:0` with a rootless one (container root is then the worker's user). `--oci-timeout-secs` bounds each housekeeping call to the runtime.

Kernel-exact memory and task limits for `--sandbox namespace` need a cgroup v2 directory the worker may write, with
the `memory` and `pids` controllers enabled and no processes in it: `--cgroup-base DIR` (or
`AG_SANDBOX_CGROUP_BASE`). Without one the limits are sampled and rlimit-based, which the job's `result_json` says
(`memory_enforcement` and `task_enforcement`: `sampled`, `rlimit` or `cgroup`). How to prepare the directory is in [the sandbox
page](../architecture/sandbox.md#resource-limits); `ag-worker doctor --cgroup-base DIR` says whether it works.

### Running as root

A worker in a container often runs as root. The namespace sandbox then maps the check to an unprivileged host
user (`--sandbox-uid`/`--sandbox-gid`, default 65534) instead of to root, and the worker `chown`s each
checked-out tree to it. That matters: the kernel does not apply the process limit to root, and a check that
somehow left its namespaces would otherwise be root. The scratch directory (`--scratch-dir`, default
`ag-worker` under the system temp directory) must be searchable by that user; the worker checks at start-up.

### Toolchains

The sandbox starts empty. `--mount HOST[:SANDBOX][=IDENTITY]` adds read-only paths; by default the host's
`/usr`, `/bin`, `/sbin`, `/lib*`, `/etc/alternatives`, `/etc/ld.so.cache` and `/etc/ssl/certs` are mounted
(`--no-system-mounts` turns that off). Give each mount an **identity** that names what it is —
`--mount /opt/rust=rust-1.89.0` — because the environment digest records the identity when there is one and
only the host *path* when there is not, which says nothing about the content. `--system-mounts-identity ID`
does the same for the default system mounts (an OS release, an image digest); without it the worker warns at
start-up and `doctor` lists the mounts that carry no identity.

## The environment digest

Every attestation carries a digest of the environment the check ran in: the sandbox, the isolation that was
actually in force, the limits, the mounts' identities, the environment variables. It does **not** include the
tree or the proposal: those are what was checked, and the attestation names them separately. So a worker
configuration yields the same digest for every tree and proposal of a given check (checks with different limits
or environment variables differ), and a policy can **pin** it
(`"environment_digest": "sha256:…"` in the `required_validations` entry): only workers in that exact
configuration can satisfy the check. To learn the digest of a worker, run one job and read
`environment_digest` in the job's `result_json`.

## Keeping it safe

* The signing key and the credential belong to the worker process. The sandbox gets neither (no file, no
  environment variable, no network); the end-to-end tests check that the key's path is not visible from inside.
  Run the worker as a dedicated user and keep the key `0600`.
* Whoever can read the key can forge validations. Whoever controls the worker's host controls its verdicts. The
  policy's `trusted_issuers` is the list of machines you trust that much.
* Do not run a worker that holds a production credential on a machine that also runs untrusted checks outside
  a sandbox. Do not accept `--insecure-allow-process-sandbox` on a shared host.
* Prefer a VM boundary for hostile code: run the worker inside a microVM or on a disposable machine. The
  sandbox shares the host kernel.
* `--allow-network` lets policies request the host's network, unrestricted. Leave it off unless the checks need
  dependencies fetched at run time; better, vendor them.
* Resource ceilings (`--max-timeout-secs`, `--max-memory-mb`, `--max-cpu-secs`, `--max-pids`) are what a
  policy author cannot exceed. Set them to what the machine can afford to lose to a runaway check. The values
  used when a policy leaves a limit at 0 are `--default-memory-mb`, `--default-pids` and `--default-cpu-secs`. The
  limits that are the same for every check are flags too: `--output-mb` (8; a check that prints more is ended),
  `--tmp-mb` (512, held in memory per check: size it for `--concurrency` lanes), `--result-tail-kb` (8, what is
  stored of each stream), `--open-files` (1024) and `--file-size-mb` (1024, the largest file a check may write).

## Operating it

* **Stopping.** SIGINT or SIGTERM finishes the current job, then exits. A second signal kills the running checks
  (their containers are stopped and removed), hands the jobs back to the queue and exits, waiting at most
  `--shutdown-grace-secs` (15) for that; if the worker is killed outright, the lease runs out and the job is
  handed out again, and the next worker on that host removes the containers it left.
* **Concurrency.** `--concurrency N` runs N jobs at once, each as its own worker name (`label-1`, `label-2`, …),
  so one lane cannot complete another's job. Memory limits are per check; size the machine for N of them.
* **Logs** go to stderr (`RUST_LOG` selects verbosity). Each job logs one line with its outcome.
* **Reading the queue.** `GET /v1/repos/{repo}/jobs?state=queued|claimed|failed` (or `ListJobs`); the job event
  topics (`job.enqueue`, `job.claim`, `job.fail`, `job.complete`, and `job.expire` for a job taken back because its worker's lease ran out) are in the audit feed.
* **Several workers** can serve the same repositories; the server hands each job to one at a time. Give each a
  distinct `--label`.
* **Timings.** `--lease-secs` (120, at least 5), `--wait-secs` (20, at most 30: the server's own limit), `--retry-after-secs`
  (10: how long a job this worker could not run waits before it is offered again), `--backoff-max-secs` (30: the
  longest wait between failed claim requests, which starts at 1 s and doubles) and `--client-timeout-secs` (120:
  one call to the server, including downloading one file of the tree). A lease or wait outside its range is
  corrected with a warning in the log.

## Limits worth knowing

* One check per job; there are no worker pools or labels to route a check to a specific kind of machine yet.
* Output is bounded (8 MiB per stream by default; the run ends if exceeded) and only the tail is stored.
* A tree larger than `--max-checkout-mb` (4 GiB) is refused. The manifest does not carry sizes, so the limit is
  enforced while downloading.
* A job whose environment digest is pinned runs to the end before the worker can compare digests (the digest
  describes the run); a worker that can never produce the pinned digest burns the job's time on every attempt
  until the attempt limit. Pin digests only for workers you have run one job on.

## Requiring current job claims

After upgrading every worker, enable this on **every server replica** and restart them:

```toml
[jobs]
require_fencing_token = true
```

The built-in `ag-worker` already sends the `attempts` value returned by `ClaimJob` as
`fencing_token` on heartbeat, complete and fail. Custom workers must do the same,
using the value from their own claim, never fetching a newer value to retry an old
result. A missing or zero token returns `failed_precondition` (HTTP 412) in strict
mode without changing the job. A stale token is still rejected by the atomic claim
check. Claiming jobs and reading them remain available.

The default is `false` for older workers. Mixed strict/compatibility replicas do
not provide an installation-wide guarantee; finish the rollout on every replica.
No metadata migration is required. This setting protects job ownership; signed
attestations must still satisfy the stream policy.
