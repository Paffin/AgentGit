//! Summaries of timing samples. Percentiles are nearest-rank over the sorted samples: no interpolation, so a
//! reported p99 is a latency that really happened.

use std::time::Duration;

use serde::Serialize;

/// Distribution of a set of durations, in milliseconds.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Summary {
    /// Number of samples.
    pub n: usize,
    /// Smallest.
    pub min_ms: f64,
    /// Median (nearest rank).
    pub p50_ms: f64,
    /// 90th percentile.
    pub p90_ms: f64,
    /// 99th percentile.
    pub p99_ms: f64,
    /// Largest.
    pub max_ms: f64,
    /// Arithmetic mean.
    pub mean_ms: f64,
}

/// The nearest-rank percentile `p` (0..=100) of an ascending, non-empty slice.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

impl Summary {
    /// Summarises `samples`; all zeros for an empty set.
    pub fn of(samples: &[Duration]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut ms: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
        ms.sort_by(|a, b| a.total_cmp(b));
        Self {
            n: ms.len(),
            min_ms: ms[0],
            p50_ms: percentile(&ms, 50.0),
            p90_ms: percentile(&ms, 90.0),
            p99_ms: percentile(&ms, 99.0),
            max_ms: ms[ms.len() - 1],
            mean_ms: ms.iter().sum::<f64>() / ms.len() as f64,
        }
    }
}

/// Milliseconds with three significant digits for both microsecond and second values.
pub fn fmt_ms(ms: f64) -> String {
    if ms >= 100.0 {
        format!("{ms:.0}")
    } else if ms >= 10.0 {
        format!("{ms:.1}")
    } else if ms >= 1.0 {
        format!("{ms:.2}")
    } else {
        format!("{ms:.3}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn nearest_rank_percentiles() {
        let s = Summary::of(&(1..=100).map(ms).collect::<Vec<_>>());
        assert_eq!((s.n, s.min_ms, s.p50_ms, s.p90_ms, s.p99_ms, s.max_ms), (100, 1.0, 50.0, 90.0, 99.0, 100.0));
        assert!((s.mean_ms - 50.5).abs() < 1e-9);
    }

    #[test]
    fn small_sets_never_invent_a_value() {
        let s = Summary::of(&[ms(7)]);
        assert_eq!((s.p50_ms, s.p99_ms, s.max_ms), (7.0, 7.0, 7.0));
        let s = Summary::of(&[ms(3), ms(9)]);
        assert_eq!((s.p50_ms, s.p99_ms), (3.0, 9.0));
        assert_eq!(Summary::of(&[]).n, 0);
    }

    #[test]
    fn formatting_keeps_three_digits() {
        assert_eq!(fmt_ms(0.1234), "0.123");
        assert_eq!(fmt_ms(12.34), "12.3");
        assert_eq!(fmt_ms(1234.5), "1234");
    }
}
