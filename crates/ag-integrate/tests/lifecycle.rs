//! Garbage collection, integrity checking, derived data, workspaces and the queue.
#![allow(missing_docs)]

mod common;

use std::fs;
use std::path::Path;

use ag_integrate::{GcOptions, IntegrateRequest, Outcome, Repository, SyncOutcome};
use ag_storage::meta::{IntentState, Param};
use ag_types::{Identity, StreamName};
use common::*;

const HOUR: i64 = 3_600_000;

/// A repository on disk (`.ag/config.json` and keys) over the backends the environment selects.
fn disk_repo(dir: &TestDir) -> Repository {
    init_repo(dir)
}

fn write(root: &Path, rel: &str, data: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, data).unwrap();
}

// ---- garbage collection ----------------------------------------------------

#[test]
fn gc_deletes_only_unreachable_objects_older_than_the_grace_period() {
    let f = fx();
    let landed = f.land(None, &[("keep.txt", "reachable")]);
    // an abandoned snapshot: stored, never proposed, not in any workspace
    let orphan = f.snap(None, &[("orphan.txt", "garbage")], "user:alice");
    let orphan_blob = f.repo.changeset(&orphan).unwrap().blobs().next().unwrap();

    // too young: kept
    let rep = f.repo.gc(&GcOptions { grace_ms: HOUR, ..GcOptions::default() }, "user:alice").unwrap();
    assert!(rep.deleted.is_empty());
    assert!(rep.kept_young >= 2);

    // past the grace period: removed, reachable data untouched
    f.clock.advance_ms(2 * HOUR);
    let dry = f.repo.gc(&GcOptions { grace_ms: HOUR, dry_run: true, ..GcOptions::default() }, "user:alice").unwrap();
    assert_eq!(dry.deleted.len(), 2, "orphan changeset + orphan blob");
    assert!(f.repo.objects().contains(&orphan.object_id()).unwrap(), "dry run deletes nothing");

    let rep = f.repo.gc(&GcOptions { grace_ms: HOUR, ..GcOptions::default() }, "user:alice").unwrap();
    assert_eq!(rep.deleted.len(), 2);
    assert!(!f.repo.objects().contains(&orphan.object_id()).unwrap());
    assert!(!f.repo.objects().contains(&orphan_blob.object_id()).unwrap());
    assert_eq!(f.read(&landed, "keep.txt").as_deref(), Some("reachable"));
    assert!(f.repo.fsck().unwrap().is_ok());
}

#[test]
fn gc_keeps_everything_a_root_can_reach_including_open_proposals_pins_and_workspaces() {
    let f = fx();
    let base = f.land(None, &[("a", "1")]);
    let open = f.snap(Some(base), &[("a", "1"), ("open", "x")], "user:bob");
    f.propose(open, "open proposal", "user:bob");
    let pinned = f.snap(Some(base), &[("a", "1"), ("pinned", "y")], "user:bob");
    f.repo.pin("release-1", &pinned, "keep", &id("user:alice")).unwrap();
    let ws_snap = f.snap(Some(base), &[("a", "1"), ("ws", "z")], "user:bob");
    f.repo.meta().write(|tx| tx.workspace_update("default", Some(&base), Some(&ws_snap))).unwrap();
    let loose = f.snap(Some(base), &[("a", "1"), ("loose", "w")], "user:bob");

    f.clock.advance_ms(48 * HOUR);
    let rep = f.repo.gc(&GcOptions { grace_ms: HOUR, ..GcOptions::default() }, "user:alice").unwrap();
    for (name, c) in [("open proposal", open), ("pinned", pinned), ("workspace", ws_snap), ("stream head", base)] {
        assert!(f.repo.objects().contains(&c.object_id()).unwrap(), "{name} must survive");
    }
    assert!(!f.repo.objects().contains(&loose.object_id()).unwrap(), "only the loose snapshot goes");
    assert!(rep.deleted.contains(&loose.to_hex()));
    // unpinning makes it collectable
    f.repo.unpin("release-1", &id("user:alice")).unwrap();
    f.repo.gc(&GcOptions { grace_ms: HOUR, ..GcOptions::default() }, "user:alice").unwrap();
    assert!(!f.repo.objects().contains(&pinned.object_id()).unwrap());
}

