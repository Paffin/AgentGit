//! A global budget for object bytes held in memory while they are being received.
//!
//! Objects are buffered whole because they are verified before they are stored, so the memory a server needs for
//! uploads is the sum of the objects in flight. `limits.max_concurrent_requests` bounds how many there are, but
//! times `limits.max_object_bytes` that is a very large number; this budget bounds the bytes themselves
//! (`limits.max_inflight_object_bytes`). An upload that would pass it is refused with `unavailable` and a retry
//! hint before its body is read, and whatever it held is returned when it ends, however it ends.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use prometheus::{IntCounter, IntGauge};

use crate::error::{ApiError, ApiResult};

/// How long a refused client is told to wait.
const RETRY_AFTER_MS: i64 = 250;

/// The bytes that may be held at once (0 = no limit) and how many are.
pub struct InflightBytes {
    limit: u64,
    used: AtomicU64,
    gauge: IntGauge,
    refused: IntCounter,
}

impl InflightBytes {
    /// A budget of `limit` bytes (0 = unlimited) that reports to `gauge` and `refused`.
    pub fn new(limit: u64, gauge: IntGauge, refused: IntCounter) -> Arc<Self> {
        Arc::new(Self { limit, used: AtomicU64::new(0), gauge, refused })
    }

    /// Bytes held right now.
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }

    /// Reserves `bytes`, or refuses. The reservation is returned to the budget when dropped.
    pub fn reserve(self: &Arc<Self>, bytes: u64) -> ApiResult<Reservation> {
        let mut held = Reservation { budget: self.clone(), bytes: 0 };
        held.grow(bytes)?;
        Ok(held)
    }

    fn take(&self, bytes: u64) -> bool {
        if self.limit == 0 {
            self.used.fetch_add(bytes, Ordering::AcqRel);
        } else {
            let mut current = self.used.load(Ordering::Acquire);
            loop {
                match current.checked_add(bytes).filter(|total| *total <= self.limit) {
                    None => return false,
                    Some(total) => {
                        match self.used.compare_exchange_weak(current, total, Ordering::AcqRel, Ordering::Acquire) {
                            Ok(_) => break,
                            Err(now) => current = now,
                        }
                    }
                }
            }
        }
        self.gauge.set(self.used() as i64);
        true
    }

    fn give_back(&self, bytes: u64) {
        self.used.fetch_sub(bytes, Ordering::AcqRel);
        self.gauge.set(self.used() as i64);
    }
}

/// Bytes taken from an [`InflightBytes`]; given back on drop.
pub struct Reservation {
    budget: Arc<InflightBytes>,
    bytes: u64,
}

impl std::fmt::Debug for Reservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Reservation({} bytes)", self.bytes)
    }
}

impl Reservation {
    /// Takes `more` bytes on top of what is held, or refuses (and keeps what it had).
    pub fn grow(&mut self, more: u64) -> ApiResult<()> {
        if !self.budget.take(more) {
            self.budget.refused.inc();
            return Err(ApiError::new(
                ag_protocol::ErrorCode::Unavailable,
                "the server is already holding as many uploaded bytes as it is allowed to; retry shortly",
            )
            .with_retry_after(RETRY_AFTER_MS));
        }
        self.bytes += more;
        Ok(())
    }

    /// Gives everything back now.
    pub fn release(&mut self) {
        self.budget.give_back(self.bytes);
        self.bytes = 0;
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(limit: u64) -> Arc<InflightBytes> {
        InflightBytes::new(
            limit,
            IntGauge::new("t_used", "used").unwrap(),
            IntCounter::new("t_refused", "refused").unwrap(),
        )
    }

    #[test]
    fn reservations_add_up_to_the_limit_and_come_back_when_dropped() {
        let b = budget(100);
        let a = b.reserve(60).unwrap();
        let c = b.reserve(40).unwrap();
        assert_eq!(b.used(), 100);
        let refused = b.reserve(1).unwrap_err();
        assert_eq!(refused.code(), ag_protocol::ErrorCode::Unavailable);
        assert!(refused.0.retry_after_ms > 0);
        drop(a);
        assert_eq!(b.used(), 40);
        let mut again = b.reserve(60).unwrap();
        assert!(again.grow(1).is_err(), "growing is refused too, and keeps what it held");
        assert_eq!(b.used(), 100);
        again.release();
        assert_eq!(b.used(), 40);
        drop(c);
        assert_eq!(b.used(), 0);
        assert_eq!(b.refused.get(), 2);
    }

    #[test]
    fn zero_means_no_limit_but_the_bytes_are_still_counted() {
        let b = budget(0);
        let _a = b.reserve(u64::MAX / 4).unwrap();
        assert_eq!(b.used(), u64::MAX / 4);
    }

    #[test]
    fn racing_reservations_never_pass_the_limit() {
        let b = budget(1000);
        let granted: usize = std::thread::scope(|s| {
            let handles: Vec<_> = (0..16)
                .map(|_| {
                    let b = &b;
                    s.spawn(move || (0..100).filter_map(|_| b.reserve(10).ok()).collect::<Vec<_>>())
                })
                .collect();
            let all: Vec<Vec<Reservation>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            all.iter().map(Vec::len).sum()
        });
        assert_eq!(granted, 100, "exactly limit / size reservations fit, however the threads interleave");
        assert_eq!(b.used(), 0, "all returned");
    }
}
