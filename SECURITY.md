# Security policy

AgentGit is software for storing and integrating source code, often written by autonomous
agents. We take integrity and isolation bugs seriously.

## Supported versions

| Version | Supported |
|---------|-----------|
| `main` (pre-release) | security fixes land here first; this is the only thing that exists to run |
| 0.1.x | **no release has been published yet** - there is no `v0.1.0` tag, GitHub release or published image. When the first tag is cut, the latest 0.1 release will be supported and this table will say so |

AgentGit is **alpha** software. The on-disk formats are versioned (`ChangesetV1`, tree manifest v1,
metadata schema 2, control schema 3) and are meant to be stable - [what is promised, and the old data that
proves it](docs/architecture/compatibility.md) - but they have not had an external audit. Do not use it as the
only copy of irreplaceable data: keep backups (`ag backup`) and a Git mirror (`ag git export`).

## Reporting a vulnerability

Please **do not open a public issue** for security problems.

Report privately through GitHub:
<https://github.com/Paffin/AgentGit/security/advisories/new>

If that page is not available to you (private reporting has to be enabled by the repository's owner, and this file
cannot check that it is), open an issue that says only "I have a security report" - no details, no affected
component - and a maintainer will arrange a private channel.

Include what you can: affected version/commit, steps to reproduce, impact, and a suggested fix if you
have one. You will get an acknowledgement as soon as a maintainer sees it; this is a volunteer project,
so we cannot promise a fixed response time, but we triage reports before feature work.

## What counts as a vulnerability

We are especially interested in anything that breaks one of the documented guarantees
(see [`docs/architecture/security.md`](docs/architecture/security.md)):

- publishing or integrating something that is **not** what was verified (hash/ID confusion,
  non-canonical encodings that decode to the same value, TOCTOU between verification and commit);
- losing or silently reordering a stream update (CAS bypass, lost update, reflog divergence);
- bypassing policy: integrating without the required validation/approval/lease, reusing an
  attestation or approval for a different tree/head/policy version, forging an attestation or
  provenance record;
- writing outside a checkout (path traversal, symlink escape, planting `.git`/`.ag`);
- garbage collection deleting reachable data;
- denial of service through a small input (decompression-style amplification, unbounded
  allocation, quadratic behaviour in the object decoders or merge);
- tampering with the audit chain that `ag audit` does not detect;
- **`ag-server`**: using a credential outside its scope (repository, stream, operation, run), escalating through
  delegation, a revoked or expired credential that is still accepted, reading or writing another repository, a
  quota or rate limit that does not hold;
- a secret (token, key) that ends up in a log line, a response, a metric or an audit entry;
- **supply chain**: a release artefact that does not match its checksum or attestation, a workflow that can be made
  to leak a secret or publish from a pull request, a dependency with a vulnerability our code can reach that the
  [gates](docs/operations/supply-chain.md) did not catch.

## Out of scope / known limits

These are documented trade-offs, not vulnerabilities:

- **Local mode trusts the local user.** Anyone who can write `.ag/` can edit the database or the
  object store directly. Integrity checks (`ag verify`, the hash-chained audit log) make tampering
  *detectable*, not impossible; anchor the audit head hash somewhere you trust. Several users need
  `ag-server`, which authenticates and authorises them.
- `ag validate run` executes the given command **without a sandbox**. Use it only for code you trust. To run
  untrusted code use an [`ag-worker`](docs/guides/workers.md), whose [sandbox](docs/architecture/sandbox.md)
  shares the host kernel (it is not a virtual machine); its limits and untested parts are listed there.
  A way out of the sandbox is a vulnerability: please report it as described below.
- Private keys are stored as `0600` files and are not encrypted at rest; neither are the control database or the
  object store. See the [hardening checklist](docs/operations/hardening.md#what-this-does-not-cover).
- Case-insensitive filesystems can merge two paths that differ only by case on checkout. (Aliases of `.git` and
  `.ag` - `.GIT`, `.git.`, `git~1`, ignorable Unicode - *are* rejected.)
- A deployment that runs `ag-server` in plaintext behind a TLS-terminating proxy
  (`server.allow_insecure_remote`) is only as safe as the network between the two.
- A caller with the permissions a repository role grants can use them up to the configured quotas; that is the
  quota doing its job, not a denial of service.
- There has been **no external security audit**.

## How we look for problems ourselves

Nothing here replaces a review or an audit, and each line says where it runs:

| What | Where |
|---|---|
| Vulnerable dependencies (RustSec, npm, PyPI, Go reachability), licences, SBOMs, pinned actions | [`security.yml`](.github/workflows/security.yml): every change and daily; findings we accept are listed with an owner and an expiry in [`security/exceptions.toml`](security/exceptions.toml) ([how](docs/operations/supply-chain.md)) |
| Static analysis of Rust, TypeScript, Python and Go | [`codeql.yml`](.github/workflows/codeql.yml): every change and weekly; an alert fails the job unless it is accepted in the same file |
| Fuzzing of every parser and decoder that takes untrusted bytes (20 targets) | [`fuzz.yml`](.github/workflows/fuzz.yml): 60 s per target on changes to the fuzzed crates, 15 minutes per target nightly; results in [the security model](docs/architecture/security.md#fuzzing) |
| Coverage per crate, with floors; mutation testing of the five crates that must not be wrong | [`quality.yml`](.github/workflows/quality.yml); numbers in [Testing](docs/architecture/testing.md) |
| Compatibility with data older builds wrote | `tests/compat/` ([Compatibility](docs/architecture/compatibility.md)) |

To harden a deployment, use the [checklist](docs/operations/hardening.md).

## Disclosure

We prefer coordinated disclosure: we will work with you on a fix and credit you in the release notes
unless you ask us not to.