#[test]
fn gc_protects_objects_staged_by_an_in_flight_publication() {
    let f = fx();
    let c = f.snap(None, &[("x", "staged")], "user:alice");
    let blob = f.repo.changeset(&c).unwrap().blobs().next().unwrap();
    f.repo.failpoints().arm("publish.after_upload");
    assert!(f.repo.publish(&c, &id("user:alice"), None).is_err(), "crashed after upload");

    f.clock.advance_ms(48 * HOUR); // way past the grace period for the object ages
    // the intent is still non-terminal, so its objects are protected
    let rep = f.repo.gc(&GcOptions { grace_ms: HOUR, ..GcOptions::default() }, "user:alice").unwrap();
    assert!(rep.deleted.is_empty(), "{rep:?}");
    assert!(rep.kept_staged >= 2);
    assert!(f.repo.objects().contains(&blob.object_id()).unwrap());

    // recovery completes the publication; the data is intact
    let r = f.repo.recover(100 * HOUR).unwrap();
    assert!(r.errors.is_empty());
    assert!(f.repo.meta().read(|tx| tx.is_ready(&c)).unwrap());
}

#[test]
fn rewriting_an_old_unreachable_object_saves_it_from_a_concurrent_gc() {
    let f = fx();
    if f.wallclock_objects() {
        // S3 objects age on the server's clock, which a test cannot move forward; the same
        // guarantee is tested against S3 with real time in
        // `rewriting_an_old_unreachable_object_saves_it_from_a_concurrent_gc_on_s3`.
        return;
    }
    let blob = f.repo.put_blob(b"old but suddenly wanted".to_vec()).unwrap();
    f.clock.advance_ms(48 * HOUR);
    // A writer is about to reference the blob: putting it again refreshes its age.
    let again = f.repo.put_blob(b"old but suddenly wanted".to_vec()).unwrap();
    assert_eq!(again, blob);
    let rep = f.repo.gc(&GcOptions { grace_ms: HOUR, ..GcOptions::default() }, "user:alice").unwrap();
    assert!(rep.deleted.is_empty());
    assert!(f.repo.objects().contains(&blob.object_id()).unwrap());
}

#[test]
fn rewriting_an_old_unreachable_object_saves_it_from_a_concurrent_gc_on_s3() {
    // The S3 counterpart, in real time: objects age on the server's clock. The collector's
    // delete is serialized with writers by the repository's object lock (see
    // `s3_gc_cannot_delete_an_object_a_writer_just_reput` in ag-storage for the interleaving).
    if ag_storage::testing::s3_endpoint().is_none() {
        eprintln!("skipped: AG_TEST_S3_ENDPOINT is not set (use scripts/s3-test.sh)");
        return;
    }
    let dir = tempdir();
    let mut storage = storage_for_tests(&dir).expect("S3 is configured");
    storage.meta = ag_integrate::MetaConfig::Sqlite; // metadata is not what is tested here
    let repo = init_repo_with(&dir, Some(storage));
    assert!(objects_are_s3(&repo));
    let wanted = repo.put_blob(b"old but suddenly wanted".to_vec()).unwrap();
    let abandoned = repo.put_blob(b"abandoned".to_vec()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2300));

    // A writer is about to reference the first blob: putting it again refreshes its age.
    assert_eq!(repo.put_blob(b"old but suddenly wanted".to_vec()).unwrap(), wanted);
    let rep = repo.gc(&GcOptions { grace_ms: 1000, ..GcOptions::default() }, "user:alice").unwrap();
    assert_eq!(rep.deleted, vec![abandoned.object_id().to_hex()], "only the object nobody touched is collected");
    assert!(repo.objects().contains(&wanted.object_id()).unwrap());
    assert!(!repo.objects().contains(&abandoned.object_id()).unwrap());
    assert!(repo.fsck().unwrap().is_ok());
}

