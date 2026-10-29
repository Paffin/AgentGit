//! Concurrent workspace writers must never overwrite an acknowledged head.
#![allow(missing_docs)]
mod common;
use ag_integrate::{Repository, VirtualChange, VirtualContent};
use ag_object::RawObject;
use ag_storage::{Durability, ObjectInfo, ObjectStatus, ObjectStore, StoreError};
use ag_types::{BlobId, FileType, ObjectId, RepoPath, StreamName, Timestamp};
use common::*;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

struct PausePut {
    inner: Arc<dyn ObjectStore>,
    reached: mpsc::Sender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}
impl ObjectStore for PausePut {
    fn put(&self, object: &RawObject) -> Result<ObjectId, StoreError> {
        self.reached.send(()).unwrap();
        self.resume.lock().unwrap().recv_timeout(Duration::from_secs(10)).unwrap();
        self.inner.put(object)
    }
    fn get(&self, id: &ObjectId) -> Result<Option<RawObject>, StoreError> {
        self.inner.get(id)
    }
    fn contains(&self, id: &ObjectId) -> Result<bool, StoreError> {
        self.inner.contains(id)
    }
    fn verify(&self, id: &ObjectId) -> Result<ObjectStatus, StoreError> {
        self.inner.verify(id)
    }
    fn list(&self) -> Result<Vec<ObjectInfo>, StoreError> {
        self.inner.list()
    }
    fn delete(&self, id: &ObjectId) -> Result<bool, StoreError> {
        self.inner.delete(id)
    }
    fn delete_if_older_than(&self, id: &ObjectId, time: Timestamp) -> Result<bool, StoreError> {
        self.inner.delete_if_older_than(id, time)
    }
    fn durability(&self) -> Durability {
        self.inner.durability()
    }
}
fn paused(repo: &Repository) -> (Repository, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (tx, rx) = mpsc::channel();
    let (resume, wait) = mpsc::channel();
    let repo = repo.with_object_store(|inner| Arc::new(PausePut { inner, reached: tx, resume: Mutex::new(wait) }));
    (repo, rx, resume)
}
fn put(path: &str, blob: BlobId) -> VirtualChange {
    VirtualChange::Put {
        path: RepoPath::new(path).unwrap(),
        content: VirtualContent::Blob(blob),
        file_type: FileType::Regular,
        executable: false,
    }
}

#[test]
fn concurrent_modifications_preserve_the_winner_even_from_an_empty_head() {
    for seeded in [false, true] {
        let f = fx();
        let actor = id("user:alice");
        f.repo.workspace_create("work", &StreamName::new("main").unwrap(), &actor).unwrap();
        let blob = f.repo.put_blob(b"content".to_vec()).unwrap();
        if seeded {
            f.repo.workspace_modify("work", &[put("initial", blob)], "initial", &actor, None).unwrap();
        }
        let before = f.repo.workspace_row("work").unwrap();
        let (slow, reached, resume) = paused(&f.repo);
        let who = actor.clone();
        let thread = std::thread::spawn(move || {
            slow.workspace_modify("work", &[put("slow", blob)], "slow", &who, before.head.as_ref())
        });
        reached.recv_timeout(Duration::from_secs(10)).unwrap();
        let winner = f.repo.workspace_modify("work", &[put("fast", blob)], "fast", &actor, before.head.as_ref());
        resume.send(()).unwrap();
        let loser = thread.join().unwrap();
        let winner = winner.unwrap();
        assert!(loser.is_err(), "a stale writer must not replace the winner: {loser:?}");
        assert_eq!(f.repo.workspace_row("work").unwrap().head, winner.head);
        let tree = f.repo.tree_of(&winner.head.unwrap()).unwrap();
        assert!(tree.get(&RepoPath::new("fast").unwrap()).is_some());
        assert!(tree.get(&RepoPath::new("slow").unwrap()).is_none());
    }
}

#[test]
fn a_modification_cannot_replace_a_concurrent_fast_forward_sync() {
    let f = fx();
    let base = f.land(None, &[("base", "base")]);
    let actor = id("user:alice");
    f.repo.workspace_create("work", &StreamName::new("main").unwrap(), &actor).unwrap();
    let advanced = f.land(Some(base), &[("base", "base"), ("upstream", "upstream")]);
    let blob = f.repo.put_blob(b"content".to_vec()).unwrap();
    let (slow, reached, resume) = paused(&f.repo);
    let who = actor.clone();
    let thread =
        std::thread::spawn(move || slow.workspace_modify("work", &[put("slow", blob)], "slow", &who, Some(&base)));
    reached.recv_timeout(Duration::from_secs(10)).unwrap();
    let synced = f.repo.workspace_sync_virtual("work", &actor, &[]);
    resume.send(()).unwrap();
    let loser = thread.join().unwrap();
    synced.unwrap();
    assert!(loser.is_err());
    assert_eq!(f.repo.workspace_row("work").unwrap().head, Some(advanced));
}

#[test]
fn a_sync_merge_cannot_replace_a_concurrent_modification() {
    let f = fx();
    let base = f.land(None, &[("base", "base")]);
    let actor = id("user:alice");
    f.repo.workspace_create("work", &StreamName::new("main").unwrap(), &actor).unwrap();
    let blob = f.repo.put_blob(b"content".to_vec()).unwrap();
    let local =
        f.repo.workspace_modify("work", &[put("local", blob)], "local", &actor, Some(&base)).unwrap().head.unwrap();
    f.land(Some(base), &[("base", "base"), ("upstream", "upstream")]);
    let (slow, reached, resume) = paused(&f.repo);
    let who = actor.clone();
    let thread = std::thread::spawn(move || slow.workspace_sync_virtual("work", &who, &[]));
    reached.recv_timeout(Duration::from_secs(10)).unwrap();
    let winner = f.repo.workspace_modify("work", &[put("fast", blob)], "fast", &actor, Some(&local));
    resume.send(()).unwrap();
    let loser = thread.join().unwrap();
    let winner = winner.unwrap();
    assert!(loser.is_err(), "a stale merge must not replace the newer workspace head");
    assert_eq!(f.repo.workspace_row("work").unwrap().head, winner.head);
    let tree = f.repo.tree_of(&winner.head.unwrap()).unwrap();
    assert!(tree.get(&RepoPath::new("local").unwrap()).is_some());
    assert!(tree.get(&RepoPath::new("fast").unwrap()).is_some());
}
