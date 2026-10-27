# Roadmap

AgentGit follows [ADR-0001](docs/adr/0001-final-architecture.md): correctness first, narrow before wide.
This list is ordered by dependency, not by date, and it only lists what is **not** built. What exists is
described in the [architecture traceability](docs/architecture/adr-traceability.md).

## 0.1 — shipped (alpha)

Deterministic core with canonical formats and independent-implementation vectors · verified publication
and atomic CAS integration with idempotent, resumable intents · deterministic three-way merge · policy,
signed attestations, approvals, protected paths · agent identities, runs, tasks, leases with fencing,
signed provenance, conflict prediction, integration queue · workspaces with conflict markers · GC, fsck,
backup, tamper-evident audit · Git import/export · the `ag` CLI.

## Built since 0.1 (in the `v0.2.0-rc.2` pre-release and on `main`)

Each item is described, with what is and is not verified, in the [changelog](CHANGELOG.md) and the linked page.

- **The network layer.** `ag-server`: AGWP v1 over gRPC and HTTP/JSON on one port ([reference](docs/protocol/agwp-v1.md),
  generated [OpenAPI](docs/protocol/openapi.json)); OIDC and scoped, expiring credentials; per-repository RBAC;
  quotas (object bytes atomic on every write path) and rate limits; live events (SSE) with replay; Prometheus
  metrics and OpenTelemetry traces.
- **Server hardening.** Credentials bound to one run, the `delegator` role, operator finish and reap of tasks and
  runs (locally and `ag agent reap --remote`, with a leased periodic reaper), `agent.operate` for acting on another
  agent's run, atomic stream and proposal quotas, keyset paging with indexes, trusted proxies and a limit on failed
  logins, idempotency records that expire, a global in-flight byte budget, derive status and dangling references in
  `ag doctor`, `/readyz` and metrics, a fencing token for job claims, fail-closed control rows
  ([ADR-0010](docs/adr/0010-run-bound-credentials.md),
  [ADR-0011](docs/adr/0011-proposal-paging-indexes-and-background-leases.md),
  [ADR-0012](docs/adr/0012-job-claim-fencing.md)); the audit of mocks, stubs and hardcoded values (`dev/audit/`)
  triaged and fixed by the work packages (see `dev/handoff/`).
- **Performance work**: an integration costs 3 durable commits instead of 7-8, a decoded-tree cache with a memory
  budget, a bounded retry for same-stream contention
  ([numbers and conditions](docs/architecture/performance.md)).
- **Several replicas.** Control plane on PostgreSQL, tested with real processes against PostgreSQL and S3
  ([ADR-0007](docs/adr/0007-multi-replica-server.md), [guide](docs/operations/high-availability.md)), Kubernetes `ha` overlay.
- **PostgreSQL metadata and S3-compatible objects**, per repository, tested against a real S3 server (SeaweedFS) and moto.
- **Execution plane**: `ag-worker` claims validation jobs, runs the policy's command in an `ag-sandbox` sandbox and
  submits signed attestations; the namespace and OCI sandboxes are proven in CI against real Docker, Podman and a
  delegated cgroup v2.
- **Remote workflow in the CLI**: `clone`, `fetch`, `pull`, `push`, `ag remote …`, offline use; `ag doctor`,
  help topics, completions and man pages.
- **History**: rename-aware merge ([ADR-0005](docs/adr/0005-rename-aware-merge.md)), line-level blame that follows
  renames.
- **Clients**: the Web UI (served by `ag-server`; Chromium, Firefox, WebKit, tablet and phone; OIDC against a real
  Keycloak), TypeScript and Python SDKs, `ag-mcp` for agents ([ADR-0008](docs/adr/0008-mcp-server.md)), and the Go agent
  platform (no simulated agent; optional model-backed advisor, reviewers and index).