#[test]
fn gc_fails_closed_when_a_reachable_object_is_damaged() {
    let dir = tempdir();
    let repo = disk_repo(&dir);
    let c = snap_in(&repo, None, &[("a", "precious")], "user:alice");
    let p = repo.propose("main", &c, "t", "", &id("user:alice"), None).unwrap();
    repo.integrate(&IntegrateRequest {
        proposal_id: p.id,
        actor: id("user:alice"),
        idempotency_key: None,
        fencing_tokens: vec![],
    })
    .unwrap();
    let orphan = snap_in(&repo, None, &[("junk", "junk")], "user:alice");

    // damage the reachable blob
    let blob = repo.changeset(&c).unwrap().blobs().next().unwrap();
    damage_object(&repo, &blob.to_hex(), b"AGOB\x01\x00\x01garbage");

    let err = repo.gc(&GcOptions { grace_ms: 0, ..GcOptions::default() }, "user:alice").unwrap_err();
    assert!(err.to_string().contains("aborted"), "{err}");
    assert!(repo.objects().contains(&orphan.object_id()).unwrap(), "nothing may be deleted when the mark phase failed");
}

// ---- fsck, tampering, derived data ------------------------------------------

#[test]
fn fsck_reports_a_healthy_repository_and_finds_each_kind_of_damage() {
    let dir = tempdir();
    let repo = disk_repo(&dir);
    let c1 = snap_in(&repo, None, &[("a", "1"), ("b", "2")], "user:alice");
    let p1 = repo.propose("main", &c1, "t", "", &id("user:alice"), None).unwrap();
    repo.integrate(&IntegrateRequest {
        proposal_id: p1.id,
        actor: id("user:alice"),
        idempotency_key: None,
        fencing_tokens: vec![],
    })
    .unwrap();
    let ok = repo.fsck().unwrap();
    assert!(ok.is_ok(), "{:?}", ok.issues);
    assert!(ok.objects_checked >= 3 && ok.audit_entries > 3);

    // 1. a damaged blob
    let blob = repo.changeset(&c1).unwrap().blobs().next().unwrap();
    damage_object(&repo, &blob.to_hex(), b"AGOB\x01\x00\x01garbage");
    let rep = repo.fsck().unwrap();
    assert!(rep.issues.iter().any(|i| i.kind == "corrupt_object"), "{:?}", rep.issues);
    assert!(rep.issues.iter().any(|i| i.kind == "bad_blob"));
    // a damaged blob is also an error for readers, never silently wrong data
    assert!(repo.blob(&blob).is_err());

    // 2. repair by re-publishing the same content
    repo.put_blob(bytes_of("1", "2", &blob, &repo)).unwrap();
    assert!(repo.fsck().unwrap().is_ok());

    // 3. someone edits the audit log directly in the database
    repo.meta().write(|tx| tx.raw_execute("UPDATE audit SET actor = 'user:mallory' WHERE seq = 2", &[])).unwrap();
    drop(repo);
    let repo = Repository::open(dir.path()).unwrap();
    let rep = repo.fsck().unwrap();
    assert!(rep.issues.iter().any(|i| i.kind == "audit_chain_broken"), "{:?}", rep.issues);
}

fn bytes_of(a: &str, b: &str, blob: &ag_types::BlobId, repo: &Repository) -> Vec<u8> {
    // The blob in the fixture is "1" or "2"; find which one hashes to `blob`.
    for s in [a, b] {
        if ag_object::hash_blob(s.as_bytes()) == *blob {
            return s.as_bytes().to_vec();
        }
    }
    let _ = repo;
    panic!("unknown blob");
}

#[test]
fn tampering_with_stored_attestations_or_streams_is_detected_or_harmless() {
    let dir = tempdir();
    let repo = disk_repo(&dir);
    let c = snap_in(&repo, None, &[("a", "1")], "user:alice");
    let p = repo.propose("main", &c, "t", "", &id("user:alice"), None).unwrap();
    repo.integrate(&IntegrateRequest {
        proposal_id: p.id,
        actor: id("user:alice"),
        idempotency_key: None,
        fencing_tokens: vec![],
    })
    .unwrap();

    // Point the stream at a changeset that was never published.
    let ghost = snap_in(&repo, Some(c), &[("a", "1"), ("ghost", "g")], "user:alice");
    repo.meta()
        .write(|tx| tx.raw_execute("UPDATE streams SET head = ?1 WHERE name = 'main'", &[Param::from(ghost.to_hex())]))
        .unwrap();
    drop(repo);
    let repo = Repository::open(dir.path()).unwrap();
    let rep = repo.fsck().unwrap();
    assert!(rep.issues.iter().any(|i| i.kind == "stream_head_unpublished"), "{:?}", rep.issues);
    assert!(rep.issues.iter().any(|i| i.kind == "reflog_mismatch"), "{:?}", rep.issues);
}

