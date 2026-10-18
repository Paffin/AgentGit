//! Creating the sandboxed process: the only place where code runs between `clone` and `exec`.
//!
//! # Why a bare `clone` and no helper binary
//!
//! The caller of [`crate::Sandbox::run`] is an ordinary, possibly multi-threaded program (an async server,
//! a worker pool). After `fork` in such a process only async-signal-safe operations are allowed in the
//! child: another thread may have held the allocator's lock, a logger's mutex or the environment lock at
//! the instant of the fork, and the child owns a copy of that lock locked forever.
//!
//! Two designs are sound. One re-executes `/proc/self/exe` (or a helper binary) and does the setup in a
//! fresh, single-threaded image. That needs the host program to cooperate (its `main` must recognise the
//! trampoline) or a second binary to be located and shipped. The other keeps the setup in the child but
//! restricts it to raw system calls over data prepared *before* the fork. We chose the second: the library
//! stays self-contained, and the discipline is mechanical and testable:
//!
//! * Everything the child needs — C strings, the list of mount operations, the seccomp program — is built
//!   by the parent into a [`Plan`] before `clone`. The child only reads it.
//! * The child calls nothing but the one-line wrappers in [`raw`], each a single system call (or a few
//!   arithmetic steps) with no allocation, no locking and no `std` I/O. Failures are reported as a fixed
//!   12-byte record on a pipe, never formatted.
//! * It never returns and never unwinds: it ends in `_exit` or `execve`. (A bug that panics would abort,
//!   which the parent reports as a dead sandbox.)
//! * The unit tests install a global allocator that terminates the process if anything allocates or frees
//!   after the clone (`tests::the_child_never_touches_the_allocator`).
//!
//! The process created is the root of the sandbox: with namespaces it is the PID-namespace init, which
//! stays alive as a tiny supervisor (reaping orphans, relaying the exit status of the real command);
//! without them it simply becomes the command.
#![allow(unsafe_code)]
// Parts of this module serve only the Linux namespace sandbox.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::{Duration, Instant};

use libc::{c_char, c_int, c_ulong, pid_t};

use crate::nsinit::NsPlan;
use crate::sys;

/// Exit code of the child when setup failed (the reason is on the status pipe).
pub(crate) const EXIT_SETUP: c_int = 120;

/// `uid`/`gid` the command has inside its user namespace.
pub(crate) const INNER_ID: u32 = 1000;

// --- plan ---------------------------------------------------------------------------------------------

/// A resource limit to apply in the command's process just before `exec`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Res {
    Cpu,
    Data,
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    As,
    Nproc,
    Nofile,
    Fsize,
    Core,
}

impl Res {
    fn name(self) -> &'static str {
        match self {
            Res::Cpu => "RLIMIT_CPU",
            Res::Data => "RLIMIT_DATA",
            Res::As => "RLIMIT_AS",
            Res::Nproc => "RLIMIT_NPROC",
            Res::Nofile => "RLIMIT_NOFILE",
            Res::Fsize => "RLIMIT_FSIZE",
            Res::Core => "RLIMIT_CORE",
        }
    }
}

/// NUL-terminated C string array for `execve`.
#[derive(Debug)]
pub(crate) struct CStrings {
    _own: Vec<CString>,
    ptrs: Vec<*const c_char>,
}

impl CStrings {
    pub(crate) fn new(items: Vec<CString>) -> Self {
        let mut ptrs: Vec<*const c_char> = items.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        CStrings { _own: items, ptrs }
    }

    fn as_ptr(&self) -> *const *const c_char {
        self.ptrs.as_ptr()
    }
}

/// One classic-BPF instruction. Only Linux has seccomp; elsewhere the program is always empty.
#[cfg(target_os = "linux")]
pub(crate) type SockFilter = libc::sock_filter;
/// The element type of the seccomp program on platforms without seccomp, where that program is always empty.
#[cfg(not(target_os = "linux"))]
pub(crate) type SockFilter = u64;

/// Everything the child does, decided by the parent before the `clone`.
#[derive(Debug)]
pub(crate) struct Plan {
    /// `CLONE_NEW*` flags; zero for a plain process.
    pub clone_flags: c_ulong,
    pub ns: Option<NsPlan>,
    pub argv: CStrings,
    pub envp: CStrings,
    /// Paths to try with `execve`, in order (the `PATH` search, resolved in advance).
    pub exec_paths: Vec<CString>,
    pub cwd: CString,
    /// `(resource, soft, hard)` applied to the command's process.
    pub rlimits: Vec<(Res, u64, u64)>,
    pub seccomp: Vec<SockFilter>,
    pub no_new_privs: bool,
    /// Stop right before `execve` and exit 0 (used to probe whether the sandbox can be set up at all).
    pub probe_only: bool,
}

