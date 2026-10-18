use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A point in time as milliseconds since the Unix epoch (UTC).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(i64);

impl Timestamp {
    /// Builds a timestamp from Unix milliseconds.
    pub const fn from_millis(ms: i64) -> Self {
        Self(ms)
    }

    /// Unix milliseconds.
    pub const fn as_millis(self) -> i64 {
        self.0
    }

    /// The current wall-clock time. A clock set before 1970 gives a negative timestamp (what it says), not zero.
    pub fn now() -> Self {
        Self(millis_since_epoch(SystemTime::now()))
    }

    /// ISO-8601 / RFC 3339 representation in UTC with millisecond precision.
    pub fn to_rfc3339(self) -> String {
        let secs = self.0.div_euclid(1000);
        let ms = self.0.rem_euclid(1000);
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Timestamp({})", self.to_rfc3339())
    }
}

impl From<SystemTime> for Timestamp {
    /// The instant `t` in Unix milliseconds (negative before 1970, saturating at the `i64` range).
    fn from(t: SystemTime) -> Self {
        Self(millis_since_epoch(t))
    }
}

/// Signed milliseconds from the epoch to `t`, saturating at the `i64` range (about 292 million years: unreachable
/// for a real clock, but never a silently wrapped value).
fn millis_since_epoch(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_millis()).map(|ms| -ms).unwrap_or(i64::MIN),
    }
}

/// Days since 1970-01-01 to (year, month, day) in the proleptic Gregorian
/// calendar (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_epoch_and_known_dates() {
        assert_eq!(Timestamp::from_millis(0).to_rfc3339(), "1970-01-01T00:00:00.000Z");
        assert_eq!(Timestamp::from_millis(1_760_659_200_123).to_rfc3339(), "2025-10-17T00:00:00.123Z");
        assert_eq!(Timestamp::from_millis(-1).to_rfc3339(), "1969-12-31T23:59:59.999Z");
    }

    #[test]
    fn a_clock_before_the_epoch_is_negative_not_zero() {
        use std::time::Duration;
        assert_eq!(millis_since_epoch(UNIX_EPOCH - Duration::from_millis(1500)), -1500);
        assert_eq!(millis_since_epoch(UNIX_EPOCH), 0);
        assert_eq!(millis_since_epoch(UNIX_EPOCH + Duration::from_millis(1500)), 1500);
        assert_eq!(Timestamp::from(UNIX_EPOCH - Duration::from_millis(1)).as_millis(), -1);
    }
}