#[test]
fn derived_trees_are_disposable_and_reindex_rebuilds_them() {
    let dir = tempdir();
    let repo = disk_repo(&dir);
    let c1 = snap_in(&repo, None, &[("a", "1")], "user:alice");
    let c2 = snap_in(&repo, Some(c1), &[("a", "1"), ("b", "2")], "user:alice");
    let p = repo.propose("main", &c2, "t", "", &id("user:alice"), None).unwrap();
    repo.integrate(&IntegrateRequest {
        proposal_id: p.id,
        actor: id("user:alice"),
        idempotency_key: None,
        fencing_tokens: vec![],
    })
    .unwrap();
    assert!(repo.meta().read(|tx| tx.derived_tree(&c2)).unwrap().is_some(), "worker derived the tree");
    assert_eq!(repo.meta().read(|tx| tx.outbox_pending_count()).unwrap(), 0);

    // wipe all derived data: reads still work (recomputed from history)
    let expected = repo.tree_of(&c2).unwrap().id();
    repo.meta().write(|tx| tx.clear_derived()).unwrap();
    assert!(repo.meta().read(|tx| tx.derived_tree(&c2)).unwrap().is_none());
    assert_eq!(repo.tree_of(&c2).unwrap().id(), expected);

    // corrupt a cached manifest: ignored, never returned
    assert_eq!(repo.reindex().unwrap(), 2);
    repo.meta().write(|tx| tx.raw_execute("UPDATE trees SET data = ?1", &[Param::from(vec![1u8])])).unwrap();
    drop(repo);
    let repo = Repository::open(dir.path()).unwrap();
    assert_eq!(repo.tree_of(&c2).unwrap().id(), expected, "bad cache is bypassed");
    assert_eq!(repo.reindex().unwrap(), 2);
    assert!(repo.fsck().unwrap().is_ok());
}

// ---- workspaces ----------------------------------------------------------------

fn alice() -> Identity {
    id("user:alice")
}

fn land_dir(repo: &Repository, dir: &Path, msg: &str) -> ag_types::ChangesetId {
    let head = repo.snapshot("default", dir, msg, &alice(), false).unwrap().expect("something changed");
    let prop = repo.propose("main", &head, msg, "", &alice(), None).unwrap();
    let r = repo
        .integrate(&IntegrateRequest {
            proposal_id: prop.id,
            actor: alice(),
            idempotency_key: None,
            fencing_tokens: vec![],
        })
        .unwrap();
    assert!(matches!(r.outcome, Outcome::Integrated { .. }), "{:?}", r.outcome);
    head
}

#[test]
fn workspace_snapshot_propose_integrate_cycle() {
    let dir = tempdir();
    let repo = disk_repo(&dir);
    write(dir.path(), "README.md", "# hi\n");
    write(dir.path(), "src/main.rs", "fn main() {}\n");
    assert_eq!(repo.workspace_status("default", dir.path()).unwrap().len(), 2);

    let first = land_dir(&repo, dir.path(), "initial");
    assert_eq!(repo.workspace_status("default", dir.path()).unwrap(), vec![]);
    assert!(
        repo.snapshot("default", dir.path(), "again", &alice(), false).unwrap().is_none(),
        "no changes, no snapshot"
    );

    write(dir.path(), "src/main.rs", "fn main() { println!(\"hi\"); }\n");
    fs::remove_file(dir.path().join("README.md")).unwrap();
    let st = repo.workspace_status("default", dir.path()).unwrap();
    assert_eq!(
        st.iter().map(|c| (c.kind, c.path.as_str())).collect::<Vec<_>>(),
        [("deleted", "README.md"), ("modified", "src/main.rs")]
    );
    let second = land_dir(&repo, dir.path(), "edit");
    assert_eq!(repo.changeset(&second).unwrap().parents, vec![first]);
    assert_eq!(repo.resolve_rev("main").unwrap(), second);
    assert_eq!(repo.resolve_rev("main~1").unwrap(), first);
    assert_eq!(repo.resolve_rev("@").unwrap(), second);
    assert_eq!(repo.resolve_rev(&second.to_hex()[..10]).unwrap(), second);
    let log = repo.log(&second, 10, Some("README.md")).unwrap();
    assert_eq!(log.len(), 2, "README.md was added and then deleted");
}

