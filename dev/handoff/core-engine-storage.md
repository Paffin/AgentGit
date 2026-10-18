# Core, engine and storage — handoff (work package `core_engine_storage`)

Scope: `ag-types`, `ag-object`, `ag-changeset`, `ag-crypto`, `ag-tree`, `ag-dag`, `ag-diff`, `ag-merge`, `ag-integrate`,
`ag-storage` and `migrations/`: the audit areas core-formats, core-merge, engine and storage (`dev/audit/CONFIRMED.md`,
`audit-final.json`, verified at commit 5d35ee2; every item was re-read against `938f744` before being touched, and again after
rebasing onto the server work package). Everything below is on `main`.

## What changed

### Real S3 server leg (the headline)

* `scripts/s3-test.sh` starts **moto** (default, unchanged) or, with `AG_S3_SERVER=seaweedfs`, the real **SeaweedFS 4.48**
  server the Compose stack runs: pinned release, download checked against a pinned SHA-256 (linux and darwin, amd64 and arm64;
  cached under `~/.cache/agentgit`, or `AG_SEAWEEDFS_BIN`), one process on loopback, a single configured identity (so the gateway
  authenticates every request: the script refuses to continue unless an unsigned request gets 403, on HTTP and on HTTPS), the
  bucket created the way `deploy/compose` does it, an HTTPS listener with a certificate from a private CA made with `openssl`.
  It strips the caller's `AWS_*` variables from the server's environment, switches off the gateway's Iceberg/Lance listeners
  (fixed default ports that collide between runs), and kills a server that does not stop within ten seconds.
* CI (`ci.yml`, job *PostgreSQL + S3 backends*): the first step is unchanged (whole workspace, PostgreSQL + moto); a second step
  runs `ag-storage`, `ag-integrate` and the server's `storage_backends` suite on PostgreSQL + SeaweedFS.
