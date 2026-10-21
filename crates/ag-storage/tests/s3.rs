//! The S3 object store against a real S3-compatible server (`scripts/s3-test.sh`
//! starts one, with request signatures verified by the server).
//!
//! Without `AG_TEST_S3_ENDPOINT` these skip with an `AG-SKIP` marker (`ag-testkit`), unless
//! `AG_REQUIRE_COMPONENTS` names `s3`, which turns a missing server into a failure.
#![cfg(feature = "s3")]
#![allow(missing_docs)]

use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ag_object::{RawObject, hash_blob};
use ag_storage::testing::{s3_config, s3_endpoint, unique};
use ag_storage::{
    Credentials, InProcessLock, NoLock, ObjectLock, ObjectStatus, ObjectStore, S3Config, S3ObjectStore, StoreError,
};
use ag_types::{Limits, Timestamp};

fn config(prefix: &str) -> Option<S3Config> {
    ag_testkit::require_here("s3", "AG_TEST_S3_ENDPOINT is not set (use scripts/s3-test.sh)", s3_config(prefix))
}

/// A store on a fresh prefix, emptied again when the guard drops.
struct Fixture {
    store: Arc<S3ObjectStore>,
    cfg: S3Config,
}

impl Fixture {
    fn new() -> Option<Self> {
        let cfg = config(&unique("agtest"))?;
        Self::with(cfg, Arc::new(InProcessLock::new()))
    }

    fn with(cfg: S3Config, lock: Arc<dyn ObjectLock>) -> Option<Self> {
        let store = Arc::new(S3ObjectStore::open(cfg.clone(), lock).expect("open"));
        Some(Self { store, cfg })
    }

    /// Another handle on the same prefix, as another process would have.
    fn second(&self, lock: Arc<dyn ObjectLock>) -> S3ObjectStore {
        S3ObjectStore::open(self.cfg.clone(), lock).expect("open")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.store.purge_for_test();
    }
}

fn blob(s: &str) -> RawObject {
    RawObject::blob(s.as_bytes().to_vec())
}

#[test]
fn put_get_roundtrip_and_idempotency() {
    let Some(f) = Fixture::new() else { return };
    let s = &f.store;
    let obj = blob("hello");
    let id = s.put(&obj).unwrap();
    assert_eq!(id, obj.id());
    assert_eq!(s.put(&obj).unwrap(), id, "putting again is a no-op");
    assert_eq!(s.get(&id).unwrap().unwrap(), obj);
    assert!(s.contains(&id).unwrap());
    assert_eq!(s.verify(&id).unwrap(), ObjectStatus::Ok { size: 5 });
    let listed = s.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].size, obj.encode_envelope().len() as u64);
    assert!(s.delete(&id).unwrap());
    assert!(!s.delete(&id).unwrap());
    assert_eq!(s.verify(&id).unwrap(), ObjectStatus::Missing);
    assert!(s.get(&id).unwrap().is_none());
    assert!(!s.contains(&id).unwrap());
}

#[test]
fn oversized_blobs_are_refused_before_anything_is_sent() {
    let Some(mut cfg) = config(&unique("agtest")) else { return };
    cfg.limits = Limits { max_blob_size: 4, ..Limits::default() };
    let f = Fixture::with(cfg, Arc::new(InProcessLock::new())).unwrap();
    assert!(matches!(f.store.put(&blob("12345")), Err(StoreError::TooLarge { size: 5, limit: 4 })));
    assert!(f.store.put(&blob("1234")).is_ok());
    assert_eq!(f.store.list().unwrap().len(), 1);
}