#[test]
fn sync_fast_forwards_merges_and_resolves_conflicts() {
    let main_dir = tempdir();
    let repo = disk_repo(&main_dir);
    write(main_dir.path(), "a.txt", "line1\nline2\nline3\nline4\nline5\n");
    write(main_dir.path(), "b.txt", "b\n");
    let base = land_dir(&repo, main_dir.path(), "base");

    // A second workspace in a separate directory, starting from the stream head.
    let other = tempfile::tempdir().unwrap();
    repo.workspace_create("other", &StreamName::new("main").unwrap(), &id("user:bob")).unwrap();
    repo.workspace_checkout("other", other.path()).unwrap();
    assert!(repo.workspace_status("other", other.path()).unwrap().is_empty());

    // Stream moves ahead via the main workspace.
    write(main_dir.path(), "a.txt", "LINE1\nline2\nline3\nline4\nline5\n");
    let c1 = land_dir(&repo, main_dir.path(), "edit top");

    // other has no local work: sync fast-forwards it.
    assert_eq!(
        repo.sync_workspace("other", other.path(), &id("user:bob")).unwrap(),
        SyncOutcome::FastForwarded { to: c1 }
    );
    assert_eq!(fs::read_to_string(other.path().join("a.txt")).unwrap(), "LINE1\nline2\nline3\nline4\nline5\n");

    // other makes a non-conflicting local change; main moves again.
    write(other.path(), "a.txt", "LINE1\nline2\nline3\nline4\nLINE5-other\n");
    repo.snapshot("other", other.path(), "other edits bottom", &id("user:bob"), false).unwrap().unwrap();
    write(main_dir.path(), "b.txt", "b changed\n");
    land_dir(&repo, main_dir.path(), "edit b");
    match repo.sync_workspace("other", other.path(), &id("user:bob")).unwrap() {
        SyncOutcome::Merged { id } => assert_eq!(repo.changeset(&id).unwrap().parents.len(), 2),
        o => panic!("{o:?}"),
    }
    assert_eq!(fs::read_to_string(other.path().join("b.txt")).unwrap(), "b changed\n");
    assert_eq!(fs::read_to_string(other.path().join("a.txt")).unwrap(), "LINE1\nline2\nline3\nline4\nLINE5-other\n");

    // Now both edit the same line.
    write(other.path(), "a.txt", "LINE1\nline2\nline3\nline4\nCONFLICT-other\n");
    repo.snapshot("other", other.path(), "other edits line 5", &id("user:bob"), false).unwrap().unwrap();
    write(main_dir.path(), "a.txt", "LINE1\nline2\nline3\nline4\nCONFLICT-main\n");
    land_dir(&repo, main_dir.path(), "main edits line 5");

    let out = repo.sync_workspace("other", other.path(), &id("user:bob")).unwrap();
    let SyncOutcome::Conflicted { conflicts } = out else { panic!("{out:?}") };
    assert_eq!(conflicts[0].path, "a.txt");
    let marked = fs::read_to_string(other.path().join("a.txt")).unwrap();
    assert!(marked.contains("<<<<<<< ours\nCONFLICT-other\n=======\nCONFLICT-main\n>>>>>>> theirs"), "{marked}");
    assert!(repo.workspace_is_merging("other").unwrap());

    // Snapshotting with markers still present is refused; resolving it works.
    assert!(repo.snapshot("other", other.path(), "oops", &id("user:bob"), false).is_err());
    assert!(repo.sync_workspace("other", other.path(), &id("user:bob")).is_err(), "must resolve first");
    write(other.path(), "a.txt", "LINE1\nline2\nline3\nline4\nCONFLICT-resolved\n");
    let resolved = repo.snapshot("other", other.path(), "resolve", &id("user:bob"), false).unwrap().unwrap();
    assert_eq!(repo.changeset(&resolved).unwrap().parents.len(), 2, "the resolution is a merge changeset");
    assert!(!repo.workspace_is_merging("other").unwrap());

    // The resolved merge fast-forwards the stream.
    let prop = repo.propose("main", &resolved, "resolution", "", &id("user:bob"), None).unwrap();
    let r = repo
        .integrate(&IntegrateRequest {
            proposal_id: prop.id,
            actor: alice(),
            idempotency_key: None,
            fencing_tokens: vec![],
        })
        .unwrap();
    let Outcome::Integrated { strategy, .. } = r.outcome else { panic!("{:?}", r.outcome) };
    assert_eq!(strategy, ag_integrate::Strategy::FastForward);
    let _ = base;
}