* What it found:
  * **Nothing in the S3 client had to change**: every existing S3 test (`tests/s3.rs`, the engine's GC/backup/lifecycle on S3)
    passed on SeaweedFS 4.48 unchanged, and a boto3 probe confirmed the behaviours the client relies on (a second conditional
    create answers 412, a self-`CopyObject` with `REPLACE` moves `LastModified`, bad signatures get 403).
  * S3 over **TLS had never been exercised** by any test (the client's `ca_bundle` path included): new test
    `https_works_with_the_private_ca_and_no_other_certificate_is_trusted` (round trip with the CA; refused without it; an empty
    bundle is an error). It passes.
  * The test helper defaulted the credentials to `test`/`test`: now required.
  * **A failure mode of the server itself**: its volume server turns read-only when less than 1 % of the *file system* is free
    (gigabytes on a big, nearly full disk) while the gateway and the bucket come up fine, so every PUT waited 300 s for capacity
    and failed (seen locally: 7 blame tests, 368 s). The script now sets an absolute margin (`-volume.minFreeSpace=100MiB`) and
    asks the master for a file id before handing over, failing within seconds with the master's message (checked by forcing the
    percentage up: exits 2 after 20 s, no process left behind).
* `check_deploy.py` fails if the SeaweedFS release under test differs from the Compose image.
* `docs/operations/postgres-and-s3.md` now has "What is verified against what", names what is **not** verified (Amazon S3, MinIO,
  Ceph, R2; other PostgreSQL majors), and names the two remaining test doubles and why (a scripted local HTTP server for the S3
  client's fault handling; failpoints in the engine).

### Confirmed items (both verifiers), each with a test that fails without the fix

| Item | Fix | Test |
|---|---|---|
| `ag-merge/tree.rs:304` merge_full swallowed `TooManyEntries` (HIGH) | only a file/directory collision falls back to ours; anything else is an error | `tight_limits::*` (fail on the old code, verified), incl. a property test over random trees and limits |
| `ag-merge/lib.rs` docs said renames are undetected | rewritten, three conflict rows added | — |
| `ag-merge` `ALGORITHM_VERSION` copied `ag-diff/1` | explicit literal kept (changing it changes merge ids), guard test | `algorithm_version_embeds_the_diff_version` |
| `ag-types` `principal()` could be `agent:` | names may not start with `#`: **ADR-0009** | `a_name_cannot_start_with_the_run_separator`, proptest |
| `ag-changeset` `ChangesetSource` doc named a nonexistent backend | doc | — |
| `ag-integrate/fsck.rs` doc and `storage.md` overclaimed | verify now checks pins, open-proposal heads and workspace head/base (readable in the object store, *not* "published": proposals and workspaces legitimately point at unpublished changesets, so the verifiers' `is_ready` test would have been a false positive); the docs say what is and is not checked | `fsck_finds_pins_proposals_and_workspaces_that_point_at_nothing` |
| `ag-storage/object_store.rs` staging files never reclaimed | drop guard on every error path, and `reap_tmp` on open under the exclusive store lock | three tests in `object_store::tests` |
| `StoreError::Injected` used for an RNG failure | variant removed; an I/O error that says so | — |
| `ag-storage/lib.rs` named SQLite only | doc | — |
| `meta/jobs.rs` private dedupe lookup (server scanned 1000 jobs) | landed first by the server work package (`job_by_key` public, used for enqueue and for the read-only listing); I added the end-to-end test with 1100 jobs queued after the validation | `asking_again_finds_the_queued_validation_however_many_jobs_came_after_it` (fails with the old scan, verified before the rebase) |

### Contested items, triaged

Fixed: `history.rs` log cap is a named constant (the server work package took the Negotiate limits from `Limits::default()`;
the limits decide changeset validity and are fixed by `object-format.md`, so no config knobs); `Timestamp::now()` no longer maps
a pre-1970 clock to 0; `merge_incremental` hands over only file/directory collisions; the dead `ag-dag` filter is gone; the
outbox/derive docs say what exists (no background dispatcher; `stream.updated` has no consumer; unknown topics must *not* stay
pending, they would crowd `changeset.published` out of the bounded batch); the post-commit derive pass records its failure
(`Repository::derive_status`, cleared by a later success; failpoint test); S3 `request_timeout_secs` / `max_retries` /
`list_page_size` are `storage.objects` keys (validated, additive); an unlock that the server refuses ends the lock session (the
test hangs on the old code, verified); the SQLite per-handle read serialization is documented.

Stays, with the reason:

* `ag-crypto` Windows ACLs: already handled by the Windows track (`private_file.rs`: owner-only ACL via `icacls` at creation,
  tested on the Windows runner). Reading the ACL back is not implemented and `docs/operations/windows.md` says so; `security.md`
  now says it per OS.
* Queue fencing tokens: a token proves the lease *holder* integrates; whoever runs the queue is not that party. A blocked entry
  names the lease; the documented way is a direct `ag integrate --fencing-tokens`. Carrying tokens needs a schema and protocol
  change; documented on `run_queue`.

### Dropped (no longer apply)

* `ag-types/path.rs:10` (reserved-component aliases): implemented by the Windows track (`portable::is_reserved_component`,
  tested). Only `object-format.md` was out of date and is fixed.
* The `description = "AgentGit: ag-*"` placeholders: the manifests already carry real descriptions.

### Found while looking (not in the audit)

* `FsObjectStore::list` / `delete_if_older_than` turned an unreadable mtime into 0, which made garbage collection read the object
  as infinitely old: now an error. (Cannot be provoked on a real file system, so no test.)
* The S3 client signed requests with `unwrap_or(0)` for a failing clock (now the clock's own reading; a skewed clock gets
  `RequestTimeTooSkewed` plus the existing hint) and defaulted a listing entry's missing `Size` to 0 (now an error, test).
* `integrate`'s payload parsing used `unwrap_or_default` for proposal, stream and tokens (now an error naming the operation).
* Magic numbers named: S3 connect timeout, backoff and response limits, the decode pre-allocation cap, the binary-sniff window, the
  revision-suffix cap, `RECOVER_BATCH`, the derive batches, `MAX_CAS_ATTEMPTS`, the GC defaults, the S3 listing page ceiling.

## Evidence

Local (Linux, before pushing):

* `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc` for the
  touched crates: clean. `python3 scripts/check_docs.py`, `python3 scripts/check_deploy.py`: pass.
* `cargo test --workspace --no-fail-fast`: 98 suites ok; the two failures were the shared-target artefacts described under "Not
  verified" and passed once re-run. After the rebase onto the server work package: `cargo test -p ag-server -p ag-storage
  -p ag-integrate -p ag-merge -p ag-types -p ag-dag -p ag-changeset -p ag-diff` and the doc-tests: all ok.
* The CI command of the new step, `AG_REQUIRE_BACKENDS=1 AG_S3_SERVER=seaweedfs scripts/pg-test.sh scripts/s3-test.sh bash -c
  'cargo test -p ag-storage -p ag-integrate && cargo test -p ag-server --test storage_backends'`: exit 0 (PostgreSQL 16 + SeaweedFS
  4.48). `tests/s3.rs` alone: 13 tests ok on SeaweedFS and on moto (the TLS test skips on moto and says so).
* Regression tests shown failing on the old code: `tight_limits::*` (ag-merge), `an_unlock_the_server_refuses_does_not_leave_the_lock_held`
  (hangs), `asking_again_finds_the_queued_validation_...` (2 audit entries instead of 1).

CI for the last code commit `a3ac650` (all green): [CI](https://github.com/Paffin/AgentGit/actions/runs/39101099316) (fmt + clippy +
rustdoc, ubuntu and macOS tests, MSRV 1.89, **PostgreSQL + S3 backends** with both S3 steps, docs + demo, deployment files, SDKs,
orchestrator), [Windows](https://github.com/Paffin/AgentGit/actions/runs/39101099287),
[Containers](https://github.com/Paffin/AgentGit/actions/runs/39101099298), [Packaging](https://github.com/Paffin/AgentGit/actions/runs/39101099288),
[Web UI](https://github.com/Paffin/AgentGit/actions/runs/39101099292). In the backends job the step *Test on PostgreSQL + S3 (SeaweedFS, a
real S3 server)* ran for 1 min 45 s after the moto step.

## Not verified

* Amazon S3, MinIO, Ceph RGW, R2; PostgreSQL other than 16; the SeaweedFS leg on macOS and arm64 (the script pins hashes for all
  four release assets, but only linux/amd64 was downloaded and run).
* The Windows and macOS legs of the new storage tests (`reap_tmp`, `a_failed_put_leaves_no_staging_file`): see CI for what ran.
* `derive_status` and the new `dangling_*` kinds are not surfaced by `ag doctor`, `/readyz` or metrics (other tracks' crates).
* An unreadable file modification time cannot be provoked on a real file system; that fix has no test.
* The local full-workspace run had two failures that are artefacts of the shared cargo target directory (a stale `ag` build-script
  commit hash and a stale `ag-server` binary used by `ag-cli --test version` and `ag-server --test console`); both pass after the
  build script is re-run. CI builds from scratch.

## Next steps

1. Surface `Repository::derive_status()` in `ag doctor` and as a server metric / health detail.
2. If queue integration of leased paths is wanted: store fencing tokens with the queue entry (additive migration, proto field, CLI flag).
3. A read-back of the key-file ACL on Windows.
4. The GC defaults (`24h`, `90d`) now have one home in `ag-integrate` (`DEFAULT_GC_GRACE_MS`, `DEFAULT_REFLOG_RETENTION_MS`), but the
   CLI's argument parser (`ag-cli/src/cmds/admin.rs`) still spells them as strings.
5. `ObjectsConfig::S3` is a struct variant with one field per key: every new key breaks the literals in other crates' tests. A
   builder or `Default` for the S3 settings would end that.