#[test]
fn every_read_verifies_the_hash_and_put_heals_damage() {
    let Some(f) = Fixture::new() else { return };
    let s = &f.store;
    let obj = blob("important");
    let id = s.put(&obj).unwrap();
    let envelope = obj.encode_envelope();

    // a flipped payload byte
    let mut flipped = envelope.clone();
    *flipped.last_mut().unwrap() ^= 0xff;
    s.corrupt_for_test(&id, &flipped).unwrap();
    assert!(matches!(s.get(&id), Err(StoreError::Corrupt { .. })));
    assert!(matches!(s.verify(&id).unwrap(), ObjectStatus::Corrupt(_)));
    assert!(s.contains(&id).unwrap(), "damaged is not absent");

    // truncation
    s.corrupt_for_test(&id, &envelope[..3]).unwrap();
    assert!(matches!(s.verify(&id).unwrap(), ObjectStatus::Corrupt(_)));

    // a perfectly valid object stored under the wrong key
    s.corrupt_for_test(&id, &blob("something else").encode_envelope()).unwrap();
    assert!(matches!(s.get(&id), Err(StoreError::Corrupt { .. })), "the id is recomputed, not trusted");

    // republishing the same content repairs the store
    s.put(&obj).unwrap();
    assert_eq!(s.get(&id).unwrap().unwrap(), obj);
    assert_eq!(s.verify(&id).unwrap(), ObjectStatus::Ok { size: 9 });
}

#[test]
fn listing_pages_through_every_object_and_ignores_other_prefixes() {
    let Some(mut cfg) = config(&unique("agtest")) else { return };
    cfg.list_page_size = 7;
    let f = Fixture::with(cfg.clone(), Arc::new(InProcessLock::new())).unwrap();
    let mut other_cfg = cfg;
    other_cfg.prefix = format!("{}-other", other_cfg.prefix);
    let other = Fixture::with(other_cfg, Arc::new(InProcessLock::new())).unwrap();

    let ids: Vec<_> = (0..25).map(|i| f.store.put(&blob(&format!("object {i}"))).unwrap()).collect();
    other.store.put(&blob("belongs to someone else")).unwrap();
    let mut want = ids.clone();
    want.sort();
    let listed = f.store.list().unwrap();
    assert_eq!(listed.iter().map(|i| i.id).collect::<Vec<_>>(), want, "25 objects over 4 pages of 7");
    assert_eq!(other.store.list().unwrap().len(), 1, "prefixes are isolated");
    let age_ms = Timestamp::now().as_millis() - listed[0].modified.as_millis();
    assert!((-5_000..60_000).contains(&age_ms), "LastModified is the server's, recent: {age_ms}");
}

#[test]
fn large_objects_and_concurrent_puts_of_one_object_are_safe() {
    let Some(f) = Fixture::new() else { return };
    let big = RawObject::blob((0..5_000_000u32).map(|i| (i % 251) as u8).collect());
    let id = f.store.put(&big).unwrap();
    assert_eq!(f.store.get(&id).unwrap().unwrap(), big);

    let obj = blob(&"shared ".repeat(10_000));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (s, o) = (f.store.clone(), obj.clone());
            std::thread::spawn(move || s.put(&o).unwrap())
        })
        .collect();
    for h in handles {
        assert_eq!(h.join().unwrap(), obj.id());
    }
    assert_eq!(f.store.get(&obj.id()).unwrap().unwrap(), obj);
    assert_eq!(f.store.list().unwrap().len(), 2);
}

#[test]
fn two_handles_see_the_same_bucket() {
    let Some(f) = Fixture::new() else { return };
    let b = f.second(Arc::new(InProcessLock::new()));
    let id = f.store.put(&blob("from a")).unwrap();
    assert_eq!(b.get(&id).unwrap().unwrap(), blob("from a"));
    b.delete(&id).unwrap();
    assert!(!f.store.contains(&id).unwrap());
}

#[test]
fn rewriting_an_old_unreachable_object_refreshes_its_age_so_gc_keeps_it() {
    // The counterpart of the filesystem test `reput_refreshes_age_and_delete_if_older_respects_it`.
    // S3 cannot be told a modification time, so the object really ages on the server's clock.
    let Some(f) = Fixture::new() else { return };
    let s = &f.store;
    let obj = blob("old but wanted");
    let id = s.put(&obj).unwrap();
    std::thread::sleep(Duration::from_millis(2200));
    let cutoff = Timestamp::from_millis(Timestamp::now().as_millis() - 1000);
    assert!(s.list().unwrap()[0].modified < cutoff, "the object looks old");

    // A writer touches it again just before the collector acts: it survives.
    s.put(&obj).unwrap();
    assert!(!s.delete_if_older_than(&id, cutoff).unwrap());
    assert!(s.contains(&id).unwrap());
    assert!(s.list().unwrap()[0].modified >= cutoff, "the server-side copy moved LastModified");

    // Without a refresh it is collected.
    let later = Timestamp::from_millis(Timestamp::now().as_millis() + 10_000);
    assert!(s.delete_if_older_than(&id, later).unwrap());
    assert!(!s.contains(&id).unwrap());
    assert!(!s.delete_if_older_than(&id, later).unwrap(), "already gone");
}