// ---- queue ----------------------------------------------------------------------

#[test]
fn queue_integrates_in_order_and_isolates_failures() {
    let f = fx();
    let base = f.land(None, &[("a", "1\n2\n3\n"), ("b", "b\n"), ("c", "c\n")]);
    let ca = f.snap(Some(base), &[("a", "A\n2\n3\n"), ("b", "b\n"), ("c", "c\n")], "user:bob");
    let cb = f.snap(Some(base), &[("a", "1\n2\n3\n"), ("b", "B\n"), ("c", "c\n")], "user:carol");
    let cconflict = f.snap(Some(base), &[("a", "X\n2\n3\n"), ("b", "b\n"), ("c", "c\n")], "user:dave");
    let cc = f.snap(Some(base), &[("a", "1\n2\n3\n"), ("b", "b\n"), ("c", "C\n")], "user:erin");
    let pa = f.propose(ca, "a", "user:bob");
    let pb = f.propose(cb, "b", "user:carol");
    let px = f.propose(cconflict, "conflicts with a", "user:dave");
    let pc = f.propose(cc, "c", "user:erin");
    for p in [&pa, &px, &pb, &pc] {
        f.repo.enqueue(p, &id("user:alice")).unwrap();
    }
    assert!(f.repo.enqueue(&pa, &id("user:alice")).is_err(), "no duplicates while queued");

    let rep = f.repo.run_queue("main", &id("user:alice")).unwrap();
    assert_eq!(rep.integrated, vec![pa.clone(), pb.clone(), pc.clone()]);
    assert_eq!(rep.rejected, vec![px.clone()]);
    assert!(rep.blocked.is_empty() && rep.failed.is_empty());

    let head = f.head().unwrap();
    assert_eq!(f.read(&head, "a").as_deref(), Some("A\n2\n3\n"));
    assert_eq!(f.read(&head, "b").as_deref(), Some("B\n"));
    assert_eq!(f.read(&head, "c").as_deref(), Some("C\n"));
    let states: Vec<_> = f.repo.queue("main").unwrap().iter().map(|q| q.state.clone()).collect();
    assert_eq!(states, ["done", "rejected", "done", "done"]);
    assert_eq!(f.repo.operation_by_key(&format!("integrate:{px}:{cconflict}:0")).unwrap().state, IntentState::Rejected);
}

