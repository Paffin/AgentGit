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
//! Garbage collection is held off for the duration of the copy
//! ([`ObjectStore::block_gc`](ag_storage::ObjectStore::block_gc)). The result is
//! verified by opening it and running the full integrity check.
//!
//! `ag backup` produces a self-contained *local* repository (SQLite file and
//! object files), whatever the source's object backend: objects of an S3 bucket
//! are downloaded, verified one by one, into the backup. A repository whose
//! **metadata** is in PostgreSQL is not backed up by this command: a faithful
//! copy of a database needs the database's own tooling (`pg_dump`, PITR).

use std::fs;
use std::path::Path;

use ag_storage::meta::MetaKind;
use ag_storage::{FsObjectStore, ObjectStore};
use serde::Serialize;

use crate::fsck::FsckReport;
use crate::layout::RepoConfig;
use crate::storage::ObjectsConfig;
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
    ///
    /// Refused for PostgreSQL metadata (back that up with `pg_dump`; see
    /// `docs/operations/postgres-and-s3.md`), before anything is written.
    pub fn backup(&self, dest: &Path, include_keys: bool) -> Result<BackupReport> {
        if self.meta.kind() == MetaKind::Postgres {
            return Err(Error::Invalid(format!(
                "this repository keeps its metadata in PostgreSQL (schema '{}'), which `ag backup` cannot copy \
                 consistently. Back the database up with pg_dump (or continuous archiving / PITR), back the S3 bucket \
                 up with bucket versioning or replication, and check any restore with `ag verify`. \
                 See docs/operations/postgres-and-s3.md",
                self.meta.schema_name().unwrap_or("?")
            )));
        }
        if dest.exists() && fs::read_dir(dest)?.next().is_some() {
            return Err(Error::Conflict(format!("{} is not empty", dest.display())));
        }
        let ag = dest.join(".ag");
        fs::create_dir_all(&ag)?;

        // Hold off garbage collection while objects are copied.
        let _gc_held_off = self.objects.block_gc()?;

        self.meta.backup_to(&ag.join("meta.db"))?;
        let (mut objects, mut bytes) = (0usize, 0u64);
        let local_files = matches!(self.config.storage().objects, ObjectsConfig::Fs);
        if local_files {
            copy_dir(
                &self.layout.objects_dir().join("objects"),
                &ag.join("store").join("objects"),
                &mut objects,
                &mut bytes,
            )?;
            fs::create_dir_all(ag.join("store").join("tmp"))?;
        } else {
            // Remote objects come down through the store, which verifies each one.
            let local = FsObjectStore::open(ag.join("store"), self.limits.clone())?;
            for info in self.objects.list()? {
                let obj = self.objects.get(&info.id)?.ok_or_else(|| {
                    Error::Invalid(format!("object {} vanished while it was being backed up", info.id))
                })?;
                local.put(&obj)?;
                objects += 1;
                bytes += info.size;
            }
        }
        drop(_gc_held_off);

        // Imported source bundles and reconciliation reports are part of recovery, not disposable cache.
        let imported = self.layout.ag_dir().join("import");
        if imported.is_dir() {
            let (mut n, mut b) = (0, 0);
            copy_dir(&imported, &ag.join("import"), &mut n, &mut b)?;
        }
        let mut keys_included = false;
        let keys = self.layout.keys_dir();
        if include_keys && keys.is_dir() {
            let (mut n, mut b) = (0, 0);
            copy_dir(&keys, &ag.join("keys"), &mut n, &mut b)?;
            keys_included = n > 0;
        }
        // Written last: its presence marks a complete backup. The copy is a plain local
        // repository, so it carries no `storage` section.
        if self.config.storage.is_none() {
            fs::copy(self.layout.config_path(), ag.join("config.json"))?;
        } else {
            let plain = RepoConfig { storage: None, ..self.config.clone() };
            fs::write(ag.join("config.json"), serde_json::to_vec_pretty(&plain)?)?;
        }

        let restored = Repository::open(dest)?;
        let verification = restored.fsck()?;
        Ok(BackupReport { objects, bytes, keys_included, verification })
    }
}