/// Parks the collector between its age check and its delete, runs a writer that
/// re-puts the object at that moment, then lets the collector go on.
/// Returns `(writer finished while the collector was parked, object exists at the end)`.
fn gc_races_a_writer(lock: Arc<dyn ObjectLock>) -> Option<(bool, bool)> {
    let cfg = config(&unique("agtest"))?;
    let writer = Arc::new(S3ObjectStore::open(cfg.clone(), lock.clone()).unwrap());
    let collector = Arc::new(S3ObjectStore::open(cfg, lock).unwrap());
    let obj = blob("old but suddenly wanted");
    let id = writer.put(&obj).unwrap();

    let (checked_tx, checked_rx) = channel();
    let (go_tx, go_rx) = channel::<()>();
    let go_rx = Mutex::new(go_rx);
    collector.set_test_hook(Some(Arc::new(move |point: &str| {
        if point == "delete.checked" {
            checked_tx.send(()).unwrap();
            let _ = go_rx.lock().unwrap().recv_timeout(Duration::from_secs(20));
        }
    })));

    // Everything is "older than the cutoff" for this collector.
    let cutoff = Timestamp::from_millis(Timestamp::now().as_millis() + 3_600_000);
    let c = collector.clone();
    let gc = std::thread::spawn(move || c.delete_if_older_than(&id, cutoff).unwrap());
    checked_rx.recv_timeout(Duration::from_secs(20)).expect("the collector decided to delete");

    // The writer re-puts the object now, after the decision and before the delete.
    let (w, o) = (writer.clone(), obj.clone());
    let put = std::thread::spawn(move || w.put(&o).unwrap());
    std::thread::sleep(Duration::from_millis(700));
    let writer_finished_early = put.is_finished();

    go_tx.send(()).unwrap();
    assert!(gc.join().unwrap(), "the collector acts on its decision");
    put.join().unwrap();
    let exists = writer.contains(&id).unwrap();
    let _ = writer.purge_for_test();
    Some((writer_finished_early, exists))
}

#[test]
fn s3_gc_cannot_delete_an_object_a_writer_just_reput() {
    // The regression test for the race the ObjectLock exists for.
    //
    // Without a lock the interleaving is real: the writer's put finishes (and
    // "refreshes" the object) while the collector is parked between check and
    // delete, and the collector then deletes what the writer was told is stored.
    let Some((early, exists)) = gc_races_a_writer(Arc::new(NoLock)) else { return };
    assert!(early, "without a lock the writer is not held up");
    assert!(!exists, "…and the object it just re-put is gone: this is the lost-object race");

    // With the lock the writer waits for the collector's whole check-then-delete and then
    // simply writes the object again, so a completed put is never undone.
    let (early, exists) = gc_races_a_writer(Arc::new(InProcessLock::new())).unwrap();
    assert!(!early, "the writer waits while the collector holds the exclusive lock");
    assert!(exists, "a put that returned Ok is still there");
}

#[test]
fn block_gc_holds_deletion_off_until_released() {
    let Some(f) = Fixture::new() else { return };
    let id = f.store.put(&blob("backed up")).unwrap();
    let guard = f.store.block_gc().unwrap();
    let (s, started) = (f.store.clone(), Arc::new(Mutex::new(false)));
    let flag = started.clone();
    let t = std::thread::spawn(move || {
        let r = s.delete_if_older_than(&id, Timestamp::from_millis(i64::MAX / 2)).unwrap();
        *flag.lock().unwrap() = true;
        r
    });
    std::thread::sleep(Duration::from_millis(500));
    assert!(!*started.lock().unwrap(), "deletion waits for the backup");
    assert!(f.store.contains(&id).unwrap());
    drop(guard);
    assert!(t.join().unwrap());
}

