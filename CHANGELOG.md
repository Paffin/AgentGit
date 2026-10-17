# Changelog

All notable changes are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres to
[Semantic Versioning](https://semver.org/) — while `0.x`, minor versions may change behaviour, but
on-disk formats are versioned and refuse to open when newer than the build understands.

## [Unreleased]

## [0.1.0] — 2026-10-17

First public alpha.

### Added

- **Deterministic core**: SHA-256 domain-separated content IDs, strict canonical binary encoding,
  `ChangesetV1`, derived trees with delta/first-parent semantics, DAG algorithms (ancestors, merge bases),
  Myers diff, deterministic three-way merge for text and trees with typed conflicts. Formats verified
  against an independent Python implementation (`tests/conformance/`).
- **Storage**: filesystem object store (fsync + atomic rename, verified on read, self-healing, GC-safe
  deletes) and a SQLite metadata store (WAL, `synchronous=FULL`) with changeset index, streams + reflog,
  intents, transactional outbox, hash-chained audit log, policies, proposals, reviews, attestations, pins,
  workspaces and agent state.
- **Engine** (`ag-integrate`): verified publication; integration with candidate computation outside the
  lock and policy + compare-and-swap inside one transaction; idempotent, resumable operations with fault
  injection at every step; integration queue; garbage collection that fails closed and cannot lose a
  concurrently re-referenced object; `ag verify`; rebuildable, sparse (keyframe) derived tree cache;
  consistent verified backups.
- **Policy**: versioned per-stream JSON policy; trusted issuers; signed attestations bound to tree, policy
  version and environment; approvals bound to the reviewed head; protected paths; agent autonomy envelope;
  provenance requirements.
- **Agent-native**: agent identities and runs, tasks with dependencies, soft and exclusive leases with
  fencing tokens, signed provenance stored beside immutable changesets, path-level conflict prediction.
- **Workspaces**: isolated directories, snapshots, sync with conflict markers and resolution.
- **Git bridge**: import and export through git plumbing; trees round-trip with identical SHAs.
- **CLI** `ag` with stable exit codes (0 ok, 1 error, 2 usage, 3 blocked, 4 refused) and `--json`.
- **Docs and tooling**: architecture, guides, operations, generated CLI reference, ADR traceability,
  documentation consistency checks, runnable end-to-end demo, CI.

### Known limitations

Local mode trusts the local user; no sandbox for `ag validate run`; flat tree manifests; no rename
detection; no network server. See [ROADMAP.md](ROADMAP.md) and
[docs/architecture/security.md](docs/architecture/security.md).

[Unreleased]: https://github.com/Paffin/AgentGit/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Paffin/AgentGit/releases/tag/v0.1.0
