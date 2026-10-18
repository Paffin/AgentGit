//! The repository object budget.
//!
//! Every byte that enters a repository's object store is charged against the repository's quota, whichever
//! operation writes it: an upload, a virtual-workspace edit, a workspace sync, an integration that merges
//! blobs. Instead of asking each endpoint to remember to check, [`App::with_repo`](crate::app::App::with_repo)
//! hands the engine a view of the repository whose object store is a [`MeteredStore`]: a `put` of an object the
//! store does not hold yet is charged first and refused when it would pass the quota.
//!
//! Accounting is exact with respect to concurrent writers. When the repository has a total-bytes quota the
//! meter *reserves* bytes from the control database with one conditional `UPDATE` (see
//! [`Control::reserve_object_bytes`]) — in steps of [`RESERVE_STEP`], to avoid a control-plane write per
//! object — and gives back what the request did not use when it ends. Without a quota nothing is reserved; the
//! bytes written are added once, at the end. The tracked total is still an *estimate* of what is on disk
//! (garbage collection recounts it), but a quota can no longer be exceeded by racing requests.

use std::io;
use std::sync::{Arc, Mutex};

use ag_object::{ENVELOPE_HEADER_LEN, RawObject};
use ag_storage::{Durability, ObjectInfo, ObjectStatus, ObjectStore, StoreError};
use ag_types::{ObjectId, Timestamp};

use crate::control::{Control, RepoRow};
use crate::error::ApiError;

/// How many bytes a meter asks the control database for at a time when the repository has a quota. Larger
/// steps mean fewer control-plane writes; a step that does not fit is retried with the exact size of the
/// object, so a small quota is still used to the byte.
pub const RESERVE_STEP: u64 = 8 * 1024 * 1024;

/// The limits one request's writes are held to.
#[derive(Clone, Debug)]
pub struct Budget {
    /// Repository name (the key of its row in the control database).
    pub repo: String,
    /// Total-bytes quota of the repository (0 = unlimited).
    pub max_total: u64,
    /// Largest single object (0 = no limit beyond the server's own).
    pub max_object: u64,
}

impl Budget {
    /// The budget a repository's registry row describes.
    pub fn of(row: &RepoRow) -> Self {
        Self {
            repo: row.name.clone(),
            max_total: row.quota.max_total_object_bytes,
            max_object: row.quota.max_object_bytes,
        }
    }
}

#[derive(Default)]
struct State {
    /// Bytes reserved from the control database so far (quota case).
    reserved: u64,
    /// Bytes of new objects written.
    used: u64,
    /// The first reason a write was refused.
    violation: Option<ApiError>,
    finished: bool,
}

/// Counts what one request writes into a repository and enforces the repository's quota.
pub struct ObjectMeter {
    control: Arc<Control>,
    budget: Budget,
    state: Mutex<State>,
}