// --- status records -------------------------------------------------------------------------------------

/// Which step of the child failed. Sent as part of a [`Failure`].
pub(crate) mod stage {
    pub const SETSID: u32 = 1;
    pub const STDIO: u32 = 2;
    pub const HOSTNAME: u32 = 3;
    pub const LOOPBACK: u32 = 4;
    pub const MAKE_PRIVATE: u32 = 5;
    pub const MOUNT_ROOT: u32 = 6;
    pub const MKDIR_OLD: u32 = 7;
    pub const PIVOT_ROOT: u32 = 8;
    pub const CHDIR_ROOT: u32 = 9;
    pub const OP: u32 = 10;
    pub const DETACH_OLD: u32 = 11;
    pub const RMDIR_OLD: u32 = 12;
    pub const REMOUNT_ROOT: u32 = 13;
    pub const CAPS: u32 = 14;
    pub const NO_NEW_PRIVS: u32 = 15;
    pub const SECCOMP: u32 = 16;
    pub const FORK: u32 = 17;
    pub const RLIMIT: u32 = 18;
    pub const CHDIR: u32 = 19;
    pub const EXEC: u32 = 20;
    pub const SET_IDS: u32 = 21;
}

/// A setup failure reported by the child.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Failure {
    pub stage: u32,
    pub detail: u32,
    pub errno: i32,
}

impl Failure {
    /// Decode the 12-byte record; `None` for a short or empty read.
    pub(crate) fn decode(buf: &[u8]) -> Option<Failure> {
        if buf.len() < 12 {
            return None;
        }
        let u = |i: usize| u32::from_ne_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
        Some(Failure { stage: u(0), detail: u(4), errno: u(8) as i32 })
    }

    /// Human-readable step name, using the plan to name mount operations and limits.
    pub(crate) fn stage_name(&self, plan: &Plan) -> String {
        use stage::*;
        match self.stage {
            SETSID => "setsid".into(),
            STDIO => "connect stdio".into(),
            HOSTNAME => "sethostname".into(),
            LOOPBACK => "bring up loopback".into(),
            MAKE_PRIVATE => "make mounts private".into(),
            MOUNT_ROOT => "mount new root".into(),
            MKDIR_OLD => "create old-root directory".into(),
            PIVOT_ROOT => "pivot_root".into(),
            CHDIR_ROOT => "chdir /".into(),
            OP => crate::nsinit::describe_op(plan, self.detail),
            DETACH_OLD => "detach old root".into(),
            RMDIR_OLD => "remove old-root directory".into(),
            REMOUNT_ROOT => "make root read-only".into(),
            CAPS => "drop capabilities".into(),
            NO_NEW_PRIVS => "set no_new_privs".into(),
            SECCOMP => "install seccomp filter".into(),
            FORK => "fork command".into(),
            RLIMIT => plan
                .rlimits
                .get(self.detail as usize)
                .map(|(r, ..)| format!("setrlimit {}", r.name()))
                .unwrap_or_else(|| "setrlimit".into()),
            CHDIR => "chdir to working directory".into(),
            EXEC => "execve".into(),
            SET_IDS => "switch to the mapped user and group".into(),
            other => format!("setup step {other}"),
        }
    }
}

// --- the child --------------------------------------------------------------------------------------------

/// Descriptors the child uses (all `>= 3`, close-on-exec in the parent).
#[derive(Clone, Copy)]
pub(crate) struct ChildFds {
    pub go_r: c_int,
    pub status_w: c_int,
    pub result_w: c_int,
    pub in_r: c_int,
    pub out_w: c_int,
    pub err_w: c_int,
}

#[cfg(test)]
pub(crate) static IN_CHILD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Async-signal-safe building blocks. Each function is one system call (or plain arithmetic); none allocates,
/// locks or formats.
pub(crate) mod raw {
    use super::*;

    pub fn errno() -> c_int {
        // `last_os_error` builds an `Os(code)` value; it does not allocate.
        io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }

    pub fn exit(code: c_int) -> ! {
        // SAFETY: _exit is async-signal-safe and never returns.
        unsafe { libc::_exit(code) }
    }

