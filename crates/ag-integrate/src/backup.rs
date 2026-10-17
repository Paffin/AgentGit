//! Consistent backups.
//!
//! The metadata database and the object store cannot be snapshotted together,
//! so the order matters:
//!
//! 1. **database first** (`VACUUM INTO`, a transactionally consistent copy);
//! 2. **objects second**.
//!
//! Objects are immutable and always written *before* the metadata row that
//! makes them reachable, so every object the database snapshot refers to
//! already exists when the copy of the object store starts. The copy may
//! contain extra, newer objects; they are unreferenced garbage in the backup.
//! Garbage collection is held off for the duration of the copy (it needs the
//! store's exclusive lock). The result is verified by opening it and running
//! the full integrity check.

use std::fs::{self, File, OpenOptions};
use std::path::Path;

use serde::Serialize;

use crate::fsck::FsckReport;
use crate::{Error, Repository, Result};

/// What a backup contains.
#[derive(Debug, Serialize)]
pub struct BackupReport {
    /// Object files copied.
    pub objects: usize,
    /// Bytes of object data copied.
    pub bytes: u64,
    /// Whether private keys were copied too.
    pub keys_included: bool,
    /// Integrity check of the finished backup.
    pub verification: FsckReport,
}

fn copy_dir(src: &Path, dst: &Path, objects: &mut usize, bytes: &mut u64) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copy_dir(&entry.path(), &to, objects, bytes)?;
        } else if ft.is_file() {
            *bytes += fs::copy(entry.path(), &to)?;
            *objects += 1;
        }
    }
    Ok(())
}

impl Repository {
    /// Writes a verified, self-contained copy of the repository to `dest`
    /// (a new or empty directory). Private keys are only copied with
    /// `include_keys`: a backup of history is not a backup of identities.
    pub fn backup(&self, dest: &Path, include_keys: bool) -> Result<BackupReport> {
        if dest.exists() && fs::read_dir(dest)?.next().is_some() {
            return Err(Error::Conflict(format!("{} is not empty", dest.display())));
        }
        let ag = dest.join(".ag");
        fs::create_dir_all(&ag)?;

        // Hold off garbage collection while objects are copied.
        let store_dir = self.layout.objects_dir();
        let lock: File = OpenOptions::new().create(true).truncate(false).write(true).open(store_dir.join("gc.lock"))?;
        lock.lock_shared()?;

        self.meta.backup_to(&ag.join("meta.db"))?;
        let (mut objects, mut bytes) = (0usize, 0u64);
        copy_dir(&store_dir.join("objects"), &ag.join("store").join("objects"), &mut objects, &mut bytes)?;
        fs::create_dir_all(ag.join("store").join("tmp"))?;
        drop(lock);

        let mut keys_included = false;
        let keys = self.layout.keys_dir();
        if include_keys && keys.is_dir() {
            let (mut n, mut b) = (0, 0);
            copy_dir(&keys, &ag.join("keys"), &mut n, &mut b)?;
            keys_included = n > 0;
        }
        // Written last: its presence marks a complete backup.
        fs::copy(self.layout.config_path(), ag.join("config.json"))?;

        let restored = Repository::open(dest)?;
        let verification = restored.fsck()?;
        Ok(BackupReport { objects, bytes, keys_included, verification })
    }
}