impl ObjectMeter {
    /// A meter for one request.
    pub fn new(control: Arc<Control>, budget: Budget) -> Arc<Self> {
        Arc::new(Self { control, budget, state: Mutex::new(State::default()) })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn refuse(&self, st: &mut State, err: ApiError, as_store_error: StoreError) -> StoreError {
        st.violation.get_or_insert(err);
        as_store_error
    }

    /// Charges a new object of `size` envelope bytes (payload `payload_len`); refuses it when it is over the
    /// per-object limit or would pass the repository quota.
    fn charge(&self, size: u64, payload_len: u64) -> Result<(), StoreError> {
        let b = &self.budget;
        let mut st = self.state();
        if st.finished {
            // Writing after the request ended (a leaked view) would go uncounted: refuse rather than miscount.
            return Err(StoreError::Io(io::Error::other("the request that owned this repository view has ended")));
        }
        if b.max_object > 0 && size > b.max_object {
            let err = ApiError::new(
                ag_protocol::ErrorCode::TooLarge,
                format!(
                    "an object of {size} bytes exceeds this repository's limit of {} bytes per object",
                    b.max_object
                ),
            );
            return Err(self.refuse(&mut st, err, StoreError::TooLarge { size: payload_len, limit: b.max_object }));
        }
        if b.max_total > 0 && st.used + size > st.reserved {
            // Ask for a step, or for exactly what is needed if the step does not fit.
            let step = size.max(RESERVE_STEP);
            let reserve = |bytes: u64| self.control.reserve_object_bytes(&b.repo, bytes, b.max_total);
            let granted = match reserve(step) {
                Ok(true) => Ok(step),
                Ok(false) if step > size => reserve(size).map(|ok| if ok { size } else { 0 }),
                Ok(false) => Ok(0),
                Err(e) => Err(e),
            };
            match granted {
                Ok(0) => {
                    let err = ApiError::new(
                        ag_protocol::ErrorCode::QuotaExceeded,
                        format!("this repository's object quota of {} bytes would be exceeded", b.max_total),
                    );
                    let cause = StoreError::Io(io::Error::new(io::ErrorKind::QuotaExceeded, "object quota exceeded"));
                    return Err(self.refuse(&mut st, err, cause));
                }
                Ok(n) => st.reserved += n,
                Err(e) => {
                    let cause = StoreError::Io(io::Error::other("the control database is unavailable"));
                    return Err(self.refuse(&mut st, e, cause));
                }
            }
        }
        st.used += size;
        Ok(())
    }

    /// The first refusal, if a write was refused during the request.
    pub fn violation(&self) -> Option<ApiError> {
        self.state().violation.clone()
    }

    /// Bytes of new objects the request wrote.
    pub fn bytes_written(&self) -> u64 {
        self.state().used
    }

    /// Settles the account with the control database: adds the bytes written (no quota) or hands back the
    /// unused part of the reservation (quota). Idempotent; later writes through the view are refused.
    pub fn finish(&self) {
        let (reserved, used) = {
            let mut st = self.state();
            if st.finished {
                return;
            }
            st.finished = true;
            (st.reserved, st.used)
        };
        let delta = if self.budget.max_total > 0 { -((reserved - used.min(reserved)) as i64) } else { used as i64 };
        if delta != 0
            && let Err(e) = self.control.add_object_bytes(&self.budget.repo, delta)
        {
            // The tracked total drifts until the next garbage collection recounts it; nothing else is harmed.
            tracing::warn!(repo = %self.budget.repo, error = %e.0.message, delta, "could not settle the object byte count");
        }
    }
}

/// Ends the meter when dropped, so a panic in the engine call cannot leave a reservation behind.
pub struct MeterGuard(pub Arc<ObjectMeter>);

impl Drop for MeterGuard {
    fn drop(&mut self) {
        self.0.finish();
    }
}

/// An object store that charges new objects to an [`ObjectMeter`] before storing them.
pub struct MeteredStore {
    inner: Arc<dyn ObjectStore>,
    meter: Arc<ObjectMeter>,
}

impl MeteredStore {
    /// Wraps `inner`.
    pub fn new(inner: Arc<dyn ObjectStore>, meter: Arc<ObjectMeter>) -> Self {
        Self { inner, meter }
    }
}

impl ObjectStore for MeteredStore {
    fn put(&self, obj: &RawObject) -> Result<ObjectId, StoreError> {
        // An object the store already holds costs nothing (and is still written, which heals a damaged copy).
        if !self.inner.contains(&obj.id())? {
            let payload = obj.payload.len() as u64;
            self.meter.charge(ENVELOPE_HEADER_LEN as u64 + payload, payload)?;
        }
        self.inner.put(obj)
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
    fn payload_len(&self, id: &ObjectId) -> Result<Option<u64>, StoreError> {
        self.inner.payload_len(id)
    }
    fn delete(&self, id: &ObjectId) -> Result<bool, StoreError> {
        self.inner.delete(id)
    }
    fn delete_if_older_than(&self, id: &ObjectId, cutoff: Timestamp) -> Result<bool, StoreError> {
        self.inner.delete_if_older_than(id, cutoff)
    }
    fn block_gc(&self) -> Result<ag_storage::lock::LockGuard, StoreError> {
        self.inner.block_gc()
    }
    fn durability(&self) -> Durability {
        self.inner.durability()
    }
}

#[cfg(test)]
mod tests {
    use ag_protocol::ErrorCode;
    use ag_storage::{MemoryObjectStore, SystemClock};

    use super::*;

    fn fixture(max_total: u64, max_object: u64) -> (Arc<Control>, Arc<dyn ObjectStore>, Budget) {
        let control = Arc::new(Control::open_in_memory(Arc::new(SystemClock)).unwrap());
        let quota = ag_protocol::v1::Quota {
            max_total_object_bytes: max_total,
            max_object_bytes: max_object,
            ..Default::default()
        };
        let row = RepoRow {
            name: "acme/web".into(),
            namespace: "acme/web".into(),
            default_stream: "main".into(),
            description: String::new(),
            created_at_ms: 1,
            created_by: "user:root".into(),
            quota,
            object_bytes: 0,
            config_json: String::new(),
        };
        control.insert_repo(&row).unwrap();
        let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
        (control, store, Budget::of(&row))
    }

    fn tracked(c: &Control) -> u64 {
        c.repo("acme/web").unwrap().unwrap().object_bytes
    }

    /// A blob of `n` payload bytes; distinct `seed`s give distinct objects.
    fn blob(seed: u32, n: usize) -> RawObject {
        let mut data = seed.to_be_bytes().to_vec();
        data.resize(n, b'x');
        RawObject::blob(data)
    }

    const HEADER: u64 = ENVELOPE_HEADER_LEN as u64;

