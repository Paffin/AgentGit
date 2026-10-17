use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use ag_object::{ObjectKind, RawObject};
use ag_types::{Limits, ObjectId, Timestamp};

use crate::clock::{Clock, SystemClock};
use crate::error::StoreError;

/// What a backend actually guarantees once `put` returns.
///
/// "The HTTP/IO call succeeded" is not a durability proof; each backend
/// states its contract explicitly so operators know which failures an
/// acknowledged write survives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Durability {
    /// Short backend identifier, e.g. `local-fsync`.
    pub name: &'static str,
    /// Failures an acknowledged write survives.
    pub survives: &'static [&'static str],
    /// Failures that can still lose acknowledged writes.
    pub does_not_survive: &'static [&'static str],
}

/// Result of verifying one stored object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObjectStatus {
    /// The object does not exist.
    Missing,
    /// The object exists and its hash matches.
    Ok {
        /// Payload size in bytes.
        size: u64,
    },
    /// The object exists but is damaged.
    Corrupt(String),
}

/// Listing entry, used by garbage collection and integrity checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectInfo {
    /// Object identifier.
    pub id: ObjectId,
    /// Stored size in bytes (envelope included).
    pub size: u64,
    /// Time the object was last written.
    pub modified: Timestamp,
}

/// An immutable, content-addressed object store.
pub trait ObjectStore: Send + Sync {
    /// Stores an object and returns its identifier. Idempotent: storing the
    /// same object again is a no-op, and heals a damaged copy.
    fn put(&self, obj: &RawObject) -> Result<ObjectId, StoreError>;
    /// Loads an object, verifying that it hashes to `id`.
    fn get(&self, id: &ObjectId) -> Result<Option<RawObject>, StoreError>;
    /// True if an object file exists (no verification).
    fn contains(&self, id: &ObjectId) -> Result<bool, StoreError>;
    /// Reads and verifies the object without returning it.
    fn verify(&self, id: &ObjectId) -> Result<ObjectStatus, StoreError>;
    /// Lists every stored object.
    fn list(&self) -> Result<Vec<ObjectInfo>, StoreError>;
    /// Deletes an object (garbage collection only). Returns whether it existed.
    fn delete(&self, id: &ObjectId) -> Result<bool, StoreError>;
    /// Deletes an object only if it was last written before `cutoff`, atomically
    /// with respect to concurrent `put`s: a `put` that finishes before this call
    /// refreshes the object's age and therefore saves it, one that starts after
    /// simply rewrites it. This closes the "GC deletes what a writer is about to
    /// reference" race. Returns whether the object was deleted.
    fn delete_if_older_than(&self, id: &ObjectId, cutoff: Timestamp) -> Result<bool, StoreError>;
    /// The durability contract of this backend.
    fn durability(&self) -> Durability;
}

impl<T: ObjectStore + ?Sized> ObjectStore for Arc<T> {
    fn put(&self, obj: &RawObject) -> Result<ObjectId, StoreError> {
        (**self).put(obj)
    }
    fn get(&self, id: &ObjectId) -> Result<Option<RawObject>, StoreError> {
        (**self).get(id)
    }
    fn contains(&self, id: &ObjectId) -> Result<bool, StoreError> {
        (**self).contains(id)
    }
    fn verify(&self, id: &ObjectId) -> Result<ObjectStatus, StoreError> {
        (**self).verify(id)
    }
    fn list(&self) -> Result<Vec<ObjectInfo>, StoreError> {
        (**self).list()
    }
    fn delete(&self, id: &ObjectId) -> Result<bool, StoreError> {
        (**self).delete(id)
    }
    fn delete_if_older_than(&self, id: &ObjectId, cutoff: Timestamp) -> Result<bool, StoreError> {
        (**self).delete_if_older_than(id, cutoff)
    }
    fn durability(&self) -> Durability {
        (**self).durability()
    }
}

