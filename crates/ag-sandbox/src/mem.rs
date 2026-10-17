//! Memory accounting for the namespace sandbox when no cgroup is available.
//!
//! The supervisor reads the sandbox's *own* `/proc` through the init process's root
//! (`/proc/<init>/root/proc`), so it sees exactly the processes of the PID namespace and nothing else.
//!
//! The memory figure is the sum over all processes of **resident anonymous memory plus resident shared memory**
//! (`RssAnon + RssShmem` in `/proc/<pid>/status`). File-backed pages (program text, mapped libraries) are
//! deliberately left out: the kernel can drop them under pressure, and counting them would charge every
//! process again for the same shared libraries. Pages shared between a forked parent and child are still
//! counted once per process, so multi-process workloads are over- rather than under-counted.
//! The same listing yields the number of tasks (`Threads:` of every process).
//! The sandbox's own init process (pid 1 there) is skipped: it is a copy of the host process and its
//! copy-on-write pages are not the command's memory.

use std::fs;
use std::path::PathBuf;

use crate::supervise::Sample;

/// Reads the memory use of the sandbox whose init has the host pid `init`.
pub(crate) struct Sampler {
    proc_dir: PathBuf,
}

impl Sampler {
    pub(crate) fn new(init: libc::pid_t) -> Self {
        Sampler { proc_dir: PathBuf::from(format!("/proc/{init}/root/proc")) }
    }

    /// Whether the sandbox's `/proc` can be read at all (permissions, `hidepid`, a missing procfs).
    pub(crate) fn readable(&self) -> bool {
        fs::read_dir(&self.proc_dir).is_ok()
    }

    /// What the sandbox uses right now, or `None` if its `/proc` cannot be read (it is gone or inaccessible).
    pub(crate) fn sample(&self) -> Option<Sample> {
        let mut sample = Sample::default();
        for entry in fs::read_dir(&self.proc_dir).ok()? {
            let Ok(entry) = entry else { continue };
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name == "1" || !name.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            // A process may exit between listing and reading; that simply contributes nothing.
            if let Ok(status) = fs::read_to_string(entry.path().join("status")) {
                sample.memory = sample.memory.saturating_add(resident_bytes(&status));
                sample.tasks += threads(&status);
            }
        }
        Some(sample)
    }
}

/// The `Threads:` line of a `/proc/<pid>/status` (every process is at least one task).
pub(crate) fn threads(status: &str) -> u64 {
    status.lines().find_map(|l| l.strip_prefix("Threads:")?.trim().parse().ok()).unwrap_or(1)
}

/// `RssAnon + RssShmem` from the text of a `/proc/<pid>/status`, in bytes.
pub(crate) fn resident_bytes(status: &str) -> u64 {
    let mut total = 0u64;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("RssAnon:").or_else(|| line.strip_prefix("RssShmem:")) {
            let kb: u64 = rest.trim().trim_end_matches("kB").trim().parse().unwrap_or(0);
            total = total.saturating_add(kb.saturating_mul(1024));
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_anonymous_and_shared_memory_only() {
        let status = "Name:\tx\nVmRSS:\t  9999 kB\nRssAnon:\t    100 kB\nRssFile:\t   5000 kB\nRssShmem:\t     20 kB\nThreads:\t1\n";
        assert_eq!(resident_bytes(status), 120 * 1024);
        assert_eq!(resident_bytes("Name:\tx\n"), 0);
    }

    #[test]
    fn counts_threads() {
        assert_eq!(threads("Name:\tx\nThreads:\t17\nSigQ:\t0/1\n"), 17);
        assert_eq!(threads("Name:\tx\n"), 1);
    }

    #[test]
    fn our_own_process_is_measurable() {
        let status = fs::read_to_string("/proc/self/status").unwrap();
        // present on every kernel since 4.5; the sampler relies on it
        assert!(status.contains("RssAnon:"), "this kernel has no RssAnon accounting");
        assert!(resident_bytes(&status) > 0);
    }
}
