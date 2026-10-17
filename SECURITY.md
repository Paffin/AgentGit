# Security policy

AgentGit is software for storing and integrating source code, often written by autonomous
agents. We take integrity and isolation bugs seriously.

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.1.x (alpha) | ✅ security fixes on the latest 0.1 release |

AgentGit is **alpha** software. The on-disk formats are versioned (`ChangesetV1`, tree manifest v1,
schema v1) and are meant to be stable, but they have not had an external audit. Do not use it as the
only copy of irreplaceable data: keep backups (`ag backup`) and a Git mirror (`ag git export`).

## Reporting a vulnerability

Please **do not open a public issue** for security problems.

Report privately through GitHub:
<https://github.com/Paffin/AgentGit/security/advisories/new>

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
- tampering with the audit chain that `ag audit` does not detect.

## Out of scope / known limits

These are documented trade-offs, not vulnerabilities:

- **Local mode trusts the local user.** Anyone who can write `.ag/` can edit the database or the
  object store directly. Integrity checks (`ag verify`, the hash-chained audit log) make tampering
  *detectable*, not impossible; anchor the audit head hash somewhere you trust. Multi-user enforcement
  needs the (planned) server mode.
- `ag validate run` executes the given command **without a sandbox**. Use it only for code you trust. To run
  untrusted code use an [`ag-worker`](docs/guides/workers.md), whose [sandbox](docs/architecture/sandbox.md)
  shares the host kernel (it is not a virtual machine); its limits and untested parts are listed there.
  A way out of the sandbox is a vulnerability: please report it as described below.
- Private keys are stored as `0600` files and are not encrypted at rest.
- Case-insensitive filesystems can merge paths that differ only by case on checkout.

## Disclosure

We prefer coordinated disclosure: we will work with you on a fix and credit you in the release notes
unless you ask us not to.