    pub fn write(fd: c_int, buf: &[u8]) -> isize {
        // SAFETY: `buf` is valid for reads of its length.
        unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) }
    }

    pub fn read(fd: c_int, buf: &mut [u8]) -> isize {
        loop {
            // SAFETY: `buf` is valid for writes of its length.
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n >= 0 || errno() != libc::EINTR {
                return n;
            }
        }
    }

    pub fn close(fd: c_int) {
        // SAFETY: closing an integer descriptor; EBADF is harmless here.
        unsafe {
            libc::close(fd);
        }
    }

    pub fn dup2(from: c_int, to: c_int) -> c_int {
        // SAFETY: integer arguments only.
        unsafe { libc::dup2(from, to) }
    }

    /// Close descriptors `lo..=hi`, via `close_range(2)` or, on old kernels, one by one.
    pub fn close_range(lo: c_int, hi: c_int) {
        if lo > hi {
            return;
        }
        #[cfg(target_os = "linux")]
        {
            // SAFETY: close_range takes three integers.
            let r = unsafe {
                libc::syscall(libc::SYS_close_range, lo as libc::c_uint, hi as libc::c_uint, 0 as libc::c_uint)
            };
            if r == 0 {
                return;
            }
        }
        // Fallback: bounded by the descriptor limit (and a sane maximum, in case that is "unlimited").
        let mut max: c_int = 65_536;
        let mut rl = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: `rl` is a valid rlimit struct.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut rl) } == 0 && rl.rlim_cur < max as libc::rlim_t {
            max = rl.rlim_cur as c_int;
        }
        let mut fd = lo;
        while fd <= hi.min(max) {
            close(fd);
            fd += 1;
        }
    }

    pub fn chdir(path: &CStr) -> c_int {
        // SAFETY: valid C string.
        unsafe { libc::chdir(path.as_ptr()) }
    }

    pub fn setsid() -> c_int {
        // SAFETY: no arguments.
        unsafe { libc::setsid() }
    }

    #[cfg(target_os = "linux")]
    pub fn prctl(option: c_int, a: c_ulong, b: c_ulong) -> c_int {
        // SAFETY: integer arguments only; callers pass options whose arguments are plain integers.
        unsafe { libc::prctl(option, a, b, 0 as c_ulong, 0 as c_ulong) }
    }

    pub fn umask(mask: libc::mode_t) {
        // SAFETY: no pointers.
        unsafe {
            libc::umask(mask);
        }
    }

    /// `fork` as a raw `clone(SIGCHLD)` on Linux: no libc atfork handlers, no allocator interaction.
    pub fn fork() -> c_int {
        raw_clone(0)
    }

    pub fn waitpid_any(status: &mut c_int) -> pid_t {
        loop {
            // SAFETY: `status` is a valid out-pointer.
            let r = unsafe { libc::waitpid(-1, status, 0) };
            if r >= 0 || errno() != libc::EINTR {
                return r;
            }
        }
    }

    pub fn setrlimit(res: Res, soft: u64, hard: u64) -> c_int {
        let lim = libc::rlimit { rlim_cur: soft as libc::rlim_t, rlim_max: hard as libc::rlim_t };
        // SAFETY: `lim` is a valid rlimit struct.
        unsafe {
            match res {
                Res::Cpu => libc::setrlimit(libc::RLIMIT_CPU, &lim),
                Res::Data => libc::setrlimit(libc::RLIMIT_DATA, &lim),
                Res::As => libc::setrlimit(libc::RLIMIT_AS, &lim),
                Res::Nproc => libc::setrlimit(libc::RLIMIT_NPROC, &lim),
                Res::Nofile => libc::setrlimit(libc::RLIMIT_NOFILE, &lim),
                Res::Fsize => libc::setrlimit(libc::RLIMIT_FSIZE, &lim),
                Res::Core => libc::setrlimit(libc::RLIMIT_CORE, &lim),
            }
        }
    }

    pub fn execve(path: &CStr, argv: *const *const c_char, envp: *const *const c_char) -> c_int {
        // SAFETY: `path` is a valid C string; `argv` and `envp` come from `CStrings`, which are
        // NUL-terminated arrays of valid C strings that outlive the call.
        unsafe { libc::execve(path.as_ptr(), argv, envp) }
    }

    /// Block every signal for this thread; the previous mask is not needed (the child never returns).
    pub fn block_all_signals() {
        // SAFETY: sigfillset/sigprocmask on a locally owned set.
        unsafe {
            let mut all: libc::sigset_t = std::mem::zeroed();
            libc::sigfillset(&mut all);
            libc::sigprocmask(libc::SIG_SETMASK, &all, std::ptr::null_mut());
        }
    }

    pub fn unblock_all_signals() {
        // SAFETY: sigemptyset/sigprocmask on a locally owned set.
        unsafe {
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
        }
    }

    /// Reset every catchable signal to its default action: handlers installed by the host program must not
    /// run in the child, and ignored signals (Rust ignores `SIGPIPE`) must not stay ignored in the command.
    pub fn reset_signal_dispositions() {
        for sig in 1..=64 {
            if sig == libc::SIGKILL || sig == libc::SIGSTOP {
                continue;
            }
            // SAFETY: SIG_DFL is a valid disposition for every signal; invalid numbers just fail with EINVAL.
            unsafe {
                libc::signal(sig, libc::SIG_DFL);
            }
        }
    }
}

