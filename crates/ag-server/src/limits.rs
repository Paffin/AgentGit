//! Rate limiting and concurrency control.
//!
//! Each caller - a credential, or for OIDC sign-ins the identity (see `app::bucket_key`) - has a token bucket;
//! repositories may carry their own quota. Exceeding a limit yields
//! `rate_limited` with the exact delay after which a retry can succeed, which
//! is the backpressure signal clients and SDKs honour.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use crate::error::{ApiError, ApiResult};

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Token buckets keyed by caller (and optionally repository).
pub struct RateLimiter {
    rate: f64,
    burst: f64,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    /// `rate` requests per second with bursts up to `burst`; `rate == 0` disables limiting.
    pub fn new(rate: u32, burst: u32) -> Self {
        Self { rate: f64::from(rate), burst: f64::from(burst.max(1)), buckets: Mutex::new(HashMap::new()) }
    }

    /// Takes one token for `key` (with an optional per-key override).
    pub fn check(&self, key: &str, override_rate: Option<(u32, u32)>) -> ApiResult<()> {
        let (rate, burst) = match override_rate {
            Some((r, b)) if r > 0 => (f64::from(r), f64::from(b.max(1))),
            _ => (self.rate, self.burst),
        };
        if rate <= 0.0 {
            return Ok(());
        }
        let now = Instant::now();
        let mut map = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() > 50_000 {
            // Drop idle buckets so a flood of distinct keys cannot grow memory without bound.
            map.retain(|_, b| now.duration_since(b.last).as_secs() < 300);
        }
        let b = map.entry(key.to_string()).or_insert(Bucket { tokens: burst, last: now });
        let elapsed = now.duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + elapsed * rate).min(burst);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            let wait_ms = (((1.0 - b.tokens) / rate) * 1000.0).ceil().max(1.0) as i64;
            Err(ApiError::new(ag_protocol::ErrorCode::RateLimited, "rate limit exceeded; slow down")
                .with_retry_after(wait_ms))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bursts_are_allowed_then_limited_with_a_retry_hint() {
        let l = RateLimiter::new(10, 3);
        for _ in 0..3 {
            l.check("a", None).unwrap();
        }
        let e = l.check("a", None).unwrap_err();
        assert_eq!(e.code(), ag_protocol::ErrorCode::RateLimited);
        assert!(e.0.retry_after_ms > 0 && e.0.retry_after_ms <= 100);
        l.check("b", None).unwrap(); // other callers are unaffected
    }

    #[test]
    fn zero_rate_means_unlimited_and_overrides_apply() {
        let l = RateLimiter::new(0, 1);
        for _ in 0..1000 {
            l.check("a", None).unwrap();
        }
        assert!(l.check("x", Some((1, 1))).is_ok());
        assert!(l.check("x", Some((1, 1))).is_err());
    }

    #[test]
    fn tokens_refill_over_time() {
        let l = RateLimiter::new(1000, 1);
        l.check("a", None).unwrap();
        assert!(l.check("a", None).is_err());
        std::thread::sleep(std::time::Duration::from_millis(5));
        l.check("a", None).unwrap();
    }
}