fn check_size(obj: &RawObject, limits: &Limits) -> Result<(), StoreError> {
    if obj.kind == ObjectKind::Blob && obj.payload.len() as u64 > limits.max_blob_size {
        return Err(StoreError::TooLarge { size: obj.payload.len() as u64, limit: limits.max_blob_size });
    }
    Ok(())
}

/// In-memory store for tests and ephemeral use.
pub struct MemoryObjectStore {
    objects: RwLock<HashMap<ObjectId, (Vec<u8>, Timestamp)>>,
    limits: Limits,
    clock: Arc<dyn Clock>,
}

impl MemoryObjectStore {
    /// Creates an empty store.
    pub fn new() -> Self {
        Self::with_clock(Arc::new(SystemClock))
    }

    /// Creates an empty store with a custom clock.
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self { objects: RwLock::new(HashMap::new()), limits: Limits::default(), clock }
    }

    /// Overwrites raw stored bytes (test helper for corruption scenarios).
    pub fn corrupt_for_test(&self, id: &ObjectId, bytes: Vec<u8>) {
        let mut g = self.objects.write().expect("lock");
        if let Some(slot) = g.get_mut(id) {
            slot.0 = bytes;
        }
    }
}

impl Default for MemoryObjectStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ObjectStore for MemoryObjectStore {
    fn put(&self, obj: &RawObject) -> Result<ObjectId, StoreError> {
        check_size(obj, &self.limits)?;
        let id = obj.id();
        let env = obj.encode_envelope();
        let mut g = self.objects.write().expect("lock");
        let healthy = g
            .get(&id)
            .map(|(b, _)| RawObject::decode_envelope(b).ok().is_some_and(|o| o.verify(&id).is_ok()))
            .unwrap_or(false);
        if healthy {
            // refresh the age so a concurrent GC keeps it
            if let Some(slot) = g.get_mut(&id) {
                slot.1 = self.clock.now();
            }
        } else {
            g.insert(id, (env, self.clock.now()));
        }
        Ok(id)
    }

    fn get(&self, id: &ObjectId) -> Result<Option<RawObject>, StoreError> {
        let g = self.objects.read().expect("lock");
        let Some((bytes, _)) = g.get(id) else { return Ok(None) };
        let obj = RawObject::decode_envelope(bytes)
            .map_err(|e| StoreError::Corrupt { id: *id, reason: e.to_string() })?;
        obj.verify(id).map_err(|e| StoreError::Corrupt { id: *id, reason: e.to_string() })?;
        Ok(Some(obj))
    }

    fn contains(&self, id: &ObjectId) -> Result<bool, StoreError> {
        Ok(self.objects.read().expect("lock").contains_key(id))
    }

    fn verify(&self, id: &ObjectId) -> Result<ObjectStatus, StoreError> {
        match self.get(id) {
            Ok(None) => Ok(ObjectStatus::Missing),
            Ok(Some(o)) => Ok(ObjectStatus::Ok { size: o.payload.len() as u64 }),
            Err(StoreError::Corrupt { reason, .. }) => Ok(ObjectStatus::Corrupt(reason)),
            Err(e) => Err(e),
        }
    }

    fn list(&self) -> Result<Vec<ObjectInfo>, StoreError> {
        let g = self.objects.read().expect("lock");
        let mut v: Vec<_> =
            g.iter().map(|(id, (b, t))| ObjectInfo { id: *id, size: b.len() as u64, modified: *t }).collect();
        v.sort_by_key(|i| i.id);
        Ok(v)
    }

    fn delete(&self, id: &ObjectId) -> Result<bool, StoreError> {
        Ok(self.objects.write().expect("lock").remove(id).is_some())
    }

    fn delete_if_older_than(&self, id: &ObjectId, cutoff: Timestamp) -> Result<bool, StoreError> {
        let mut g = self.objects.write().expect("lock");
        match g.get(id) {
            Some((_, t)) if *t < cutoff => Ok(g.remove(id).is_some()),
            _ => Ok(false),
        }
    }