/// `clone` with `flags | SIGCHLD`, returning in both parent and child like `fork`.
#[cfg(target_os = "linux")]
fn raw_clone(flags: c_ulong) -> pid_t {
    // SAFETY: with a null stack and no CLONE_VM/CLONE_THREAD the kernel gives the child a copy-on-write copy
    // of this thread's address space and the same stack contents, exactly like fork. Register/argument order
    // differs between architectures only for the arguments that are zero here. The child restricts itself to
    // async-signal-safe operations (see the module documentation).
    unsafe {
        libc::syscall(
            libc::SYS_clone,
            flags | libc::SIGCHLD as c_ulong,
            0 as c_ulong,
            0 as c_ulong,
            0 as c_ulong,
            0 as c_ulong,
        ) as pid_t
    }
}

/// Other Unixes have no namespaces; a plain `fork` is all that is needed.
#[cfg(not(target_os = "linux"))]
fn raw_clone(_flags: c_ulong) -> pid_t {
    // SAFETY: fork; the child restricts itself to async-signal-safe operations.
    unsafe { libc::fork() }
}

/// Report a failure to the parent and exit.
pub(crate) fn fail(status_w: c_int, stage: u32, detail: u32) -> ! {
    let e = raw::errno();
    let mut rec = [0u8; 12];
    rec[0..4].copy_from_slice(&stage.to_ne_bytes());
    rec[4..8].copy_from_slice(&detail.to_ne_bytes());
    rec[8..12].copy_from_slice(&e.to_ne_bytes());
    // A single write of 12 bytes to a pipe is atomic; if it fails there is nobody left to tell.
    raw::write(status_w, &rec);
    raw::exit(EXIT_SETUP)
}

macro_rules! check {
    ($fds:expr, $stage:expr, $detail:expr, $call:expr) => {
        if $call < 0 {
            $crate::spawn::fail($fds.status_w, $stage, $detail)
        }
    };
}
#[cfg_attr(not(target_os = "linux"), allow(unused_imports))]
pub(crate) use check;

/// Entry point of the cloned child. Never returns.
fn child_main(plan: &Plan, fds: ChildFds) -> ! {
    #[cfg(test)]
    IN_CHILD.store(true, std::sync::atomic::Ordering::Relaxed);

    raw::block_all_signals();
    raw::reset_signal_dispositions();
    #[cfg(target_os = "linux")]
    raw::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as c_ulong, 0);

    // Wait until the parent has finished its part (id maps, cgroup membership). EOF or an error means the
    // parent gave up or died: leave quietly.
    let mut go = [0u8; 1];
    if raw::read(fds.go_r, &mut go) != 1 {
        raw::exit(EXIT_SETUP);
    }
    raw::close(fds.go_r);

    check!(fds, stage::SETSID, 0, raw::setsid());

    // The command's stdio is the three pipes; the host's own descriptors 0-2 are replaced.
    check!(fds, stage::STDIO, 0, raw::dup2(fds.in_r, 0));
    check!(fds, stage::STDIO, 1, raw::dup2(fds.out_w, 1));
    check!(fds, stage::STDIO, 2, raw::dup2(fds.err_w, 2));
    for fd in [fds.in_r, fds.out_w, fds.err_w] {
        raw::close(fd);
    }
    // Everything else the host process had open (database handles, sockets, key files) must not leak in.
    let (a, b) = if fds.status_w < fds.result_w { (fds.status_w, fds.result_w) } else { (fds.result_w, fds.status_w) };
    raw::close_range(3, a - 1);
    raw::close_range(a + 1, b - 1);
    raw::close_range(b + 1, c_int::MAX);

    match &plan.ns {
        Some(ns) => crate::nsinit::init_main(plan, ns, fds),
        None => worker_main(plan, fds, false),
    }
}