#[test]
fn the_server_really_checks_signatures() {
    // If this failed, every other test in this file would prove nothing about our signer.
    let Some(mut cfg) = config(&unique("agtest")) else { return };
    cfg.credentials = Credentials::new(cfg.credentials.access_key_id.clone(), "not-the-secret");
    cfg.max_retries = 0;
    let err = S3ObjectStore::open(cfg.clone(), Arc::new(InProcessLock::new())).expect_err("refused");
    assert!(matches!(&err, StoreError::Remote { status: Some(403), .. }), "{err}");
    assert!(err.to_string().contains("access key") && !err.to_string().contains("not-the-secret"), "{err}");

    cfg.credentials = Credentials::new("AKIANOSUCHKEY", "x");
    assert!(S3ObjectStore::open(cfg, Arc::new(InProcessLock::new())).is_err());
}

#[test]
fn a_missing_bucket_or_a_wrong_endpoint_is_reported_when_opening() {
    let Some(mut cfg) = config(&unique("agtest")) else { return };
    cfg.max_retries = 1;
    let mut no_bucket = cfg.clone();
    no_bucket.bucket = format!("no-such-bucket-{}", unique("x").replace('_', "-"));
    let err = S3ObjectStore::open(no_bucket, Arc::new(InProcessLock::new())).expect_err("refused");
    assert!(err.to_string().contains("bucket"), "{err}");

    let mut dead = cfg;
    dead.endpoint = "http://127.0.0.1:9".into();
    let err = S3ObjectStore::open(dead, Arc::new(InProcessLock::new())).expect_err("refused");
    assert!(matches!(err, StoreError::Remote { status: None, .. }), "{err}");
}

#[test]
fn durability_is_stated_honestly() {
    let Some(f) = Fixture::new() else { return };
    let d = f.store.durability();
    assert_eq!(d.name, "s3-compatible");
    assert!(d.survives.iter().any(|s| s.contains("replicates")));
    assert!(d.does_not_survive.iter().any(|s| s.contains("single node")));
    assert!(d.does_not_survive.iter().any(|s| s.contains("not evidence of replication")));
    let _ = (hash_blob(b"x"), s3_endpoint());
}

#[test]
fn https_works_with_the_private_ca_and_no_other_certificate_is_trusted() {
    // The production configuration is TLS; everything above ran over plain HTTP. Only a server that can serve TLS
    // with a private CA takes part (scripts/s3-test.sh with AG_S3_SERVER=seaweedfs).
    let Some(cfg) = ag_testkit::require_here(
        "s3-tls",
        "the S3 server under test offers no TLS (AG_TEST_S3_TLS_ENDPOINT, AG_TEST_S3_CA)",
        ag_storage::testing::s3_tls_config(&unique("agtest")),
    ) else {
        return;
    };
    assert!(cfg.endpoint.starts_with("https://"), "{}", cfg.endpoint);

    // With the CA that signed the certificate: a whole round trip over TLS, signatures still checked by the server.
    let f = Fixture::with(cfg.clone(), Arc::new(InProcessLock::new())).unwrap();
    let obj = blob("over tls");
    let id = f.store.put(&obj).unwrap();
    assert_eq!(f.store.get(&id).unwrap().unwrap(), obj);
    assert_eq!(f.store.list().unwrap().len(), 1);
    assert!(f.store.delete(&id).unwrap());

    // Without it, the certificate does not chain to anything the built-in web roots trust: refused outright, not
    // retried into success and not downgraded to plain HTTP.
    let mut untrusted = cfg.clone();
    untrusted.ca_bundle = None;
    untrusted.max_retries = 0;
    let err =
        S3ObjectStore::open(untrusted, Arc::new(InProcessLock::new())).expect_err("unknown certificate authority");
    assert!(matches!(err, StoreError::Remote { status: None, .. }), "{err}");

    // A bundle without certificates is an error that names the file, not an empty trust store.
    let empty = tempfile::NamedTempFile::new().unwrap();
    let mut no_certs = cfg;
    no_certs.ca_bundle = Some(empty.path().to_path_buf());
    let err = S3ObjectStore::open(no_certs, Arc::new(InProcessLock::new())).expect_err("no certificates");
    assert!(err.to_string().contains("contains no certificates"), "{err}");
}
