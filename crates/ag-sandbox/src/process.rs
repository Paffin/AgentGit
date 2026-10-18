//! The honest fallback: a plain child process with a scrubbed environment and resource limits.
//!
//! This is **not isolation**. The command runs as the calling user with that user's file system, network and
//! process visibility. What it adds over `std::process::Command`: nothing is inherited (environment,
//! descriptors, signal dispositions), `no_new_privs` is set, resource limits apply, the command leads its own
//! session and process group so a timeout can kill its descendants, output is capped, and the wall clock is
//! enforced. It reports [`IsolationLevel::None`].

use std::time::Instant;

use crate::digest::environment_digest;
use crate::error::SandboxError;
use crate::outcome::{Enforcement, IsolationLevel, IsolationReport, Outcome};
use crate::planning::{exec_inputs, path_cstring};
use crate::spawn::{self, Plan, Res};
use crate::spec::{Network, SandboxSpec};
use crate::supervise::{Hooks, classify, supervise};
use crate::sys;
use crate::{Sandbox, spawn::stage};

/// Runs commands as plain child processes with limits only. See the module documentation.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessSandbox {
    _private: (),
}

impl ProcessSandbox {
    /// A process sandbox. It is available on every Unix host.
    pub fn new() -> Self {
        ProcessSandbox { _private: () }
    }
}

impl Sandbox for ProcessSandbox {
    fn name(&self) -> &'static str {
        "process"
    }

    fn isolation(&self) -> IsolationLevel {
        IsolationLevel::None
    }

    fn run_cancellable(&self, spec: &SandboxSpec, cancel: &(dyn Fn() -> bool + Sync)) -> Result<Outcome, SandboxError> {
        spec.validate()?;
        let started = Instant::now();
        let (argv, envp, exec_paths) = exec_inputs(spec)?;
        let l = &spec.limits;
        let mut notes = vec![
            "limits only, not isolation: the command has the host user's file system, network and process visibility".to_string(),
            "ro_mounts and rw_tmp_bytes are not applied; the working directory is the host path of `workdir`, not /work".to_string(),
        ];
        let mut rlimits = vec![
            (Res::Cpu, l.cpu_s, l.cpu_s.saturating_add(1)),
            (Res::Nofile, l.open_files, l.open_files),
            (Res::Fsize, l.file_size_bytes, l.file_size_bytes),
            (Res::Core, 0, 0),
        ];
        // macOS accepts `RLIMIT_DATA` but does not enforce it (and `RLIMIT_AS` is refused), so there it is not
        // applied and the report says that memory is unlimited.
        let memory_limited = cfg!(target_os = "linux");
        if memory_limited {
            rlimits.push((Res::Data, l.memory_bytes, l.memory_bytes));
        }
        let mut task_limited = false;
        match task_limit(l.pids) {
            Some(limit) => {
                rlimits.push((Res::Nproc, limit, limit));
                task_limited = true;
            }
            None => notes.push("the task limit is not applied on this platform".into()),
        }
        if sys::euid_egid().0 == 0 {
            notes.push("running as root: the kernel exempts root from RLIMIT_NPROC, so `pids` has no effect".into());
            task_limited = false;
        }
        if spec.network == Network::Host {
            notes.push("the network is not restricted".into());
        }
        if memory_limited {
            notes.push("memory: per-process RLIMIT_DATA only; no aggregate sampling, no OOM classification".into());
        } else {
            notes.push("memory is NOT limited on this platform".into());
        }

        let plan = Plan {
            clone_flags: 0,
            ns: None,
            argv,
            envp,
            exec_paths,
            cwd: path_cstring(&spec.workdir)?,
            rlimits,
            seccomp: Vec::new(),
            no_new_privs: true,
            probe_only: false,
        };
        let mut child = spawn::spawn(&plan).map_err(|e| SandboxError::setup("create the process", e))?;
        child.release().map_err(|e| SandboxError::setup("release the process", e))?;
        let io = child.take_io();
        let finished = supervise(
            child.pid,
            child.pidfd.as_ref(),
            io,
            &spec.stdin,
            l,
            started,
            Hooks { group: true, cancel: Some(cancel), ..Hooks::default() },
        )?;
        child.mark_reaped();

        if let Some(f) = finished.failure {
            let source = std::io::Error::from_raw_os_error(f.errno);
            return Err(if f.stage == stage::EXEC {
                SandboxError::Exec { program: spec.argv[0].clone(), source }
            } else {
                SandboxError::setup(f.stage_name(&plan), source)
            });
        }
        let status = classify(&finished, None, l);
        let (cpu_ms, _) = sys::usage(&finished.reaped.rusage);
        let isolation = IsolationReport {
            verified: true,
            no_new_privs: cfg!(target_os = "linux"),
            rlimits: true,
            host_root: sys::euid_egid().0 == 0,
            memory: if memory_limited { Enforcement::Rlimit } else { Enforcement::None },
            tasks: if task_limited { Enforcement::Rlimit } else { Enforcement::None },
            network: spec.network,
            notes,
            ..IsolationReport::default()
        };
        Ok(Outcome {
            status,
            stdout: finished.stdout,
            stderr: finished.stderr,
            stdout_truncated: finished.stdout_truncated,
            stderr_truncated: finished.stderr_truncated,
            wall_ms: finished.wall.as_millis() as u64,
            cpu_ms: Some(cpu_ms),
            // `ru_maxrss` of a child includes the size of the process it was forked from, so it says
            // nothing about the command; not reported.
            max_rss_bytes: None,
            environment_digest: environment_digest("process", &isolation, spec),
            isolation,
        })
    }
}

/// `RLIMIT_NPROC` counts the tasks of the whole *user*, not of the sandbox, so the limit is the number of
/// tasks the user has now plus the allowance. Best effort: tasks the user starts elsewhere meanwhile eat
/// into the allowance. Linux only (elsewhere there is no cheap way to count).
#[cfg(target_os = "linux")]
fn task_limit(allowance: u64) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    let (euid, _) = sys::euid_egid();
    let mut tasks = 0u64;
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let name = entry.file_name();
        if !name.to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit())) {
            continue;
        }
        if entry.metadata().ok().map(|m| m.uid()) != Some(euid) {
            continue;
        }
        let threads = std::fs::read_to_string(entry.path().join("status"))
            .ok()
            .and_then(|s| s.lines().find_map(|l| l.strip_prefix("Threads:").and_then(|v| v.trim().parse::<u64>().ok())))
            .unwrap_or(1);
        tasks += threads;
    }
    // +1: the command itself is also a new task.
    Some(tasks.saturating_add(allowance))
}

#[cfg(not(target_os = "linux"))]
fn task_limit(_allowance: u64) -> Option<u64> {
    None
}
