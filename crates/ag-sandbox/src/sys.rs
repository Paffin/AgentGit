//! Thin, safe wrappers over the few system calls the supervising (parent) side needs.
//!
//! Everything here runs in the caller's process, in normal (allocating) Rust. The code that runs in the
//! forked child lives in `spawn.rs` and follows much stricter rules.
#![allow(unsafe_code)]
// Parts of this module serve only the Linux namespace sandbox.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::Duration;

use libc::{c_int, pid_t};

/// Map a `-1` return to the current `errno`.
pub(crate) fn cvt(ret: c_int) -> io::Result<c_int> {
    if ret == -1 { Err(io::Error::last_os_error()) } else { Ok(ret) }
}

/// A close-on-exec pipe `(read, write)` whose descriptors are both `>= 3`, so that a later `dup2` onto 0, 1
/// or 2 can never alias them, and so that a concurrent `fork`+`exec` in another thread cannot leak them.
pub(crate) fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as c_int; 2];
    // SAFETY: `fds` is a valid array of two ints, as pipe2 requires.
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    cvt(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) })?;
    // No pipe2 (macOS): pipe, then close-on-exec. A fork+exec in another thread between the two calls could
    // leak the descriptors into its child; that is a narrow window the platform leaves us with.
    // SAFETY: `fds` is a valid array of two ints, as pipe requires; fcntl on descriptors just created.
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    )))]
    unsafe {
        cvt(libc::pipe(fds.as_mut_ptr()))?;
        for fd in fds {
            cvt(libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC))?;
        }
    }
    // SAFETY: pipe2 just returned these two descriptors and nothing else owns them.
    let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    Ok((lift(r)?, lift(w)?))
}

/// Move `fd` above the standard descriptors (a no-op if it already is).
fn lift(fd: OwnedFd) -> io::Result<OwnedFd> {
    if fd.as_raw_fd() >= 3 {
        return Ok(fd);
    }
    // SAFETY: F_DUPFD_CLOEXEC on a descriptor we own; the result is a new descriptor we then own.
    let n = cvt(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) })?;
    // SAFETY: `n` is a fresh descriptor returned by fcntl.
    Ok(unsafe { OwnedFd::from_raw_fd(n) })
}

pub(crate) fn set_nonblocking(fd: &OwnedFd) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor we own, with integer arguments only.
    unsafe {
        let flags = cvt(libc::fcntl(fd.as_raw_fd(), libc::F_GETFL))?;
        cvt(libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK))?;
    }
    Ok(())
}