/// The command's process: apply limits, then `execve`. Never returns.
pub(crate) fn worker_main(plan: &Plan, fds: ChildFds, in_namespace: bool) -> ! {
    if !in_namespace {
        // A plain process still owns the result pipe's write end; the command must never see it.
        raw::close(fds.result_w);
        #[cfg(target_os = "linux")]
        if plan.no_new_privs {
            check!(fds, stage::NO_NEW_PRIVS, 0, raw::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0));
        }
    }
    raw::unblock_all_signals();
    for (i, (res, soft, hard)) in plan.rlimits.iter().enumerate() {
        check!(fds, stage::RLIMIT, i as u32, raw::setrlimit(*res, *soft, *hard));
    }
    raw::umask(0o022);
    check!(fds, stage::CHDIR, 0, raw::chdir(&plan.cwd));
    if plan.probe_only {
        raw::exit(0);
    }
    // execvp semantics over the candidates resolved in advance: keep looking after "not found" and
    // "permission denied", stop at any other error.
    let mut denied = false;
    for (i, path) in plan.exec_paths.iter().enumerate() {
        raw::execve(path, plan.argv.as_ptr(), plan.envp.as_ptr());
        match raw::errno() {
            libc::ENOENT | libc::ENOTDIR => {}
            libc::EACCES => denied = true,
            e => report_exec(fds.status_w, i as u32, e),
        }
    }
    report_exec(fds.status_w, 0, if denied { libc::EACCES } else { libc::ENOENT })
}

fn report_exec(status_w: c_int, detail: u32, errno: c_int) -> ! {
    let mut rec = [0u8; 12];
    rec[0..4].copy_from_slice(&stage::EXEC.to_ne_bytes());
    rec[4..8].copy_from_slice(&detail.to_ne_bytes());
    rec[8..12].copy_from_slice(&errno.to_ne_bytes());
    raw::write(status_w, &rec);
    raw::exit(127)
}

// --- the parent ---------------------------------------------------------------------------------------------

/// A created, still-blocked sandbox process and our ends of its pipes.
pub(crate) struct Child {
    pub pid: pid_t,
    pub pidfd: Option<OwnedFd>,
    go: Option<OwnedFd>,
    io: Option<Io>,
    /// The relayed exit status of the command (namespace sandbox only), read after the child is gone.
    pub result: OwnedFd,
    reaped: bool,
}

/// The pipes the supervisor works with.
pub(crate) struct Io {
    /// Write end of the command's stdin; `None` once closed.
    pub stdin: Option<OwnedFd>,
    pub stdout: OwnedFd,
    pub stderr: OwnedFd,
    /// Becomes readable with a [`Failure`] record, or hits EOF when the command has exec'd.
    pub status: Option<OwnedFd>,
}

impl Child {
    /// Let the child proceed (after the id maps and cgroup membership are in place).
    pub(crate) fn release(&mut self) -> io::Result<()> {
        let go = self.go.take().ok_or_else(|| io::Error::other("sandbox already released"))?;
        sys::write(&go, b"g").map(|_| ())
    }

    /// Hand the pipes to the supervisor.
    pub(crate) fn take_io(&mut self) -> Io {
        self.io.take().expect("pipes are taken once")
    }

    /// Mark the child as reaped by the supervisor, so that `Drop` does not signal a reused pid.
    pub(crate) fn mark_reaped(&mut self) {
        self.reaped = true;
    }

    /// The relayed `(kind, value)` status record of the namespace init, if it wrote one:
    /// kind 1 = exited with `value`, 2 = killed by signal `value`.
    pub(crate) fn relayed_status(&self) -> Option<(u32, i32)> {
        let mut buf = [0u8; 8];
        match sys::read(&self.result, &mut buf) {
            Ok(8) => Some((
                u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]),
                i32::from_ne_bytes([buf[4], buf[5], buf[6], buf[7]]),
            )),
            _ => None,
        }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        // An error path: make sure nothing survives and no zombie remains. The child called `setsid`, so
        // its process group is its own.
        sys::kill(self.pid, libc::SIGKILL);
        sys::killpg(self.pid, libc::SIGKILL);
        let give_up = Instant::now() + Duration::from_secs(10);
        while Instant::now() < give_up {
            match sys::wait4(self.pid, true) {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    }
}

