//! The supervising side of a run: feed stdin, capture output under a hard cap, enforce the wall-clock
//! budget, sample memory, kill and reap. Shared by all three sandboxes.

use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::{Duration, Instant};

use libc::pid_t;

use crate::error::SandboxError;
use crate::outcome::ExitStatus;
use crate::spawn::{Failure, Io};
use crate::spec::Limits;
use crate::sys::{self, PollEntry};

/// Why the supervisor, rather than the command, ended the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    TimedOut,
    OutputLimit,
    MemoryLimit,
    PidsLimit,
}

/// One measurement of everything inside a sandbox (its init excluded).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sample {
    /// Resident anonymous plus shared memory, in bytes, summed over all processes.
    pub memory: u64,
    /// Tasks: processes and threads.
    pub tasks: u64,
}

/// Optional behaviour of a supervised run.
#[derive(Default)]
pub(crate) struct Hooks<'a> {
    /// The process leads its own process group, which is killed along with it.
    pub group: bool,
    /// Extra action when the sandbox must be killed (e.g. `docker kill`).
    pub on_kill: Option<&'a mut dyn FnMut()>,
    /// Measures the sandbox; called only after the command has started. Memory is always checked against
    /// the limit; the number of tasks only if `task_limit` is set.
    pub sampler: Option<&'a mut dyn FnMut() -> Option<Sample>>,
    /// Kill the sandbox when a sample shows more tasks than this (used where the kernel does not enforce it).
    pub task_limit: Option<u64>,
    /// Asked on every wake-up; once it says `true` the sandbox is killed and the run ends with
    /// [`SandboxError::Cancelled`] (the lease was lost, the operator wants the worker to stop now).
    pub cancel: Option<&'a (dyn Fn() -> bool + Sync)>,
}

/// How often a cancellation request is noticed while the command is quiet.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// What happened, before it is mapped to an [`ExitStatus`] by the caller.
pub(crate) struct Finished {
    pub reaped: sys::Reaped,
    pub verdict: Option<Verdict>,
    /// A setup or exec failure reported by the child.
    pub failure: Option<Failure>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// Highest total memory seen by the sampler.
    pub peak_sampled: Option<u64>,
    pub wall: Duration,
}

/// One captured output stream.
struct Stream {
    fd: Option<OwnedFd>,
    buf: Vec<u8>,
    cap: usize,
    truncated: bool,
}

impl Stream {
    fn new(fd: OwnedFd, cap: u64) -> Self {
        Stream { fd: Some(fd), buf: Vec::new(), cap: usize::try_from(cap).unwrap_or(usize::MAX), truncated: false }
    }

