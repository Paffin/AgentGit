//! Coordination between object writers and the garbage collector.
//!
//! The filesystem store gets "a writer that re-puts an old object always beats
//! the collector" from an `flock` on `gc.lock`. An object store reached over
//! HTTP has no cross-request atomicity, so a backend like S3 borrows the same
//! idea from an [`ObjectLock`]:
//!
//! * `put` (and the age refresh of an existing object) holds [`ObjectLock::shared`];
//! * `delete_if_older_than` holds [`ObjectLock::exclusive`] across its whole
//!   check-then-act, so the age it checked is still the age when it deletes.
//!
//! Many writers share the lock; the collector excludes them one object at a
//! time. Pick the implementation that spans everyone who can touch the bucket:
//!
//! | lock | spans |
//! |---|---|
//! | [`InProcessLock`] | threads of one process (tests, embedding) |
//! | [`FileLock`] | processes on one host that can open the same file (SQLite metadata) |
//! | [`PostgresAdvisoryLock`] | every process that can reach the same PostgreSQL database |

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::error::StoreError;
use crate::meta::pg::{self, PgClient, Pool, Pooled};
use crate::meta::{PgOptions, advisory_key};

/// A held lock; releasing happens when the box is dropped.
pub type LockGuard = Box<dyn Send>;

/// A reader/writer lock shared by everyone who writes to, or collects garbage
/// from, one object store.
///
/// Guards must not be nested: taking [`shared`](ObjectLock::shared) while the
/// same thread already holds a guard can deadlock behind a waiting collector.
pub trait ObjectLock: Send + Sync {
    /// Held by writers. Any number of holders at once; excluded by [`exclusive`](ObjectLock::exclusive).
    fn shared(&self) -> Result<LockGuard, StoreError>;
    /// Held by the collector around one check-then-act. Excludes everyone else.
    fn exclusive(&self) -> Result<LockGuard, StoreError>;
}

impl<T: ObjectLock + ?Sized> ObjectLock for Arc<T> {
    fn shared(&self) -> Result<LockGuard, StoreError> {
        (**self).shared()
    }
    fn exclusive(&self) -> Result<LockGuard, StoreError> {
        (**self).exclusive()
    }
}

/// No coordination at all. Only for tests that demonstrate the race the real
/// locks prevent; never use it with a store that is garbage collected.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoLock;

impl ObjectLock for NoLock {
    fn shared(&self) -> Result<LockGuard, StoreError> {
        Ok(Box::new(()))
    }
    fn exclusive(&self) -> Result<LockGuard, StoreError> {
        Ok(Box::new(()))
    }
}

// ---- in process ------------------------------------------------------------------

#[derive(Default)]
struct RwState {
    readers: usize,
    writer: bool,
    writers_waiting: usize,
}

#[derive(Default)]
struct RwInner {
    state: Mutex<RwState>,
    changed: Condvar,
}

impl RwInner {
    fn state(&self) -> MutexGuard<'_, RwState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A reader/writer lock inside one process. Writer-preferring: once the
/// collector asks, new writers queue behind it, so it cannot be starved.
#[derive(Clone, Default)]
pub struct InProcessLock(Arc<RwInner>);

impl InProcessLock {
    /// A new, unlocked lock. Clones share the same lock.
    pub fn new() -> Self {
        Self::default()
    }
}

struct SharedGuard(Arc<RwInner>);
struct ExclusiveGuard(Arc<RwInner>);

impl Drop for SharedGuard {
    fn drop(&mut self) {
        self.0.state().readers -= 1;
        self.0.changed.notify_all();
    }
}

impl Drop for ExclusiveGuard {
    fn drop(&mut self) {
        self.0.state().writer = false;
        self.0.changed.notify_all();
    }
}

impl ObjectLock for InProcessLock {
    fn shared(&self) -> Result<LockGuard, StoreError> {
        let mut st = self.0.state();
        while st.writer || st.writers_waiting > 0 {
            st = self.0.changed.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        st.readers += 1;
        Ok(Box::new(SharedGuard(self.0.clone())))
    }

    fn exclusive(&self) -> Result<LockGuard, StoreError> {
        let mut st = self.0.state();
        st.writers_waiting += 1;
        while st.writer || st.readers > 0 {
            st = self.0.changed.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        st.writers_waiting -= 1;
        st.writer = true;
        Ok(Box::new(ExclusiveGuard(self.0.clone())))
    }
}

// ---- file ------------------------------------------------------------------------

/// An advisory `flock` on a file: spans every process on the host that opens
/// the same path (and, on a shared file system that supports it, other hosts).
#[derive(Clone, Debug)]
pub struct FileLock {
    path: PathBuf,
}

impl FileLock {
    /// A lock on `path` (created on first use).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn open(&self) -> Result<File, StoreError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Ok(OpenOptions::new().create(true).truncate(false).write(true).open(&self.path)?)
    }
}

impl ObjectLock for FileLock {
    fn shared(&self) -> Result<LockGuard, StoreError> {
        let f = self.open()?;
        f.lock_shared()?;
        Ok(Box::new(f)) // the lock goes away with the file handle
    }