    fn durability(&self) -> Durability {
        Durability { name: "memory", survives: &[], does_not_survive: &["process exit"] }
    }
}

/// Local filesystem store.
///
/// Layout: `<root>/objects/<2 hex>/<62 hex>` and `<root>/tmp/`. Writes go to
/// a unique temporary file, are `fsync`ed, then atomically renamed into
/// place, and the directory is `fsync`ed, so a reader never observes a
/// partial object and an acknowledged write survives a crash or power loss
/// of this host. It does **not** survive loss of the disk or the site: put
/// the directory on replicated storage (or use a replicating backend) if you
/// need that.
pub struct FsObjectStore {
    root: PathBuf,
    limits: Limits,
}

impl FsObjectStore {
    /// Opens (creating if needed) a store rooted at `root`.
    pub fn open(root: impl Into<PathBuf>, limits: Limits) -> Result<Self, StoreError> {
        let root = root.into();
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("tmp"))?;
        Ok(Self { root, limits })
    }

    fn path_of(&self, id: &ObjectId) -> PathBuf {
        let hex = id.to_hex();
        self.root.join("objects").join(&hex[..2]).join(&hex[2..])
    }

    fn read_verified(&self, id: &ObjectId, path: &Path) -> Result<Option<RawObject>, StoreError> {
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let obj = RawObject::decode_envelope(&bytes)
            .map_err(|e| StoreError::Corrupt { id: *id, reason: e.to_string() })?;
        obj.verify(id).map_err(|e| StoreError::Corrupt { id: *id, reason: e.to_string() })?;
        Ok(Some(obj))
    }

    fn lock_file(&self) -> std::io::Result<File> {
        OpenOptions::new().create(true).truncate(false).write(true).open(self.root.join("gc.lock"))
    }

    fn sync_dir(dir: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            File::open(dir)?.sync_all()?;
        }
        #[cfg(not(unix))]
        {
            let _ = dir;
        }
        Ok(())
    }
}

