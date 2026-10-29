#![allow(missing_docs)]
use ag_integrate::{GcOptions, Repository, VirtualChange, VirtualContent};
use ag_object::RawObject;
use ag_storage::{Durability, ManualClock, ObjectInfo, ObjectStatus, ObjectStore, StoreError};
use ag_types::{FileType, Identity, Namespace, ObjectId, RepoPath, StreamName, Timestamp};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
struct Gated {
    inner: Arc<dyn ObjectStore>,
    method: &'static str,
    reached: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}
impl Gated {
    fn pause(&self, method: &str) {
        if self.method == method {
            self.reached.send(()).unwrap();
            self.release.lock().unwrap().recv_timeout(Duration::from_secs(10)).unwrap();
        }
    }
}
impl ObjectStore for Gated {
    fn put(&self, o: &RawObject) -> Result<ObjectId, StoreError> {
        self.pause("put");
        self.inner.put(o)
    }
    fn get(&self, i: &ObjectId) -> Result<Option<RawObject>, StoreError> {
        self.inner.get(i)
    }
    fn contains(&self, i: &ObjectId) -> Result<bool, StoreError> {
        self.inner.contains(i)
    }
    fn verify(&self, i: &ObjectId) -> Result<ObjectStatus, StoreError> {
        self.inner.verify(i)
    }
    fn list(&self) -> Result<Vec<ObjectInfo>, StoreError> {
        self.pause("list");
        self.inner.list()
    }
    fn delete(&self, i: &ObjectId) -> Result<bool, StoreError> {
        self.inner.delete(i)
    }
    fn delete_if_older_than(&self, i: &ObjectId, t: Timestamp) -> Result<bool, StoreError> {
        self.inner.delete_if_older_than(i, t)
    }
    fn durability(&self) -> Durability {
        self.inner.durability()
    }
}
fn gated(repo: &Repository, method: &'static str) -> (Repository, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (tx, rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let view =
        repo.with_object_store(|inner| Arc::new(Gated { inner, method, reached: tx, release: Mutex::new(release_rx) }));
    (view, rx, release_tx)
}
fn fixture() -> (Repository, Arc<ManualClock>, Identity) {
    let clock = Arc::new(ManualClock::new(Timestamp::from_millis(1000)));
    let repo =
        Repository::ephemeral(Namespace::new("audit/repo").unwrap(), StreamName::new("main").unwrap(), clock.clone())
            .unwrap();
    let who = Identity::new("user:audit").unwrap();
    repo.workspace_create("work", &StreamName::new("main").unwrap(), &who).unwrap();
    (repo, clock, who)
}
fn change(path: &str, id: ag_types::BlobId) -> VirtualChange {
    VirtualChange::Put {
        path: RepoPath::new(path).unwrap(),
        content: VirtualContent::Blob(id),
        file_type: FileType::Regular,
        executable: false,
    }
}
#[test]
fn audit_workspace_expected_head_must_be_atomic() {
    let (repo, _, who) = fixture();
    let blob = repo.put_blob(b"content".to_vec()).unwrap();
    let initial =
        repo.workspace_modify("work", &[change("initial", blob)], "initial", &who, None).unwrap().head.unwrap();
    let (slow, reached, release) = gated(&repo, "put");
    let who2 = who.clone();
    let thread = std::thread::spawn(move || {
        slow.workspace_modify("work", &[change("slow", blob)], "slow", &who2, Some(&initial))
    });
    reached.recv_timeout(Duration::from_secs(10)).unwrap();
    let fast = repo.workspace_modify("work", &[change("fast", blob)], "fast", &who, Some(&initial)).unwrap();
    release.send(()).unwrap();
    let late = thread.join().unwrap();
    println!("fast head={:?}; delayed result={:?}", fast.head, late);
    let final_head = repo.workspace_row("work").unwrap().head.unwrap();
    let tree = repo.tree_of(&final_head).unwrap();
    println!("final paths={:?}", tree.iter().map(|(p, _)| p.to_string()).collect::<Vec<_>>());
    assert!(late.is_err(), "a write based on a stale expected_head must be rejected");
    assert!(tree.get(&RepoPath::new("fast").unwrap()).is_some());
    assert!(tree.get(&RepoPath::new("slow").unwrap()).is_none());
}
#[test]
fn audit_gc_serializes_new_pins_and_rejects_swept_roots() {
    let (repo, clock, who) = fixture();
    let blob = repo.put_blob(b"retained content".to_vec()).unwrap();
    let cs = repo
        .commit(
            vec![],
            &who,
            "keep me",
            vec![ag_changeset::Op::Put {
                path: RepoPath::new("kept").unwrap(),
                content: blob,
                file_type: FileType::Regular,
                executable: false,
            }],
        )
        .unwrap();
    clock.advance_ms(10000);
    let (collector, reached, release) = gated(&repo, "list");
    let actor = who.clone();
    let thread = std::thread::spawn(move || {
        collector.gc(&GcOptions { grace_ms: 1000, reflog_retention_ms: 1000, dry_run: false }, actor.as_str())
    });
    reached.recv_timeout(Duration::from_secs(10)).unwrap();
    let (pin, _, _) = gated(&repo, "never");
    let (sent, received) = mpsc::channel();
    let pinner = std::thread::spawn(move || sent.send(pin.pin("keep", &cs, "retention root", &who)).unwrap());
    assert!(received.recv_timeout(Duration::from_millis(50)).is_err(), "pin must wait for the collector transaction");
    release.send(()).unwrap();
    thread.join().unwrap().unwrap();
    assert!(received.recv_timeout(Duration::from_secs(10)).unwrap().is_err(), "cannot pin a swept object");
    pinner.join().unwrap();
    assert!(repo.pins().unwrap().is_empty());
    assert!(repo.fsck().unwrap().is_ok());
}