    /// Read everything currently available. Bytes beyond the cap are discarded and flagged.
    fn pump(&mut self) {
        let Some(fd) = &self.fd else { return };
        let mut tmp = [0u8; 16 * 1024];
        loop {
            match sys::read(fd, &mut tmp) {
                Ok(0) => {
                    self.fd = None;
                    return;
                }
                Ok(n) => {
                    let room = self.cap.saturating_sub(self.buf.len());
                    let take = n.min(room);
                    self.buf.extend_from_slice(&tmp[..take]);
                    if take < n {
                        self.truncated = true;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(_) => {
                    self.fd = None;
                    return;
                }
            }
        }
    }
}

/// Run the supervision loop for `pid` until it ends or a limit forces it to.
///
/// On return the process has been reaped (its pid is free again) and, if it led a sandbox, the sandbox is
/// empty. On `Err` the process may still exist; the caller's guard (`spawn::Child`'s `Drop`) deals with it.
pub(crate) fn supervise(
    pid: pid_t,
    pidfd: Option<&OwnedFd>,
    io: Io,
    stdin: &[u8],
    limits: &Limits,
    started: Instant,
    mut hooks: Hooks<'_>,
) -> Result<Finished, SandboxError> {
    let _sigpipe = sys::SigpipeGuard::new();
    let Io { stdin: mut stdin_fd, stdout, stderr, status: mut status_fd } = io;
    let mut out = Stream::new(stdout, limits.output_bytes);
    let mut err = Stream::new(stderr, limits.output_bytes);
    let mut stdin_off = 0usize;
    if stdin.is_empty() {
        stdin_fd = None;
    }
    let deadline = started + Duration::from_millis(limits.wall_ms);
    let memory_limit = limits.memory_bytes;

    let mut verdict = None;
    let mut cancelled = false;
    let mut failure: Option<Failure> = None;
    let mut exec_done = false;
    let mut peak: Option<u64> = None;
    let mut next_sample = Instant::now();
    let mut sample_interval = Duration::from_millis(10);

    let kill_all = |hooks: &mut Hooks<'_>| {
        sys::kill(pid, libc::SIGKILL);
        if hooks.group {
            sys::killpg(pid, libc::SIGKILL);
        }
        if let Some(f) = hooks.on_kill.as_mut() {
            f();
        }
    };

    loop {
        out.pump();
        err.pump();
        pump_stdin(&mut stdin_fd, stdin, &mut stdin_off);
        read_status(&mut status_fd, &mut failure, &mut exec_done);

        if sys::has_exited(pid).map_err(|e| SandboxError::io("checking whether the sandbox exited", e))? {
            break;
        }
        if failure.is_some() {
            // The child reported that setup failed and is on its way out; make sure.
            kill_all(&mut hooks);
            break;
        }
        let now = Instant::now();
        if hooks.cancel.is_some_and(|cancel| cancel()) {
            cancelled = true;
            kill_all(&mut hooks);
            break;
        }
        if out.truncated || err.truncated {
            verdict = Some(Verdict::OutputLimit);
            kill_all(&mut hooks);
            break;
        }
        if now >= deadline {
            verdict = Some(Verdict::TimedOut);
            kill_all(&mut hooks);
            break;
        }
        if exec_done
            && now >= next_sample
            && let Some(sampler) = hooks.sampler.as_mut()
        {
            if let Some(sample) = sampler() {
                peak = Some(peak.map_or(sample.memory, |p| p.max(sample.memory)));
                // Tasks first: a runaway process tree also exceeds the memory limit, and its cause is the tree.
                if hooks.task_limit.is_some_and(|max| sample.tasks > max) {
                    verdict = Some(Verdict::PidsLimit);
                    kill_all(&mut hooks);
                    break;
                }
                if sample.memory > memory_limit {
                    verdict = Some(Verdict::MemoryLimit);
                    kill_all(&mut hooks);
                    break;
                }
                // Look closer as a limit approaches: a runaway allocator adds many MiB per millisecond and a
                // fork bomb doubles its processes every few hundred microseconds.
                let near_memory = sample.memory >= memory_limit / 2;
                let near_tasks = hooks.task_limit.is_some_and(|max| sample.tasks >= max / 4 && sample.tasks > 8);
                sample_interval =
                    if near_memory || near_tasks { Duration::from_millis(1) } else { Duration::from_millis(10) };
            }
            next_sample = now + sample_interval;
        }

        // Sleep until something happens: data, exit, the deadline or the next memory sample.
        let mut wake = deadline.saturating_duration_since(now).min(Duration::from_secs(1));
        if exec_done && hooks.sampler.is_some() {
            wake = wake.min(next_sample.saturating_duration_since(now));
        }
        if pidfd.is_none() {
            wake = wake.min(Duration::from_millis(10));
        }
        if hooks.cancel.is_some() {
            wake = wake.min(CANCEL_POLL);
        }
        let mut entries: Vec<PollEntry> = Vec::with_capacity(5);
        let mut roles: Vec<Role> = Vec::with_capacity(5);
        if let Some(fd) = pidfd {
            entries.push(PollEntry::new(fd.as_raw_fd(), false));
            roles.push(Role::Pid);
        }
        for (role, s) in [(Role::Out, &out), (Role::Err, &err)] {
            if let Some(fd) = &s.fd {
                entries.push(PollEntry::new(fd.as_raw_fd(), false));
                roles.push(role);
            }
        }
        if let Some(fd) = &status_fd {
            entries.push(PollEntry::new(fd.as_raw_fd(), false));
            roles.push(Role::Status);
        }
        if let Some(fd) = &stdin_fd {
            entries.push(PollEntry::new(fd.as_raw_fd(), true));
            roles.push(Role::In);
        }
        if entries.is_empty() {
            std::thread::sleep(wake.min(Duration::from_millis(5)));
        } else {
            sys::poll(&mut entries, wake).map_err(|e| SandboxError::io("polling the sandbox", e))?;
            // A hang-up on the stdin pipe means the command closed it; stop feeding.
            for (e, r) in entries.iter().zip(&roles) {
                if matches!(r, Role::In) && e.hup {
                    stdin_fd = None;
                }
            }
        }
    }

    // Whatever is left of the process tree dies with the leader; the group kill is done while the leader's
    // pid is still reserved (it is at least a zombie until reaped below).
    if hooks.group {
        sys::killpg(pid, libc::SIGKILL);
    }
    let reaped = wait_dead(pid, pidfd)?;

    // The command is gone; collect the rest of its output and a late failure record.
    read_status(&mut status_fd, &mut failure, &mut exec_done);
    drain(&mut out, &mut err);
    if cancelled {
        return Err(SandboxError::Cancelled);
    }
    if verdict.is_none() && (out.truncated || err.truncated) {
        verdict = Some(Verdict::OutputLimit);
    }
    Ok(Finished {
        reaped,
        verdict,
        failure,
        stdout: out.buf,
        stderr: err.buf,
        stdout_truncated: out.truncated,
        stderr_truncated: err.truncated,
        peak_sampled: peak,
        wall: started.elapsed(),
    })
}

enum Role {
    Pid,
    Out,
    Err,
    Status,
    In,
}

fn pump_stdin(fd: &mut Option<OwnedFd>, data: &[u8], off: &mut usize) {
    let Some(f) = fd else { return };
    while *off < data.len() {
        match sys::write(f, &data[*off..]) {
            Ok(n) => *off += n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
            Err(_) => {
                // EPIPE: the command is not reading any more.
                *fd = None;
                return;
            }
        }
    }
    // All delivered: closing the pipe is the command's end-of-file.
    *fd = None;
}

fn read_status(fd: &mut Option<OwnedFd>, failure: &mut Option<Failure>, exec_done: &mut bool) {
    let Some(f) = fd else { return };
    let mut buf = [0u8; 12];
    match sys::read(f, &mut buf) {
        Ok(0) => {
            *exec_done = true;
            *fd = None;
        }
        Ok(n) => {
            *failure = Failure::decode(&buf[..n]);
            *fd = None;
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
        Err(_) => *fd = None,
    }
}

/// Wait (bounded) for `pid` to become reapable, then reap it.
fn wait_dead(pid: pid_t, pidfd: Option<&OwnedFd>) -> Result<sys::Reaped, SandboxError> {
    let give_up = Instant::now() + Duration::from_secs(10);
    loop {
        match sys::wait4(pid, true) {
            Ok(Some(r)) => return Ok(r),
            Ok(None) => {}
            Err(e) => return Err(SandboxError::io("waiting for the sandbox to exit", e)),
        }
        let left = give_up.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(SandboxError::io(
                "waiting for the sandbox to exit",
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the killed sandbox did not terminate within 10 s (a process stuck in uninterruptible sleep?)",
                ),
            ));
        }
        match pidfd {
            Some(fd) => {
                let mut e = [PollEntry::new(fd.as_raw_fd(), false)];
                let _ = sys::poll(&mut e, left.min(Duration::from_millis(100)));
            }
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
}

/// After the command is gone the pipes normally hit end-of-file at once. Processes that escaped the sandbox
/// (only possible without a PID namespace) may keep a pipe open; do not wait for them for long.
fn drain(out: &mut Stream, err: &mut Stream) {
    let give_up = Instant::now() + Duration::from_millis(300);
    loop {
        out.pump();
        err.pump();
        if (out.fd.is_none() && err.fd.is_none()) || Instant::now() >= give_up {
            return;
        }
        let mut entries: Vec<PollEntry> =
            [&out.fd, &err.fd].into_iter().flatten().map(|fd| PollEntry::new(fd.as_raw_fd(), false)).collect();
        let _ = sys::poll(&mut entries, Duration::from_millis(20));
    }
}

/// Map how the process ended, plus what the supervisor did, to the public [`ExitStatus`].
///
/// `relayed` is the namespace init's record of the command's own status (`None` for a plain process, whose
/// wait status is the command's).
pub(crate) fn classify(f: &Finished, relayed: Option<(u32, i32)>, limits: &Limits) -> ExitStatus {
    let natural = match relayed {
        Some((1, code)) => Some(ExitStatus::Exited(code)),
        Some((2, sig)) => Some(ExitStatus::Signaled(sig)),
        _ => None,
    };
    let natural = natural.unwrap_or_else(|| wait_status(f.reaped.status));
    match f.verdict {
        Some(Verdict::TimedOut) => return ExitStatus::TimedOut,
        Some(Verdict::OutputLimit) => return ExitStatus::OutputLimit,
        Some(Verdict::MemoryLimit) => return ExitStatus::MemoryLimit,
        Some(Verdict::PidsLimit) => return ExitStatus::PidsLimit,
        None => {}
    }
    let (cpu_ms, _) = sys::usage(&f.reaped.rusage);
    let abnormal = !natural.success();
    // The command ran into its CPU budget: the kernel sends SIGXCPU at the soft limit and SIGKILL at the hard one.
    match natural {
        ExitStatus::Signaled(s) if s == libc::SIGXCPU => return ExitStatus::CpuLimit,
        ExitStatus::Signaled(s) if s == libc::SIGKILL && cpu_ms + 50 >= limits.cpu_s.saturating_mul(1000) => {
            return ExitStatus::CpuLimit;
        }
        _ => {}
    }
    // A command that died while the sampler saw it near the memory limit most likely died of it (an allocation
    // failed under RLIMIT_DATA, or the OOM killer struck). Best effort: see the docs.
    if abnormal && f.peak_sampled.is_some_and(|p| p >= limits.memory_bytes / 10 * 9) {
        return ExitStatus::MemoryLimit;
    }
    natural
}

/// The command's own status from a raw `wait` status.
pub(crate) fn wait_status(status: libc::c_int) -> ExitStatus {
    if libc::WIFEXITED(status) {
        ExitStatus::Exited(libc::WEXITSTATUS(status))
    } else if libc::WIFSIGNALED(status) {
        ExitStatus::Signaled(libc::WTERMSIG(status))
    } else {
        ExitStatus::Exited(-1)
    }
}