    fn exclusive(&self) -> Result<LockGuard, StoreError> {
        let f = self.open()?;
        f.lock()?;
        Ok(Box::new(f))
    }
}

// ---- PostgreSQL advisory locks ---------------------------------------------------

#[derive(Default)]
struct PgHeld {
    shared: usize,
    exclusive: bool,
    exclusive_waiting: usize,
}

struct PgInner {
    key: i64,
    pool: Pool,
    /// The dedicated session holding the advisory lock. `None` until first use and
    /// after it died.
    session: Mutex<Option<Pooled>>,
    held: Mutex<PgHeld>,
    changed: Condvar,
}

/// A cross-process lock backed by PostgreSQL session-level advisory locks:
/// `pg_advisory_lock_shared` for writers, `pg_advisory_lock` for the collector.
///
/// One dedicated connection per `PostgresAdvisoryLock` holds the lock. Because
/// it is a *session* lock, a process that crashes, or loses its network, drops
/// the connection and PostgreSQL releases the lock by itself; no lease or
/// cleanup is needed. Inside one process an in-memory reader/writer count sits
/// in front of the session (PostgreSQL would let one session hold both modes at
/// once), and the session takes the shared lock only for the first of many
/// concurrent local writers.
///
/// Limits, stated plainly: if the connection dies while guards are held, the
/// protection is lost for those guards (the next acquisition notices and
/// reconnects). A collector holds its guard for one object's check-then-delete,
/// a few milliseconds.
pub struct PostgresAdvisoryLock(Arc<PgInner>);

impl PostgresAdvisoryLock {
    /// Connects to the database behind `url`. `name` identifies the object
    /// store (everything that shares one bucket prefix must use the same
    /// name, for example `"<schema>/<bucket>/<prefix>"`).
    pub fn connect(url: &str, name: &str) -> Result<Self, StoreError> {
        Self::connect_as(url, name, "agentgit-object-lock")
    }

    /// Like [`connect`](Self::connect) with an explicit `application_name` for the lock
    /// session (visible in `pg_stat_activity`).
    pub fn connect_as(url: &str, name: &str, application_name: &str) -> Result<Self, StoreError> {
        let target = pg::parse_target(url).map_err(lock_err)?;
        let connector = pg::tls_connector(&target).map_err(lock_err)?;
        let opts = PgOptions { pool_size: 1, application_name: application_name.into(), ..PgOptions::default() };
        // The schema is irrelevant here (no tables are touched).
        let pool = Pool::new(target, connector, "public", &opts);
        let lock = Self(Arc::new(PgInner {
            key: advisory_key("object-gc", name),
            pool,
            session: Mutex::new(None),
            held: Mutex::new(PgHeld::default()),
            changed: Condvar::new(),
        }));
        // Fail now, not at the first put, if the database is unreachable.
        lock.0.with_session(|_| Ok(()))?;
        Ok(lock)
    }
}

fn lock_err(e: crate::meta::MetaError) -> StoreError {
    StoreError::Lock(e.to_string())
}

impl PgInner {
    /// Runs `f` on the dedicated session, (re)connecting when it is missing or dead.
    fn session_op<T>(
        &self,
        f: impl FnOnce(&mut PgClient) -> Result<T, crate::meta::MetaError>,
    ) -> Result<T, crate::meta::MetaError> {
        let mut slot = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_mut().is_none_or(|s| s.is_dead()) {
            *slot = None; // return the dead connection to the pool, which discards it
            *slot = Some(self.pool.get()?);
        }
        let conn = slot.as_mut().expect("just ensured");
        f(conn)
    }

    fn with_session<T>(
        &self,
        f: impl FnOnce(&mut PgClient) -> Result<T, crate::meta::MetaError>,
    ) -> Result<T, StoreError> {
        self.session_op(f).map_err(lock_err)
    }

    fn sql(&self, func: &str) -> String {
        format!("SELECT {func}({})", self.key)
    }

    fn lock_session(&self, exclusive: bool) -> Result<(), StoreError> {
        let func = if exclusive { "pg_advisory_lock" } else { "pg_advisory_lock_shared" };
        let sql = self.sql(func);
        match self.session_op(|c| c.batch(&sql)) {
            // The session died while idle (the first statement is what notices): nothing was
            // locked on it, so take the lock again on a fresh one.
            Err(crate::meta::MetaError::ConnectionLost { .. }) => self.with_session(|c| c.batch(&sql)),
            other => other.map_err(lock_err),
        }
    }

