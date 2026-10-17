//! Garbage collection, integrity checking, derived data, workspaces and the queue.
#![allow(missing_docs)]

mod common;

use std::fs;
use std::path::Path;

use ag_integrate::{GcOptions, InitOptions, IntegrateRequest, Outcome, Repository, SyncOutcome};
use ag_storage::meta::IntentState;
use ag_types::{Identity, Namespace, StreamName};
use common::*;

const HOUR: i64 = 3_600_000;

fn disk_repo(dir: &Path) -> Repository {
    Repository::init(
        dir,
        &InitOptions {
            namespace: Namespace::new("acme/app").unwrap(),
            default_stream: StreamName::new("main").unwrap(),
            actor: id("user:alice"),
        },
    )
    .unwrap()
}

fn write(root: &Path, rel: &str, data: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, data).unwrap();
}

fn object_path(repo: &Repository, hex: &str) -> std::path::PathBuf {
    repo.layout().objects_dir().join("objects").join(&hex[..2]).join(&hex[2..])
}

fn make_writable(p: &Path) {
    let mut perms = fs::metadata(p).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(p, perms).unwrap();
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
fn gc_fails_closed_when_a_reachable_object_is_damaged() {
    let dir = tempfile::tempdir().unwrap();
    let repo = disk_repo(dir.path());
    let c = snap_in(&repo, None, &[("a", "precious")], "user:alice");
    let p = repo.propose("main", &c, "t", "", &id("user:alice"), None).unwrap();
    repo.integrate(&IntegrateRequest { proposal_id: p.id, actor: id("user:alice"), idempotency_key: None, fencing_tokens: vec![] }).unwrap();
    let orphan = snap_in(&repo, None, &[("junk", "junk")], "user:alice");

    // damage the reachable blob
    let blob = repo.changeset(&c).unwrap().blobs().next().unwrap();
    let path = object_path(&repo, &blob.to_hex());
    make_writable(&path);
    fs::write(&path, b"AGOB\x01\x00\x01garbage").unwrap();

    let err = repo.gc(&GcOptions { grace_ms: 0, ..GcOptions::default() }, "user:alice").unwrap_err();
    assert!(err.to_string().contains("aborted"), "{err}");
    assert!(repo.objects().contains(&orphan.object_id()).unwrap(), "nothing may be deleted when the mark phase failed");
}

// ---- fsck, tampering, derived data ------------------------------------------

#[test]
fn fsck_reports_a_healthy_repository_and_finds_each_kind_of_damage() {
    let dir = tempfile::tempdir().unwrap();
    let repo = disk_repo(dir.path());
    let c1 = snap_in(&repo, None, &[("a", "1"), ("b", "2")], "user:alice");
    let p1 = repo.propose("main", &c1, "t", "", &id("user:alice"), None).unwrap();
    repo.integrate(&IntegrateRequest { proposal_id: p1.id, actor: id("user:alice"), idempotency_key: None, fencing_tokens: vec![] }).unwrap();
    let ok = repo.fsck().unwrap();
    assert!(ok.is_ok(), "{:?}", ok.issues);
    assert!(ok.objects_checked >= 3 && ok.audit_entries > 3);

    // 1. a flipped byte in a blob
    let blob = repo.changeset(&c1).unwrap().blobs().next().unwrap();
    let path = object_path(&repo, &blob.to_hex());
    make_writable(&path);
    let mut bytes = fs::read(&path).unwrap();
    *bytes.last_mut().unwrap() ^= 0xff;
    fs::write(&path, &bytes).unwrap();
    let rep = repo.fsck().unwrap();
    assert!(rep.issues.iter().any(|i| i.kind == "corrupt_object"), "{:?}", rep.issues);
    assert!(rep.issues.iter().any(|i| i.kind == "bad_blob"));
    // a damaged blob is also an error for readers, never silently wrong data
    assert!(repo.blob(&blob).is_err());

    // 2. repair by re-publishing the same content
    repo.put_blob(bytes_of("1", "2", &blob, &repo)).unwrap();
    assert!(repo.fsck().unwrap().is_ok());

    // 3. someone edits the audit log directly in the database
    drop(repo);
    let conn = rusqlite::Connection::open(dir.path().join(".ag/meta.db")).unwrap();
    conn.execute("UPDATE audit SET actor = 'user:mallory' WHERE seq = 2", []).unwrap();
    drop(conn);
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
    let dir = tempfile::tempdir().unwrap();
    let repo = disk_repo(dir.path());
    let c = snap_in(&repo, None, &[("a", "1")], "user:alice");
    let p = repo.propose("main", &c, "t", "", &id("user:alice"), None).unwrap();
    repo.integrate(&IntegrateRequest { proposal_id: p.id, actor: id("user:alice"), idempotency_key: None, fencing_tokens: vec![] }).unwrap();

    // Point the stream at a changeset that was never published.
    let ghost = snap_in(&repo, Some(c), &[("a", "1"), ("ghost", "g")], "user:alice");
    drop(repo);
    let conn = rusqlite::Connection::open(dir.path().join(".ag/meta.db")).unwrap();
    conn.execute("UPDATE streams SET head = ?1 WHERE name = 'main'", [ghost.to_hex()]).unwrap();
    drop(conn);
    let repo = Repository::open(dir.path()).unwrap();
    let rep = repo.fsck().unwrap();
    assert!(rep.issues.iter().any(|i| i.kind == "stream_head_unpublished"), "{:?}", rep.issues);
    assert!(rep.issues.iter().any(|i| i.kind == "reflog_mismatch"), "{:?}", rep.issues);
}

#[test]
fn derived_trees_are_disposable_and_reindex_rebuilds_them() {
    let dir = tempfile::tempdir().unwrap();
    let repo = disk_repo(dir.path());
    let c1 = snap_in(&repo, None, &[("a", "1")], "user:alice");
    let c2 = snap_in(&repo, Some(c1), &[("a", "1"), ("b", "2")], "user:alice");
    let p = repo.propose("main", &c2, "t", "", &id("user:alice"), None).unwrap();
    repo.integrate(&IntegrateRequest { proposal_id: p.id, actor: id("user:alice"), idempotency_key: None, fencing_tokens: vec![] }).unwrap();
    assert!(repo.meta().read(|tx| tx.derived_tree(&c2)).unwrap().is_some(), "worker derived the tree");
    assert_eq!(repo.meta().read(|tx| tx.outbox_pending_count()).unwrap(), 0);

    // wipe all derived data: reads still work (recomputed from history)
    let expected = repo.tree_of(&c2).unwrap().id();
    repo.meta().write(|tx| tx.clear_derived()).unwrap();
    assert!(repo.meta().read(|tx| tx.derived_tree(&c2)).unwrap().is_none());
    assert_eq!(repo.tree_of(&c2).unwrap().id(), expected);

    // corrupt a cached manifest: ignored, never returned
    assert_eq!(repo.reindex().unwrap(), 2);
    drop(repo);
    let conn = rusqlite::Connection::open(dir.path().join(".ag/meta.db")).unwrap();
    conn.execute("UPDATE trees SET data = x'01'", []).unwrap();
    drop(conn);
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
    let r = repo.integrate(&IntegrateRequest { proposal_id: prop.id, actor: alice(), idempotency_key: None, fencing_tokens: vec![] }).unwrap();
    assert!(matches!(r.outcome, Outcome::Integrated { .. }), "{:?}", r.outcome);
    head
}

#[test]
fn workspace_snapshot_propose_integrate_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let repo = disk_repo(dir.path());
    write(dir.path(), "README.md", "# hi\n");
    write(dir.path(), "src/main.rs", "fn main() {}\n");
    assert_eq!(repo.workspace_status("default", dir.path()).unwrap().len(), 2);

    let first = land_dir(&repo, dir.path(), "initial");
    assert_eq!(repo.workspace_status("default", dir.path()).unwrap(), vec![]);
    assert!(repo.snapshot("default", dir.path(), "again", &alice(), false).unwrap().is_none(), "no changes, no snapshot");

    write(dir.path(), "src/main.rs", "fn main() { println!(\"hi\"); }\n");
    fs::remove_file(dir.path().join("README.md")).unwrap();
    let st = repo.workspace_status("default", dir.path()).unwrap();
    assert_eq!(st.iter().map(|c| (c.kind, c.path.as_str())).collect::<Vec<_>>(), [("deleted", "README.md"), ("modified", "src/main.rs")]);
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
    let main_dir = tempfile::tempdir().unwrap();
    let repo = disk_repo(main_dir.path());
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
    assert_eq!(repo.sync_workspace("other", other.path(), &id("user:bob")).unwrap(), SyncOutcome::FastForwarded { to: c1 });
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
    let r = repo.integrate(&IntegrateRequest { proposal_id: prop.id, actor: alice(), idempotency_key: None, fencing_tokens: vec![] }).unwrap();
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