    #[test]
    fn new_objects_are_charged_and_objects_already_held_are_free() {
        let (control, store, budget) = fixture(0, 0);
        let meter = ObjectMeter::new(control.clone(), budget);
        let metered = MeteredStore::new(store.clone(), meter.clone());
        metered.put(&blob(1, 100)).unwrap();
        metered.put(&blob(2, 50)).unwrap();
        metered.put(&blob(1, 100)).unwrap(); // the same object again
        assert_eq!(meter.bytes_written(), 2 * HEADER + 150);
        assert_eq!(tracked(&control), 0, "nothing is written to the control database until the request ends");
        meter.finish();
        assert_eq!(tracked(&control), 2 * HEADER + 150);
        meter.finish();
        assert_eq!(tracked(&control), 2 * HEADER + 150, "settling twice does not count twice");
        // The same object through another request: already stored, free.
        let again = ObjectMeter::new(control.clone(), meter.budget.clone());
        MeteredStore::new(store, again.clone()).put(&blob(2, 50)).unwrap();
        again.finish();
        assert_eq!(tracked(&control), 2 * HEADER + 150);
    }

    #[test]
    fn a_total_quota_refuses_the_object_that_would_pass_it_and_keeps_only_what_was_written() {
        let one = HEADER + 300;
        let (control, store, budget) = fixture(3 * one + 10, 0);
        let meter = ObjectMeter::new(control.clone(), budget);
        let metered = MeteredStore::new(store.clone(), meter.clone());
        for seed in 0..3 {
            metered.put(&blob(seed, 300)).unwrap();
        }
        let refused = metered.put(&blob(9, 300)).unwrap_err();
        assert!(matches!(refused, StoreError::Io(ref e) if e.kind() == io::ErrorKind::QuotaExceeded), "{refused:?}");
        assert!(!store.contains(&blob(9, 300).id()).unwrap(), "a refused object is not stored");
        let violation = meter.violation().expect("the refusal is recorded for the request");
        assert_eq!(violation.code(), ErrorCode::QuotaExceeded);
        // A smaller object still fits in what is left.
        metered.put(&blob(10, 3)).unwrap();
        meter.finish();
        assert_eq!(tracked(&control), 3 * one + HEADER + 3, "reserved bytes that went unused were given back");
    }

    #[test]
    fn a_roomy_quota_reserves_in_steps_and_hands_back_what_the_request_did_not_use() {
        let (control, store, budget) = fixture(10 * RESERVE_STEP, 0);
        let meter = ObjectMeter::new(control.clone(), budget);
        MeteredStore::new(store, meter.clone()).put(&blob(1, 1000)).unwrap();
        assert_eq!(tracked(&control), RESERVE_STEP, "a step is held while the request runs");
        drop(MeterGuard(meter.clone())); // what `with_repo` does when the engine call returns (or panics)
        assert_eq!(tracked(&control), HEADER + 1000);
    }

    #[test]
    fn the_object_limit_of_a_repository_is_enforced_on_every_write() {
        let (control, store, budget) = fixture(0, 500);
        let meter = ObjectMeter::new(control, budget);
        let metered = MeteredStore::new(store, meter.clone());
        metered.put(&blob(1, 400)).unwrap();
        assert!(matches!(metered.put(&blob(2, 600)), Err(StoreError::TooLarge { limit: 500, .. })));
        assert_eq!(meter.violation().unwrap().code(), ErrorCode::TooLarge);
    }

    #[test]
    fn racing_writers_cannot_pass_the_quota_between_them() {
        let one = HEADER + 1000;
        let quota = 40 * one + 123;
        let (control, store, budget) = fixture(quota, 0);
        let stored: u64 = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8u32)
                .map(|t| {
                    let (control, store, budget) = (control.clone(), store.clone(), budget.clone());
                    s.spawn(move || {
                        let meter = ObjectMeter::new(control, budget);
                        let metered = MeteredStore::new(store, meter.clone());
                        let mut ok = 0;
                        for i in 0..20u32 {
                            if metered.put(&blob(t * 100 + i, 1000)).is_ok() {
                                ok += one;
                            }
                        }
                        meter.finish();
                        ok
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).sum()
        });
        assert!(stored <= quota, "{stored} bytes were stored under a quota of {quota}");
        assert_eq!(stored, 40 * one, "160 offered, exactly the 40 that fit were stored");
        assert_eq!(tracked(&control), stored, "and the tracked total is what was stored");
        assert_eq!(store.list().unwrap().len(), 40);
    }

    #[test]
    fn a_view_that_outlives_its_request_cannot_write_uncounted() {
        let (control, store, budget) = fixture(0, 0);
        let meter = ObjectMeter::new(control, budget);
        let metered = MeteredStore::new(store, meter.clone());
        meter.finish();
        assert!(metered.put(&blob(1, 10)).is_err());
    }
}