    fn unlock_session(&self, exclusive: bool) {
        let func = if exclusive { "pg_advisory_unlock" } else { "pg_advisory_unlock_shared" };
        // A dead session needs no unlock: PostgreSQL already released everything it held.
        let mut slot = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(conn) = slot.as_mut().filter(|c| !c.is_dead()) {
            let _ = conn.batch(&self.sql(func));
        }
    }
}

struct PgGuard {
    inner: Arc<PgInner>,
    exclusive: bool,
}

impl Drop for PgGuard {
    fn drop(&mut self) {
        let mut held = self.inner.held.lock().unwrap_or_else(|e| e.into_inner());
        if self.exclusive {
            self.inner.unlock_session(true);
            held.exclusive = false;
        } else {
            held.shared -= 1;
            if held.shared == 0 {
                self.inner.unlock_session(false);
            }
        }
        self.inner.changed.notify_all();
    }
}

impl ObjectLock for PostgresAdvisoryLock {
    fn shared(&self) -> Result<LockGuard, StoreError> {
        let inner = &self.0;
        let mut held = inner.held.lock().unwrap_or_else(|e| e.into_inner());
        while held.exclusive || held.exclusive_waiting > 0 {
            held = inner.changed.wait(held).unwrap_or_else(|e| e.into_inner());
        }
        if held.shared == 0 {
            // May block while another process collects garbage; nobody here holds a shared guard.
            inner.lock_session(false)?;
        } else {
            // Other writers here already hold the shared lock: make sure the session
            // that holds it is still alive, and take the lock again on a new one if not.
            let alive = inner.with_session(|c| c.batch("SELECT 1")).is_ok();
            if !alive {
                inner.lock_session(false)?;
            }
        }
        held.shared += 1;
        Ok(Box::new(PgGuard { inner: self.0.clone(), exclusive: false }))
    }

    fn exclusive(&self) -> Result<LockGuard, StoreError> {
        let inner = &self.0;
        let mut held = inner.held.lock().unwrap_or_else(|e| e.into_inner());
        held.exclusive_waiting += 1;
        while held.exclusive || held.shared > 0 {
            held = inner.changed.wait(held).unwrap_or_else(|e| e.into_inner());
        }
        let locked = inner.lock_session(true);
        held.exclusive_waiting -= 1;
        if let Err(e) = locked {
            inner.changed.notify_all();
            return Err(e);
        }
        held.exclusive = true;
        Ok(Box::new(PgGuard { inner: self.0.clone(), exclusive: true }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn in_process_lock_shares_readers_and_excludes_the_writer() {
        let lock = InProcessLock::new();
        let r1 = lock.shared().unwrap();
        let r2 = lock.shared().unwrap();
        let got = Arc::new(AtomicUsize::new(0));
        let (l, g) = (lock.clone(), got.clone());
        let t = std::thread::spawn(move || {
            let _w = l.exclusive().unwrap();
            g.store(1, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(got.load(Ordering::SeqCst), 0, "the writer waits for both readers");
        drop(r1);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(got.load(Ordering::SeqCst), 0);
        drop(r2);
        t.join().unwrap();
        assert_eq!(got.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_waiting_collector_is_not_starved_by_new_writers() {
        let lock = InProcessLock::new();
        let r = lock.shared().unwrap();
        let (l, order) = (lock.clone(), Arc::new(Mutex::new(Vec::new())));
        let o = order.clone();
        let collector = std::thread::spawn(move || {
            let _g = l.exclusive().unwrap();
            o.lock().unwrap().push("collector");
        });
        std::thread::sleep(Duration::from_millis(100)); // the collector is queued
        let (l2, o2) = (lock.clone(), order.clone());
        let late_writer = std::thread::spawn(move || {
            let _g = l2.shared().unwrap();
            o2.lock().unwrap().push("late writer");
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(order.lock().unwrap().is_empty(), "both wait for the first reader");
        drop(r);
        collector.join().unwrap();
        late_writer.join().unwrap();
        assert_eq!(*order.lock().unwrap(), ["collector", "late writer"]);
    }

    #[test]
    fn a_panicking_holder_does_not_wedge_the_lock() {
        let lock = InProcessLock::new();
        let l = lock.clone();
        let _ = std::thread::spawn(move || {
            let _g = l.exclusive().unwrap();
            panic!("holder dies");
        })
        .join();
        drop(lock.exclusive().unwrap());
        drop(lock.shared().unwrap());
    }

    #[test]
    fn file_lock_excludes_across_handles() {
        let dir = std::env::temp_dir().join(crate::testing::unique("agtest-flock"));
        let a = FileLock::new(dir.join("sub/objects.lock"));
        let b = FileLock::new(dir.join("sub/objects.lock"));
        let r1 = a.shared().unwrap();
        let r2 = b.shared().unwrap();
        let got = Arc::new(AtomicUsize::new(0));
        let g = got.clone();
        let t = std::thread::spawn(move || {
            let _w = b.exclusive().unwrap();
            g.store(1, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(got.load(Ordering::SeqCst), 0);
        drop((r1, r2));
        t.join().unwrap();
        assert_eq!(got.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