/// `read(2)`; `Ok(0)` is end-of-file, `ErrorKind::WouldBlock` means no data yet.
pub(crate) fn read(fd: &OwnedFd, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        // SAFETY: `buf` is valid for `buf.len()` bytes of writes.
        let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n >= 0 {
            return Ok(n as usize);
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// `write(2)`.
pub(crate) fn write(fd: &OwnedFd, buf: &[u8]) -> io::Result<usize> {
    loop {
        // SAFETY: `buf` is valid for `buf.len()` bytes of reads.
        let n = unsafe { libc::write(fd.as_raw_fd(), buf.as_ptr().cast(), buf.len()) };
        if n >= 0 {
            return Ok(n as usize);
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// Which events to wait for on one descriptor, and which fired.
pub(crate) struct PollEntry {
    pub fd: RawFd,
    pub want_write: bool,
    pub readable: bool,
    pub writable: bool,
    /// Hang-up or error (the other end closed).
    pub hup: bool,
}

impl PollEntry {
    pub(crate) fn new(fd: RawFd, want_write: bool) -> Self {
        PollEntry { fd, want_write, readable: false, writable: false, hup: false }
    }
}

/// `poll(2)` with a timeout; fills in the result flags of `entries`.
pub(crate) fn poll(entries: &mut [PollEntry], timeout: Duration) -> io::Result<()> {
    let mut fds: Vec<libc::pollfd> = entries
        .iter()
        .map(|e| libc::pollfd { fd: e.fd, events: if e.want_write { libc::POLLOUT } else { libc::POLLIN }, revents: 0 })
        .collect();
    // Round up so that a sub-millisecond remainder does not turn into a busy loop.
    let ms = timeout.as_nanos().div_ceil(1_000_000).min(i32::MAX as u128) as c_int;
    // SAFETY: `fds` is a valid array of `fds.len()` pollfd structs.
    let r = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
    if r < 0 {
        let e = io::Error::last_os_error();
        // An interrupted poll just returns early with nothing ready: the caller's loop re-checks its
        // deadlines right after.
        return if e.kind() == io::ErrorKind::Interrupted { Ok(()) } else { Err(e) };
    }
    for (e, p) in entries.iter_mut().zip(&fds) {
        e.readable = p.revents & (libc::POLLIN | libc::POLLHUP) != 0;
        e.writable = p.revents & libc::POLLOUT != 0;
        e.hup = p.revents & (libc::POLLHUP | libc::POLLERR) != 0;
    }
    Ok(())
}

/// Has `pid` (our child) terminated? Leaves the zombie in place, so the pid stays reserved and a following
/// `kill` of its process group cannot hit an unrelated process.
pub(crate) fn has_exited(pid: pid_t) -> io::Result<bool> {
    // SAFETY: an all-zero siginfo_t is a valid value to pass for waitid to fill.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `info` is a valid, writable siginfo_t.
        let r = unsafe {
            libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT)
        };
        if r == 0 {
            break;
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
    // With WNOHANG, `si_pid` stays zero when nothing has changed state.
    #[cfg(target_os = "linux")]
    // SAFETY: after a successful waitid the si_pid field is initialised (zero or the child's pid).
    let child = unsafe { info.si_pid() };
    #[cfg(not(target_os = "linux"))]
    let child = info.si_pid;
    Ok(child != 0)
}

/// Result of reaping a child.
pub(crate) struct Reaped {
    pub status: c_int,
    pub rusage: libc::rusage,
}

/// Reap `pid` with `wait4`. `None` means it has not exited yet (only with `nohang`).
pub(crate) fn wait4(pid: pid_t, nohang: bool) -> io::Result<Option<Reaped>> {
    // SAFETY: an all-zero rusage is a valid value to pass for wait4 to fill.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    let mut status: c_int = 0;
    loop {
        // SAFETY: `status` and `ru` are valid, writable and live for the call.
        let r = unsafe { libc::wait4(pid, &mut status, if nohang { libc::WNOHANG } else { 0 }, &mut ru) };
        if r == pid {
            return Ok(Some(Reaped { status, rusage: ru }));
        }
        if r == 0 {
            return Ok(None);
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

pub(crate) fn kill(pid: pid_t, sig: c_int) {
    // SAFETY: plain integer arguments. Errors (ESRCH: already gone) are irrelevant to the caller.
    unsafe {
        libc::kill(pid, sig);
    }
}

/// Signal a whole process group; only call while the group leader is an unreaped zombie or alive.
pub(crate) fn killpg(pgid: pid_t, sig: c_int) {
    // SAFETY: plain integer arguments; errors (ESRCH) are irrelevant.
    unsafe {
        libc::kill(-pgid, sig);
    }
}

/// A pollable descriptor that becomes readable when `pid` terminates (Linux 5.3+). `None` where
/// unsupported; callers then poll on a short tick instead.
#[cfg(target_os = "linux")]
pub(crate) fn pidfd_open(pid: pid_t) -> Option<OwnedFd> {
    // SAFETY: pidfd_open takes two integers and returns a new descriptor or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::c_long, 0 as libc::c_long) };
    if fd < 0 {
        return None;
    }
    // SAFETY: the kernel returned a fresh descriptor that nothing else owns.
    lift(unsafe { OwnedFd::from_raw_fd(fd as c_int) }).ok()
}

/// See the Linux variant.
#[cfg(not(target_os = "linux"))]
pub(crate) fn pidfd_open(_pid: pid_t) -> Option<OwnedFd> {
    None
}

/// Blocks `SIGPIPE` for the current thread while alive, then discards a `SIGPIPE` that our own writes to a
/// closed pipe left pending and restores the previous mask. Needed because a host that has not ignored
/// `SIGPIPE` would otherwise be killed by a command that closes its stdin early.
pub(crate) struct SigpipeGuard {
    old: libc::sigset_t,
    was_pending: bool,
}

impl SigpipeGuard {
    pub(crate) fn new() -> Self {
        // SAFETY: sigset operations on locally owned, properly initialised sets; pthread_sigmask only
        // changes the calling thread's mask.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            let mut old: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGPIPE);
            let mut pending: libc::sigset_t = std::mem::zeroed();
            libc::sigpending(&mut pending);
            let was_pending = libc::sigismember(&pending, libc::SIGPIPE) == 1;
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut old);
            SigpipeGuard { old, was_pending }
        }
    }
}

impl Drop for SigpipeGuard {
    fn drop(&mut self) {
        // SAFETY: as in `new`; SIGPIPE is blocked on this thread, so if it is pending `sigwait` returns at
        // once and consumes it (it is only called after `sigpending` said so, so it can never block).
        unsafe {
            if !self.was_pending {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGPIPE);
                let mut pending: libc::sigset_t = std::mem::zeroed();
                libc::sigpending(&mut pending);
                if libc::sigismember(&pending, libc::SIGPIPE) == 1 {
                    let mut sig: c_int = 0;
                    libc::sigwait(&set, &mut sig);
                }
            }
            libc::pthread_sigmask(libc::SIG_SETMASK, &self.old, std::ptr::null_mut());
        }
    }
}

/// `rusage` → (cpu milliseconds, max RSS bytes). `ru_maxrss` is kilobytes on Linux, bytes on macOS.
pub(crate) fn usage(ru: &libc::rusage) -> (u64, u64) {
    let tv = |t: libc::timeval| (t.tv_sec.max(0) as u64) * 1000 + (t.tv_usec.max(0) as u64) / 1000;
    let cpu_ms = tv(ru.ru_utime) + tv(ru.ru_stime);
    let rss = ru.ru_maxrss.max(0) as u64;
    #[cfg(target_os = "macos")]
    let rss_bytes = rss;
    #[cfg(not(target_os = "macos"))]
    let rss_bytes = rss * 1024;
    (cpu_ms, rss_bytes)
}

/// The effective user and group ids of this process.
pub(crate) fn euid_egid() -> (u32, u32) {
    // SAFETY: geteuid/getegid take no arguments and cannot fail.
    unsafe { (libc::geteuid(), libc::getegid()) }
}

/// The `f_type` of the file system holding `path`.
pub(crate) fn filesystem_type(path: &std::path::Path) -> Option<i64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statfs with a valid C string and a zeroed out-parameter of the right type.
    unsafe {
        let mut st: libc::statfs = std::mem::zeroed();
        (libc::statfs(c.as_ptr(), &mut st) == 0).then_some(st.f_type as i64)
    }
}

/// Whether the calling process may write to `path` (`access(2)` with `W_OK`).
pub(crate) fn is_writable(path: &std::path::Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: access(2) with a valid C string.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// An all-zero `rusage` (tests build `Reaped` values by hand).
#[cfg(test)]
pub(crate) fn zero_rusage() -> libc::rusage {
    // SAFETY: rusage is plain integers and timevals; all-zero is a valid value.
    unsafe { std::mem::zeroed() }
}