impl ObjectStore for FsObjectStore {
    fn put(&self, obj: &RawObject) -> Result<ObjectId, StoreError> {
        check_size(obj, &self.limits)?;
        let id = obj.id();
        let path = self.path_of(&id);
        // Writers hold a shared lock; the collector's delete takes it exclusively.
        let lock = self.lock_file()?;
        lock.lock_shared()?;
        if let Ok(Some(_)) = self.read_verified(&id, &path) {
            // Already stored: refresh its age so a concurrent GC keeps it.
            File::open(&path)?.set_modified(std::time::SystemTime::now())?;
            return Ok(id);
        }
        let dir = path.parent().expect("object path has a parent");
        fs::create_dir_all(dir)?;
        let tmp = self.root.join("tmp").join(format!(
            "{}-{}",
            std::process::id(),
            ag_crypto::random_id("t").map_err(|e| StoreError::Injected(e.to_string()))?
        ));
        {
            let mut f = File::create(&tmp)?;
            f.write_all(&obj.encode_envelope())?;
            f.sync_all()?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o444))?;
        }
        fs::rename(&tmp, &path)?;
        Self::sync_dir(dir)?;
        Ok(id)
    }

    fn get(&self, id: &ObjectId) -> Result<Option<RawObject>, StoreError> {
        self.read_verified(id, &self.path_of(id))
    }

    fn contains(&self, id: &ObjectId) -> Result<bool, StoreError> {
        Ok(self.path_of(id).is_file())
    }

    fn verify(&self, id: &ObjectId) -> Result<ObjectStatus, StoreError> {
        match self.read_verified(id, &self.path_of(id)) {
            Ok(None) => Ok(ObjectStatus::Missing),
            Ok(Some(o)) => Ok(ObjectStatus::Ok { size: o.payload.len() as u64 }),
            Err(StoreError::Corrupt { reason, .. }) => Ok(ObjectStatus::Corrupt(reason)),
            Err(e) => Err(e),
        }
    }

    fn list(&self) -> Result<Vec<ObjectInfo>, StoreError> {
        let mut out = Vec::new();
        let objects = self.root.join("objects");
        for shard in fs::read_dir(&objects)? {
            let shard = shard?;
            let Some(prefix) = shard.file_name().to_str().map(str::to_owned) else { continue };
            if prefix.len() != 2 || !shard.file_type()?.is_dir() {
                continue;
            }
            for f in fs::read_dir(shard.path())? {
                let f = f?;
                let Some(rest) = f.file_name().to_str().map(str::to_owned) else { continue };
                let Ok(id) = format!("{prefix}{rest}").parse::<ObjectId>() else { continue };
                let md = f.metadata()?;
                let modified = md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| Timestamp::from_millis(d.as_millis() as i64))
                    .unwrap_or_else(|| Timestamp::from_millis(0));
                out.push(ObjectInfo { id, size: md.len(), modified });
            }
        }
        out.sort_by_key(|i| i.id);
        Ok(out)
    }

    fn delete(&self, id: &ObjectId) -> Result<bool, StoreError> {
        match fs::remove_file(self.path_of(id)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    fn delete_if_older_than(&self, id: &ObjectId, cutoff: Timestamp) -> Result<bool, StoreError> {
        let lock = self.lock_file()?;
        lock.lock()?;
        let path = self.path_of(id);
        let md = match fs::metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        let modified = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        if modified >= cutoff.as_millis() {
            return Ok(false);
        }
        fs::remove_file(&path)?;
        Ok(true)
    }

    fn durability(&self) -> Durability {
        Durability {
            name: "local-fsync",
            survives: &["process crash", "kernel panic", "power loss (fsync honoured by the disk)"],
            does_not_survive: &["disk loss", "host loss", "site loss"],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ag_object::hash_blob;

    fn stores() -> Vec<(Box<dyn ObjectStore>, Option<tempfile::TempDir>)> {
        let dir = tempfile::tempdir().unwrap();
        let fs = FsObjectStore::open(dir.path(), Limits::default()).unwrap();
        vec![(Box::new(MemoryObjectStore::new()), None), (Box::new(fs), Some(dir))]
    }

    #[test]
    fn put_get_roundtrip_and_idempotency() {
        for (s, _g) in stores() {
            let obj = RawObject::blob(b"hello".to_vec());
            let id = s.put(&obj).unwrap();
            assert_eq!(id, obj.id());
            assert_eq!(s.put(&obj).unwrap(), id);
            assert_eq!(s.get(&id).unwrap().unwrap(), obj);
            assert!(s.contains(&id).unwrap());
            assert_eq!(s.verify(&id).unwrap(), ObjectStatus::Ok { size: 5 });
            assert_eq!(s.list().unwrap().len(), 1);
            assert!(s.delete(&id).unwrap());
            assert!(!s.delete(&id).unwrap());
            assert_eq!(s.verify(&id).unwrap(), ObjectStatus::Missing);
            assert!(s.get(&id).unwrap().is_none());
        }
    }

    #[test]
    fn rejects_oversized_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let s = FsObjectStore::open(dir.path(), Limits { max_blob_size: 4, ..Limits::default() }).unwrap();
        let r = s.put(&RawObject::blob(vec![0; 5]));
        assert!(matches!(r, Err(StoreError::TooLarge { size: 5, limit: 4 })));
        assert!(s.put(&RawObject::blob(vec![0; 4])).is_ok());
    }

    #[test]
    fn fs_detects_corruption_and_put_heals_it() {
        let dir = tempfile::tempdir().unwrap();
        let s = FsObjectStore::open(dir.path(), Limits::default()).unwrap();
        let obj = RawObject::blob(b"important".to_vec());
        let id = s.put(&obj).unwrap();
        let path = s.path_of(&id);

        // flip a payload byte
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        fs::write(&path, &bytes).unwrap();
        assert!(matches!(s.get(&id), Err(StoreError::Corrupt { .. })));
        assert!(matches!(s.verify(&id).unwrap(), ObjectStatus::Corrupt(_)));

        // truncation
        fs::write(&path, &bytes[..3]).unwrap();
        assert!(matches!(s.verify(&id).unwrap(), ObjectStatus::Corrupt(_)));

        // republishing the same content repairs the store
        s.put(&obj).unwrap();
        assert_eq!(s.get(&id).unwrap().unwrap(), obj);
    }

    #[test]
    fn memory_detects_corruption() {
        let s = MemoryObjectStore::new();
        let id = s.put(&RawObject::blob(b"x".to_vec())).unwrap();
        s.corrupt_for_test(&id, b"AGOB\x01\x00\x01y".to_vec());
        assert!(matches!(s.verify(&id).unwrap(), ObjectStatus::Corrupt(_)));
        s.put(&RawObject::blob(b"x".to_vec())).unwrap();
        assert!(matches!(s.verify(&id).unwrap(), ObjectStatus::Ok { .. }));
    }

    #[test]
    fn fs_leaves_no_temp_files_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let id = {
            let s = FsObjectStore::open(dir.path(), Limits::default()).unwrap();
            s.put(&RawObject::blob(b"persist".to_vec())).unwrap()
        };
        let s = FsObjectStore::open(dir.path(), Limits::default()).unwrap();
        assert_eq!(s.get(&id).unwrap().unwrap().payload, b"persist");
        assert_eq!(fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0);
        assert_eq!(hash_blob(b"persist").object_id(), id);
    }

    #[test]
    fn concurrent_puts_of_same_object_are_safe() {
        let dir = tempfile::tempdir().unwrap();
        let s = Arc::new(FsObjectStore::open(dir.path(), Limits::default()).unwrap());
        let obj = RawObject::blob(vec![42; 100_000]);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (s, o) = (s.clone(), obj.clone());
                std::thread::spawn(move || s.put(&o).unwrap())
            })
            .collect();
        for h in handles {
            assert_eq!(h.join().unwrap(), obj.id());
        }
        assert_eq!(s.get(&obj.id()).unwrap().unwrap(), obj);
        assert_eq!(s.list().unwrap().len(), 1);
    }

    #[test]
    fn reput_refreshes_age_and_delete_if_older_respects_it() {
        let dir = tempfile::tempdir().unwrap();
        let s = FsObjectStore::open(dir.path(), Limits::default()).unwrap();
        let obj = RawObject::blob(b"old but wanted".to_vec());
        let id = s.put(&obj).unwrap();
        let path = s.path_of(&id);
        let an_hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        File::open(&path).unwrap().set_modified(an_hour_ago).unwrap();
        let cutoff = Timestamp::from_millis(Timestamp::now().as_millis() - 1_800_000);
        assert!(s.list().unwrap()[0].modified < cutoff, "object looks old");

        // A writer touches it again just before the collector acts: it survives.
        s.put(&obj).unwrap();
        assert!(!s.delete_if_older_than(&id, cutoff).unwrap());
        assert!(s.contains(&id).unwrap());

        // Without the refresh it is collected.
        File::open(&path).unwrap().set_modified(an_hour_ago).unwrap();
        assert!(s.delete_if_older_than(&id, cutoff).unwrap());
        assert!(!s.contains(&id).unwrap());
        assert!(!s.delete_if_older_than(&id, cutoff).unwrap(), "already gone");
    }

    #[test]
    fn memory_store_delete_if_older_respects_refresh() {
        let clock = Arc::new(crate::clock::ManualClock::new(Timestamp::from_millis(1_000)));
        let s = MemoryObjectStore::with_clock(clock.clone());
        let obj = RawObject::blob(b"x".to_vec());
        let id = s.put(&obj).unwrap();
        clock.advance_ms(10_000);
        s.put(&obj).unwrap(); // refresh at t=11_000
        assert!(!s.delete_if_older_than(&id, Timestamp::from_millis(5_000)).unwrap());
        assert!(s.delete_if_older_than(&id, Timestamp::from_millis(20_000)).unwrap());
    }
}
