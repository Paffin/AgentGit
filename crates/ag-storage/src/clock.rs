//! Injectable time source so expiry, grace periods and retention are testable.

use std::sync::atomic::{AtomicI64, Ordering};

use ag_types::Timestamp;

/// A source of the current time.
pub trait Clock: Send + Sync {
    /// The current time.
    fn now(&self) -> Timestamp;
}

/// The system wall clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// A clock that only moves when told to.
#[derive(Debug)]
pub struct ManualClock(AtomicI64);

impl ManualClock {
    /// Starts at `start`.
    pub fn new(start: Timestamp) -> Self {
        Self(AtomicI64::new(start.as_millis()))
    }

    /// Moves time forward by `ms` milliseconds.
    pub fn advance_ms(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }

    /// Sets the time.
    pub fn set(&self, t: Timestamp) {
        self.0.store(t.as_millis(), Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.0.load(Ordering::SeqCst))
    }
}