struct PauseSecondClock {
    reached: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    calls: std::sync::atomic::AtomicUsize,
}
impl ag_storage::Clock for PauseSecondClock {
    fn now(&self) -> Timestamp {
        if std::thread::current().name() == Some("closer")
            && self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1
        {
            self.reached.send(()).unwrap();
            self.release.lock().unwrap().recv_timeout(Duration::from_secs(10)).unwrap();
        }
        Timestamp::now()
    }
}
#[test]
fn audit_close_must_not_overwrite_concurrent_integration() {
    let dir = tempfile::tempdir().unwrap();
    let author = Identity::new("user:author").unwrap();
    let admin = Identity::new("user:admin").unwrap();
    let repo = Repository::init(
        dir.path(),
        &ag_integrate::InitOptions::new(
            Namespace::new("audit/repo").unwrap(),
            StreamName::new("main").unwrap(),
            author.clone(),
        ),
    )
    .unwrap();
    let cs = repo.commit(vec![], &author, "root", vec![]).unwrap();
    let prop = repo.propose("main", &cs, "change", "", &author, None).unwrap();
    let (tx, rx) = mpsc::channel();
    let (send, recv) = mpsc::channel();
    let slow = Repository::open_with_clock(
        dir.path(),
        Arc::new(PauseSecondClock {
            reached: tx,
            release: Mutex::new(recv),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }),
    )
    .unwrap();
    let pid = prop.id.clone();
    let a = admin.clone();
    let closer =
        std::thread::Builder::new().name("closer".into()).spawn(move || slow.close_proposal(&pid, &a)).unwrap();
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let integrated = repo
        .integrate(&ag_integrate::IntegrateRequest {
            proposal_id: prop.id.clone(),
            actor: admin,
            idempotency_key: None,
            fencing_tokens: vec![],
        })
        .unwrap();
    println!("integration={:?}", integrated.outcome);
    assert!(matches!(integrated.outcome, ag_integrate::Outcome::Integrated { .. }));
    send.send(()).unwrap();
    println!("close={:?}", closer.join().unwrap());
    let final_state = repo.find_proposal(&prop.id).unwrap();
    println!("final proposal={:?}", final_state);
    assert_eq!(final_state.state, "integrated", "closing an already integrated proposal corrupts its terminal state");
}

#[test]
fn audit_diff_report_must_describe_file_mode_changes() {
    let (repo, _, _) = fixture();
    let blob = repo.put_blob(b"echo hello\n".to_vec()).unwrap();
    let path = RepoPath::new("run.sh").unwrap();
    let old = ag_tree::Tree::from_entries(
        vec![(path.clone(), ag_tree::Entry { content: blob, file_type: FileType::Regular, executable: false })],
        repo.limits(),
    )
    .unwrap();
    let new = ag_tree::Tree::from_entries(
        vec![(path, ag_tree::Entry { content: blob, file_type: FileType::Regular, executable: true })],
        repo.limits(),
    )
    .unwrap();
    let (report, _) = repo.diff_report(&old, &new, &[], 3).unwrap();
    let json = serde_json::to_value(&report).unwrap();
    println!("mode-only diff={json}");
    assert_eq!(json[0]["old_mode"], "100644");
    assert_eq!(json[0]["new_mode"], "100755");
}
#[test]
fn audit_diff_report_must_enforce_its_byte_limit() {
    let (repo, _, _) = fixture();
    let blob = repo.put_blob(vec![b'x'; 5 * 1024 * 1024]).unwrap();
    let new = ag_tree::Tree::from_entries(
        vec![(
            RepoPath::new("large.txt").unwrap(),
            ag_tree::Entry { content: blob, file_type: FileType::Regular, executable: false },
        )],
        repo.limits(),
    )
    .unwrap();
    let (report, truncated) = repo.diff_report(&ag_tree::Tree::empty(), &new, &[], 3).unwrap();
    let len: usize = report.iter().map(|f| f.patch.len()).sum();
    println!("patch bytes={len}, truncated={truncated}");
    assert!(len <= 4 * 1024 * 1024, "4 MiB diff budget was exceeded by a single file");
    assert!(truncated);
    assert!(report[0].truncated);
}