/// Create the sandbox process. It blocks until [`Child::release`].
pub(crate) fn spawn(plan: &Plan) -> io::Result<Child> {
    let (go_r, go_w) = sys::pipe()?;
    let (status_r, status_w) = sys::pipe()?;
    let (result_r, result_w) = sys::pipe()?;
    let (in_r, in_w) = sys::pipe()?;
    let (out_r, out_w) = sys::pipe()?;
    let (err_r, err_w) = sys::pipe()?;
    let fds = ChildFds {
        go_r: go_r.as_raw_fd(),
        status_w: status_w.as_raw_fd(),
        result_w: result_w.as_raw_fd(),
        in_r: in_r.as_raw_fd(),
        out_w: out_w.as_raw_fd(),
        err_w: err_w.as_raw_fd(),
    };

    let pid = raw_clone(plan.clone_flags);
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        child_main(plan, fds);
    }

    // Parent: close the ends that belong to the child (by dropping them) and keep ours.
    drop((go_r, status_w, result_w, in_r, out_w, err_w));
    // From here `Drop` kills and reaps the child on any error. The pipes are polled, so they must not block.
    let child = Child {
        pid,
        pidfd: sys::pidfd_open(pid),
        go: Some(go_w),
        io: Some(Io { stdin: Some(in_w), stdout: out_r, stderr: err_r, status: Some(status_r) }),
        result: result_r,
        reaped: false,
    };
    if let Some(io) = &child.io {
        for fd in [Some(&io.stdout), Some(&io.stderr), io.stdin.as_ref(), io.status.as_ref()].into_iter().flatten() {
            sys::set_nonblocking(fd)?;
        }
    }
    sys::set_nonblocking(&child.result)?;
    Ok(child)
}

#[cfg(test)]
mod tests {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::Ordering;

    use super::*;

    /// Terminates the process the instant the cloned child allocates or frees memory.
    struct GuardAlloc;

    fn guard() {
        if IN_CHILD.load(Ordering::Relaxed) {
            raw::write(2, b"FATAL: the sandbox child touched the allocator between clone and exec\n");
            raw::exit(97);
        }
    }

    // SAFETY: every method forwards to the system allocator unchanged after the guard check.
    unsafe impl GlobalAlloc for GuardAlloc {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            guard();
            // SAFETY: same contract as the caller's.
            unsafe { System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            guard();
            // SAFETY: same contract as the caller's.
            unsafe { System.dealloc(p, l) }
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            guard();
            // SAFETY: same contract as the caller's.
            unsafe { System.alloc_zeroed(l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            guard();
            // SAFETY: same contract as the caller's.
            unsafe { System.realloc(p, l, n) }
        }
    }

    #[global_allocator]
    static ALLOC: GuardAlloc = GuardAlloc;

    #[test]
    fn failure_records_roundtrip() {
        let mut rec = [0u8; 12];
        rec[0..4].copy_from_slice(&stage::OP.to_ne_bytes());
        rec[4..8].copy_from_slice(&7u32.to_ne_bytes());
        rec[8..12].copy_from_slice(&libc::EPERM.to_ne_bytes());
        let f = Failure::decode(&rec).unwrap();
        assert_eq!((f.stage, f.detail, f.errno), (stage::OP, 7, libc::EPERM));
        assert!(Failure::decode(&rec[..11]).is_none());
    }

    #[test]
    fn the_child_never_touches_the_allocator() {
        // Run a plain process and a full namespace sandbox's setup path; the allocator guard above kills the
        // child with status 97 on any allocation, which would show up here as a failed run.
        use crate::{NamespaceSandbox, ProcessSandbox, Sandbox, SandboxSpec};
        let dir = tempfile::tempdir().unwrap();
        // `true` is looked up on the default PATH: it lives in `/bin` on some hosts and only in `/usr/bin` on
        // others (macOS has no `/bin/true`), and the lookup itself must also stay allocation-free in the child.
        let spec = SandboxSpec::new(["true"], dir.path()).ro_mounts(crate::standard_system_mounts());
        let out = ProcessSandbox::new().run(&spec).expect("process sandbox runs");
        assert!(out.success(), "{:?} {}", out.status, out.stderr_lossy());
        let ns = NamespaceSandbox::new();
        if let crate::Availability::Unavailable(why) = ns.availability() {
            eprintln!("SKIP namespace half of the allocator test: {why}");
            return;
        }
        let out = ns.run(&spec).expect("namespace sandbox runs");
        assert!(out.success(), "{:?} {}", out.status, out.stderr_lossy());
    }
}
