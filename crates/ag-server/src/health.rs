//! What the repositories report about their own condition, beyond "the process is up": the post-commit derive
//! pass (`Repository::derive_status`: events still pending, why the last pass was incomplete) and the references
//! that keep changesets alive but point at nothing (`Repository::dangling_refs`: `dangling_pin`,
//! `dangling_proposal_head`, `dangling_workspace_ref`).
//!
//! A scan visits every registered repository, so it runs in the background every `repos.health_scan_interval_secs`
//! and `/metrics` and `/readyz` report its last result instead of reading every repository per request. None of
//! this makes a replica unready: derived data being behind, or a pin pointing at a lost object, is something to
//! fix, not a reason to stop serving.

use std::collections::BTreeMap;

use crate::app::App;

/// The result of one scan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Health {
    /// When the scan finished (ms since the epoch).
    pub scanned_at_ms: i64,
    /// Repositories that were read.
    pub repos: usize,
    /// Repositories that could not be opened or read (their figures are missing from the rest).
    pub unreadable: usize,
    /// Outbox events still pending, over all repositories.
    pub derive_pending: u64,
    /// Repositories whose last post-commit pass was incomplete.
    pub derive_failing: usize,
    /// Dangling references by kind (`dangling_pin`, ...), over all repositories; absent kinds are zero.
    pub dangling: BTreeMap<String, u64>,
}

/// The kinds `Repository::dangling_refs` reports; exported as a metric label even at zero.
pub const DANGLING_KINDS: [&str; 3] = ["dangling_pin", "dangling_proposal_head", "dangling_workspace_ref"];

impl Health {
    /// Dangling references of all kinds.
    pub fn dangling_total(&self) -> u64 {
        self.dangling.values().sum()
    }

    /// Why this is not a clean bill of health, in one line (`None` when it is one).
    pub fn problems(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.derive_failing > 0 || self.derive_pending > 0 {
            parts.push(format!(
                "{} derive event(s) pending, {} repositories with an incomplete derive pass",
                self.derive_pending, self.derive_failing
            ));
        }
        if self.dangling_total() > 0 {
            let kinds: Vec<String> =
                self.dangling.iter().filter(|(_, n)| **n > 0).map(|(k, n)| format!("{n} {k}")).collect();
            parts.push(format!("dangling references: {}", kinds.join(", ")));
        }
        if self.unreadable > 0 {
            parts.push(format!("{} repositories could not be read", self.unreadable));
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    }
}

impl App {
    /// Reads every registered repository (blocking) and returns what they report. A repository that cannot be
    /// opened or read is counted in `unreadable` and logged; the figures of the others stand.
    pub fn scan_health(&self) -> Result<Health, String> {
        let rows = self.control.all_repos().map_err(|e| e.0.message)?;
        let mut h = Health { scanned_at_ms: self.clock.now().as_millis(), ..Health::default() };
        for kind in DANGLING_KINDS {
            h.dangling.insert(kind.into(), 0);
        }
        for row in rows {
            let scanned = self.repos.handle(&row.name).map_err(|e| e.0.message).and_then(|handle| {
                let derive = handle.derive_status().map_err(|e| e.to_string())?;
                let dangling = handle.dangling_refs().map_err(|e| e.to_string())?;
                Ok((derive, dangling))
            });
            match scanned {
                Ok((derive, dangling)) => {
                    h.repos += 1;
                    h.derive_pending += derive.pending;
                    if derive.last_error.is_some() {
                        h.derive_failing += 1;
                    }
                    for issue in dangling {
                        *h.dangling.entry(issue.kind).or_default() += 1;
                    }
                }
                Err(e) => {
                    h.unreadable += 1;
                    tracing::warn!(repo = %row.name, error = %e, "health scan: cannot read the repository");
                }
            }
        }
        Ok(h)
    }

    /// Scans, remembers the result and publishes it as metrics. Returns what was found.
    pub async fn refresh_health(&self) -> Option<Health> {
        let app = self.clone();
        let scanned = tokio::task::spawn_blocking(move || app.scan_health()).await;
        let h = match scanned {
            Ok(Ok(h)) => h,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "health scan: cannot list the repositories");
                return None;
            }
            Err(e) => {
                tracing::error!(error = %e, "health scan: the task failed");
                return None;
            }
        };
        let m = &self.metrics;
        m.derive_pending.set(h.derive_pending as i64);
        m.derive_failing_repos.set(h.derive_failing as i64);
        m.health_unreadable_repos.set(h.unreadable as i64);
        m.health_scan_time.set(h.scanned_at_ms / 1000);
        for (kind, n) in &h.dangling {
            m.dangling_refs.with_label_values(&[kind.as_str()]).set(*n as i64);
        }
        if let Some(why) = h.problems() {
            tracing::warn!(problems = %why, "health scan: repositories need attention");
        }
        *self.health.lock().unwrap_or_else(|e| e.into_inner()) = Some(h.clone());
        Some(h)
    }

    /// The result of the last scan, if one has finished.
    pub fn last_health(&self) -> Option<Health> {
        self.health.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_scan_has_no_problems_and_each_kind_is_named() {
        let mut h = Health::default();
        assert_eq!(h.problems(), None);
        h.derive_pending = 3;
        h.derive_failing = 1;
        h.dangling.insert("dangling_pin".into(), 2);
        h.dangling.insert("dangling_proposal_head".into(), 0);
        h.unreadable = 1;
        let why = h.problems().unwrap();
        assert!(
            why.contains("3 derive event(s) pending") && why.contains("1 repositories with an incomplete"),
            "{why}"
        );
        assert!(why.contains("2 dangling_pin") && !why.contains("dangling_proposal_head"), "{why}");
        assert!(why.contains("1 repositories could not be read"), "{why}");
    }
}