#[test]
fn caller_errors_roll_back_metadata_and_keep_the_original_error() {
    let (repo, _, _) = fixture();
    let result = repo.meta().write_with_error(|tx| -> ag_integrate::Result<()> {
        tx.meta_set("rollback-proof", "value")?;
        Err(ag_integrate::Error::Invalid("original".into()))
    });
    assert!(matches!(result,Err(ag_integrate::Error::Invalid(s)) if s=="original"));
    assert_eq!(repo.meta().read(|tx| tx.meta_get("rollback-proof")).unwrap(), None);
}
#[test]
fn pinned_history_keeps_its_ancestors_and_blobs() {
    let (repo, clock, who) = fixture();
    let blob = repo.put_blob(b"parent".to_vec()).unwrap();
    let parent = repo
        .commit(
            vec![],
            &who,
            "parent",
            vec![ag_changeset::Op::Put {
                path: RepoPath::new("file").unwrap(),
                content: blob,
                file_type: FileType::Regular,
                executable: false,
            }],
        )
        .unwrap();
    let child = repo.commit(vec![parent], &who, "child", vec![]).unwrap();
    repo.pin("keep", &child, "history", &who).unwrap();
    clock.advance_ms(10000);
    let report =
        repo.gc(&GcOptions { grace_ms: 1000, reflog_retention_ms: 1000, dry_run: false }, who.as_str()).unwrap();
    assert!(report.deleted.is_empty());
    assert_eq!(repo.blob(&blob).unwrap(), b"parent");
    assert!(repo.tree_of(&child).is_ok());
}

#[test]
fn backup_and_gc_finish_with_independent_sqlite_handles() {
    let dir = tempfile::tempdir().unwrap();
    let backup_dir = tempfile::tempdir().unwrap();
    let who = Identity::new("user:audit").unwrap();
    Repository::init(
        dir.path(),
        &ag_integrate::InitOptions::new(
            Namespace::new("audit/backup").unwrap(),
            StreamName::new("main").unwrap(),
            who.clone(),
        ),
    )
    .unwrap();
    let clock = Arc::new(ManualClock::new(Timestamp::from_millis(Timestamp::now().as_millis() + 10_000)));
    let repo = Repository::open_with_clock(dir.path(), clock).unwrap();
    let root = repo.commit(vec![], &who, "root", vec![]).unwrap();
    repo.pin("keep", &root, "retained", &who).unwrap();
    repo.put_blob(b"garbage".to_vec()).unwrap();
    let backup = repo.reopen().unwrap();
    let (collector, reached, release) = gated(&repo, "list");
    let gc = std::thread::spawn(move || {
        collector.gc(&GcOptions { grace_ms: 0, reflog_retention_ms: 0, dry_run: false }, "user:audit")
    });
    reached.recv_timeout(Duration::from_secs(10)).unwrap();
    let dest = backup_dir.path().join("copy");
    let (sent, received) = mpsc::channel();
    let copy = std::thread::spawn(move || sent.send(backup.backup(&dest, false)).unwrap());
    assert!(received.recv_timeout(Duration::from_millis(50)).is_err());
    release.send(()).unwrap();
    gc.join().unwrap().unwrap();
    let report = received.recv_timeout(Duration::from_secs(10)).expect("backup/GC deadlocked").unwrap();
    assert!(report.verification.is_ok());
    copy.join().unwrap();
}

#[test]
fn cached_changesets_cannot_resurrect_a_collected_root_or_ancestor() {
    let (repo, clock, who) = fixture();
    let parent = repo.commit(vec![], &who, "parent", vec![]).unwrap();
    repo.changeset(&parent).unwrap();
    clock.advance_ms(10_000);
    let child = repo.commit(vec![parent], &who, "young child", vec![]).unwrap();
    repo.changeset(&child).unwrap();
    repo.gc(&GcOptions { grace_ms: 1_000, reflog_retention_ms: 1_000, dry_run: false }, who.as_str()).unwrap();
    assert!(!repo.objects().contains(&parent.object_id()).unwrap());
    assert!(repo.objects().contains(&child.object_id()).unwrap());
    assert!(repo.pin("bad", &child, "missing parent", &who).is_err());
    assert!(repo.workspace_create_at("bad", &StreamName::new("main").unwrap(), &who, Some(&parent)).is_err());
    assert!(repo.workspace_row("bad").is_err());
}

#[test]
fn an_authorized_workspace_row_cannot_be_reused_after_recreation_on_another_stream() {
    let (repo, _, who) = fixture();
    let dev = StreamName::new("dev").unwrap();
    repo.create_stream(&dev, None, &who).unwrap();
    let authorized = repo.workspace_create("scoped", &dev, &who).unwrap();
    repo.workspace_remove("scoped", &who).unwrap();
    repo.workspace_create("scoped", &StreamName::new("main").unwrap(), &who).unwrap();
    let blob = repo.put_blob(b"attempt".to_vec()).unwrap();
    assert!(repo.workspace_modify_checked(&authorized, &[change("file", blob)], "attempt", &who, None).is_err());
    assert!(repo.workspace_sync_virtual_checked(&authorized, &who, &[]).is_err());
    assert!(repo.workspace_remove_checked(&authorized, &who).is_err());
    assert_eq!(repo.workspace_row("scoped").unwrap().stream, "main");
}