#[test]
fn derived_tree_cache_is_sparse_but_every_tree_is_still_readable() {
    let f = fx();
    // a linear history of 100 changesets, each adding one file
    let mut prev = None;
    let mut ids = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();
    for i in 0..100 {
        files.push((format!("f{i:03}.txt"), format!("content {i}")));
        let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let c = f.snap(prev, &refs, "user:alice");
        ids.push(c);
        prev = Some(c);
    }
    f.repo.publish(&prev.unwrap(), &id("user:alice"), None).unwrap();
    while f.repo.derive_pending(1000).unwrap().pending > 0 {}

    let cached = f.repo.meta().read(|tx| tx.count_rows("changeset_trees")).unwrap();
    let keyframes = 100 / ag_integrate::KEYFRAME_INTERVAL;
    assert!(
        cached <= keyframes + 1,
        "cache holds ~one tree per {} changesets, got {cached}",
        ag_integrate::KEYFRAME_INTERVAL
    );
    assert!(cached >= keyframes, "keyframes are cached: {cached}");

    // every tree is still available, computed from the nearest keyframe
    for (n, c) in ids.iter().enumerate() {
        assert_eq!(f.repo.tree_of(c).unwrap().len(), n + 1);
    }
    // and rebuilding from scratch yields an equivalent, equally sparse cache
    f.repo.reindex().unwrap();
    assert!(f.repo.meta().read(|tx| tx.count_rows("changeset_trees")).unwrap() <= keyframes + 1);
    assert!(f.repo.fsck().unwrap().is_ok());
}

/// Backs `repo` (a repository rooted in `dir`) up, lets the original move on, and checks that
/// the backup is a frozen, verified, self-contained and usable *local* repository.
fn check_backup(repo: &Repository, dir: &TestDir) {
    write(dir.path(), "a.txt", "one\n");
    let first = land_dir(repo, dir.path(), "first");
    repo.pin("keep", &first, "release", &alice()).unwrap();

    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("backup");
    let rep = repo.backup(&dest, false).unwrap();
    assert!(rep.verification.is_ok(), "{:?}", rep.verification.issues);
    assert_eq!(rep.objects, 2, "one blob and one changeset");
    assert!(!rep.keys_included);
    assert!(repo.backup(&dest, false).is_err(), "never overwrite an existing backup");

    // The original moves on; the backup is a frozen, self-contained repository.
    write(dir.path(), "a.txt", "two\n");
    land_dir(repo, dir.path(), "second");
    let restored = Repository::open(&dest).unwrap();
    assert!(restored.config().storage.is_none(), "a backup is a plain local repository");
    assert_eq!(restored.resolve_rev("main").unwrap(), first);
    assert_eq!(restored.pins().unwrap().len(), 1);
    assert!(restored.fsck().unwrap().is_ok());
    let tree = restored.tree_of(&first).unwrap();
    let entry = tree.get(&ag_types::RepoPath::new("a.txt").unwrap()).unwrap();
    assert_eq!(restored.blob(&entry.content).unwrap(), b"one\n");
    // and it is fully usable: it can take new work
    write(&dest, "b.txt", "from the restored copy\n");
    let head = restored.snapshot("default", &dest, "continue", &alice(), false).unwrap().unwrap();
    assert_eq!(restored.changeset(&head).unwrap().parents, vec![first]);
}

#[test]
fn backup_is_consistent_verified_and_independent_of_later_changes() {
    let dir = tempdir();
    let repo = disk_repo(&dir);
    if meta_is_postgres(&repo) {
        // `ag backup` does not half-work on PostgreSQL metadata: it refuses, says what to do
        // instead, and writes nothing.
        write(dir.path(), "a.txt", "one\n");
        land_dir(&repo, dir.path(), "first");
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("backup");
        let err = repo.backup(&dest, false).unwrap_err().to_string();
        assert!(err.contains("pg_dump") && err.contains("ag verify") && err.contains("PostgreSQL"), "{err}");
        assert!(!dest.exists(), "nothing was written");
        return;
    }
    check_backup(&repo, &dir);
}

#[test]
fn backup_downloads_s3_objects_into_a_self_contained_local_repository() {
    if ag_storage::testing::s3_endpoint().is_none() {
        eprintln!("skipped: AG_TEST_S3_ENDPOINT is not set (use scripts/s3-test.sh)");
        return;
    }
    let dir = tempdir();
    let mut storage = storage_for_tests(&dir).expect("S3 is configured");
    storage.meta = ag_integrate::MetaConfig::Sqlite; // PostgreSQL metadata cannot be backed up this way
    let repo = init_repo_with(&dir, Some(storage));
    assert!(objects_are_s3(&repo));
    assert!(!repo.layout().objects_dir().join("objects").exists(), "no objects are kept locally");
    check_backup(&repo, &dir);
}