- **Operations and distribution**: Dockerfile, Compose, Kubernetes, Prometheus / Grafana / OpenTelemetry files run in
  CI (`Containers`), a release workflow that published the pre-release `v0.2.0-rc.2` (archives, SBOMs, checksums,
  multi-arch images, attestations; what was verified: [releasing](docs/operations/releasing.md#evidence)), install
  script, Homebrew and Scoop templates, Windows builds and tests in CI.
- **Measurements**: engine benchmarks and a server load driver (`ag-bench`), numbers published with their conditions
  ([engine](docs/architecture/performance.md), [server](docs/operations/performance.md)); scale
  ([ADR-0006](docs/adr/0006-flat-trees-measured.md)).
- **Security and quality gates**: supply-chain scans, CodeQL, fuzzing, coverage floors, mutation testing,
  compatibility fixtures ([Testing](docs/architecture/testing.md)).
- **Documentation**: a strict-built MkDocs site, tutorials executed in CI, FAQ, glossary, comparison, generated
  reference pages, doc and no-stub checks in CI.
- **Easy to pick up (on `main`, not in rc.2).** A Web UI a first-time user can use (Home, plain labels, blockers with their
  fix as a button, guided flows, rules presets, editing in the browser; proven by a task-based end-to-end test);
  `ag-server dev`, `ag login`, policy presets, `ag explain`; and a test gate under which a test cannot pass by not
  running (required components, a reviewed skip list, a report in every job).

## Beta exit criteria

The next release is scoped around one reliable workflow, not every deployment option. The
[beta programme](docs/community/beta.md) defines the supported target, measurable acceptance gates and pilot process.
Track work in the [beta milestone](https://github.com/Paffin/AgentGit/milestone/1).

## Next

Only what is not done. The order comes from the first outside validation
([report](dev/feedback/validation-2026-10-21.md), 2026-10-21): the engine works, the product was hard to pick up. The
Web UI for a first-time user, the five-minute start (`ag-server dev`, `ag login`, presets, `ag explain`) and the test
gate that stops a test from passing by not running are built and on `main` (see the [changelog](CHANGELOG.md)); what
they left open is listed first.

1. **Finish the first-time-user work.**
   Browser editing covers one text file per change request (no rename, delete, multi-file or non-text conflicts);
   some tab labels and table headers still need glossary hints; Home is polled every 15 s, not live; the rules form leaves
   protected-path approvers and check timing in the raw JSON; no screen-reader pass; the `ag-server dev` sign-in link is not one-time (needs an exchange
   endpoint); the Docker one-liner is not verified (no Docker daemon yet) and `dev`, `login` and `explain` are missing
   from the `v0.2.0-rc.2` archives until the next tag.
2. **Orchestration that works out of the box.** The orchestrator in the release archives and as an image, a ready
   template "task, agent, proposal" with a real model executor verified live. ("Give this task to an agent" exists in the
   Web UI.)
3. **Finish the release.** Verify what rc.2 left unverified (archive attestations, macOS and aarch64 archives outside CI,
   the Homebrew formula); add an SBOM for `ag-mcp` and a Windows archive; cut a stable tag (until then `install.sh` needs
   `--version`) that carries `ag-server dev`; publish the 21 `agentgit-<x>` crates and the SDKs. Only the owner can push
   tags and publish.
4. **Prove what is only argued.** The `ha` and Prometheus Operator overlays and the images on a real cluster (ingress,
   storage class); mixed builds during a rolling upgrade (metadata schema 3 needs the new build first); a PostgreSQL
   failover; the periodic reaper with real replicas on PostgreSQL and S3; load on PostgreSQL, S3, TLS and several
   replicas, and the tree cache on the disk backend; resolve surviving policy mutations and retain nightly fuzz, mutation and performance evidence; a second OIDC provider (Entra, Okta, Auth0); a screen-reader and a real-phone pass.
5. **Test-gate leftovers.** The root-only halves of the sandbox tests (`n/a:host-privilege`) run in no CI job; the four
   scripts that read TOML need Python 3.11 and are not in the 3.9 job; a bare `if !cond { return }` in a new Rust test is
   not caught by the scan.
6. **Server follow-ups.** A server option that requires the job-claim fencing token; the resolved client address in the
   hash-chained audit entries; `Arc<Tree>` in the tree cache; a per-stream in-process queue so same-stream writers take
   turns instead of retrying; merge notes on the AGWP wire (and so in the Web UI); copy search over the server and MCP.
7. **Scale and isolation, when needed.** Hierarchical Merkle trees, packed objects, incremental GC marking, streaming
   blobs (triggers in [ADR-0006](docs/adr/0006-flat-trees-measured.md)); microVM isolation (Firecracker, gVisor, Kata)
   for hostile multi-tenant use; a sandbox on macOS and Windows; worker pools and labels; per-job log storage.

## Ideas we are deliberately not doing

Replacing Git for existing human workflows tomorrow; letting a model decide anything the deterministic core
guarantees; a global repository lock; hiding policy decisions inside prompts.

## How to influence this

Open an issue using the feature template and say which guarantee your idea touches; changes to formats,
hashing, merge semantics or the publication protocol need an ADR ([CONTRIBUTING](CONTRIBUTING.md)).
