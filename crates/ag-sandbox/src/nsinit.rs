//! The Linux-only part of the cloned child: the code of the PID-namespace init that builds the sandbox's
//! file system and drops privileges. Same rules as `spawn.rs`: raw system calls over prebuilt data, no
//! allocation, never returns.
#![allow(unsafe_code)]

use std::ffi::{CStr, CString};

use libc::{c_int, c_ulong};

use crate::spawn::{ChildFds, Plan, check, fail, raw, stage, worker_main};

/// One step of building the sandbox's file system, executed by the child after `pivot_root`.
#[derive(Debug)]
pub(crate) enum MountOp {
    Mkdir(CString),
    /// Create an empty file to serve as the target of a file bind mount.
    Touch(CString),
    Symlink {
        target: CString,
        link: CString,
    },
    /// Mount a fresh file system.
    Fs {
        fstype: &'static CStr,
        target: CString,
        flags: c_ulong,
        data: Option<CString>,
    },
    /// Bind `source` on `target`, then remount it with `extra` flags added to the ones it already has.
    Bind {
        source: CString,
        target: CString,
        extra: c_ulong,
    },
}

impl MountOp {
    fn describe(&self) -> String {
        match self {
            MountOp::Mkdir(p) => format!("mkdir {}", p.to_string_lossy()),
            MountOp::Touch(p) => format!("create {}", p.to_string_lossy()),
            MountOp::Symlink { link, .. } => format!("symlink {}", link.to_string_lossy()),
            MountOp::Fs { fstype, target, .. } => {
                format!("mount {} on {}", fstype.to_string_lossy(), target.to_string_lossy())
            }
            MountOp::Bind { target, .. } => format!("bind mount on {}", target.to_string_lossy()),
        }
    }
}

/// What the PID-namespace init does before it starts the command.
#[derive(Debug)]
pub(crate) struct NsPlan {
    /// Become the mapped inner id explicitly. Needed when the id map points at a host id other than the
    /// caller's own (root mapping to an unprivileged user): the process still carries its original kernel
    /// ids, which the map does not cover.
    pub switch_ids: Option<u32>,
    pub bring_up_loopback: bool,
    pub hostname: CString,
    /// An existing host directory to put the new root's tmpfs on (it is only visible in this mount namespace).
    pub base: CString,
    /// `base`/oldroot: where the old root lands during `pivot_root`.
    pub put_old: CString,
    pub root_opts: CString,
    pub ops: Vec<MountOp>,
}

/// Name of mount operation number `index` of `plan`, for error messages.
pub(crate) fn describe_op(plan: &Plan, index: u32) -> String {
    plan.ns
        .as_ref()
        .and_then(|ns| ns.ops.get(index as usize))
        .map(MountOp::describe)
        .unwrap_or_else(|| "mount setup".into())
}

mod ops {
    use super::*;

    pub fn mount(
        src: Option<&CStr>,
        target: &CStr,
        fstype: Option<&CStr>,
        flags: c_ulong,
        data: Option<&CStr>,
    ) -> c_int {
        let p = |c: Option<&CStr>| c.map_or(std::ptr::null(), CStr::as_ptr);
        // SAFETY: every pointer is null or a valid NUL-terminated string that outlives the call.
        unsafe { libc::mount(p(src), target.as_ptr(), p(fstype), flags, p(data).cast()) }
    }

    pub fn mkdir(path: &CStr) -> c_int {
        // SAFETY: valid C string.
        let r = unsafe { libc::mkdir(path.as_ptr(), 0o755) };
        if r != 0 && raw::errno() == libc::EEXIST { 0 } else { r }
    }

    pub fn touch(path: &CStr) -> c_int {
        // SAFETY: valid C string; the descriptor is closed immediately.
        unsafe {
            let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_CLOEXEC, 0o644 as libc::c_uint);
            if fd < 0 {
                return -1;
            }
            libc::close(fd);
        }
        0
    }

    pub fn symlink(target: &CStr, link: &CStr) -> c_int {
        // SAFETY: valid C strings.
        let r = unsafe { libc::symlink(target.as_ptr(), link.as_ptr()) };
        if r != 0 && raw::errno() == libc::EEXIST { 0 } else { r }
    }

    pub fn rmdir(path: &CStr) -> c_int {
        // SAFETY: valid C string.
        unsafe { libc::rmdir(path.as_ptr()) }
    }

    pub fn umount_detach(path: &CStr) -> c_int {
        // SAFETY: valid C string.
        unsafe { libc::umount2(path.as_ptr(), libc::MNT_DETACH) }
    }

    pub fn pivot_root(new_root: &CStr, put_old: &CStr) -> c_int {
        // SAFETY: valid C strings; pivot_root has no libc wrapper, so the raw syscall is used.
        unsafe { libc::syscall(libc::SYS_pivot_root, new_root.as_ptr(), put_old.as_ptr()) as c_int }
    }

    /// The mount flags (`MS_*` values) currently in effect on the mount containing `path`.
    pub fn mount_flags(path: &CStr) -> Option<c_ulong> {
        // The kernel's `struct statfs` on 64-bit Linux; libc's definition does not expose `f_flags`.
        #[repr(C)]
        struct KernelStatfs {
            f_type: i64,
            f_bsize: i64,
            f_blocks: u64,
            f_bfree: u64,
            f_bavail: u64,
            f_files: u64,
            f_ffree: u64,
            f_fsid: [i32; 2],
            f_namelen: i64,
            f_frsize: i64,
            f_flags: i64,
            f_spare: [i64; 4],
        }
        let mut st = KernelStatfs {
            f_type: 0,
            f_bsize: 0,
            f_blocks: 0,
            f_bfree: 0,
            f_bavail: 0,
            f_files: 0,
            f_ffree: 0,
            f_fsid: [0; 2],
            f_namelen: 0,
            f_frsize: 0,
            f_flags: 0,
            f_spare: [0; 4],
        };
        // SAFETY: `st` has the size and layout of the kernel's 64-bit `struct statfs`, and `path` is a valid C string.
        let r = unsafe { libc::syscall(libc::SYS_statfs, path.as_ptr(), &mut st as *mut KernelStatfs) };
        // The kernel's ST_* mount flags are numerically the MS_* flags for the ones we care about.
        (r == 0).then_some(st.f_flags as c_ulong)
    }

    pub fn sethostname(name: &CStr) -> c_int {
        // SAFETY: pointer and length describe the C string.
        unsafe { libc::sethostname(name.as_ptr(), name.to_bytes().len()) }
    }

    /// Set real, effective and saved ids to `id` with raw system calls (see `clear_supplementary_groups` for
    /// why glibc's `setres*id` must not be used here), keeping the capabilities the process holds in its user
    /// namespace: without `SECBIT_NO_SETUID_FIXUP` the kernel would clear them on leaving uid 0.
    pub fn switch_ids(id: u32) -> c_int {
        const SECBIT_NO_SETUID_FIXUP: c_ulong = 1 << 2;
        if raw::prctl(libc::PR_SET_SECUREBITS, SECBIT_NO_SETUID_FIXUP, 0) < 0 {
            return -1;
        }
        // SAFETY: setresgid/setresuid take three integers each.
        unsafe {
            if libc::syscall(libc::SYS_setresgid, id, id, id) < 0 || libc::syscall(libc::SYS_setresuid, id, id, id) < 0
            {
                return -1;
            }
        }
        0
    }

    pub fn clear_supplementary_groups() {
        // The raw system call, NOT glibc's `setgroups`: glibc implements the set*id family by signalling
        // every thread it knows of and waiting for them on a futex. After a `clone` from a multi-threaded
        // process the child still has the parent's thread list but none of the threads, so the wrapper
        // would wait forever. (Found by running the integration tests in parallel; see
        // `concurrent_runs_from_a_multi_threaded_process`.)
        // SAFETY: setgroups(0, NULL) is valid; failure (EPERM after a "deny" setgroups policy) is ignored.
        unsafe {
            libc::syscall(libc::SYS_setgroups, 0 as libc::size_t, std::ptr::null::<libc::gid_t>());
        }
    }

    /// Drop every capability from the bounding, ambient, inheritable, permitted and effective sets.
    pub fn drop_all_capabilities() -> c_int {
        #[repr(C)]
        struct Header {
            version: u32,
            pid: c_int,
        }
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Data {
            effective: u32,
            permitted: u32,
            inheritable: u32,
        }
        const LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;
        // The highest capability number is below 64 on every kernel; EINVAL for numbers past the last is fine.
        for cap in 0..64 {
            raw::prctl(libc::PR_CAPBSET_DROP, cap, 0);
        }
        raw::prctl(libc::PR_CAP_AMBIENT, libc::PR_CAP_AMBIENT_CLEAR_ALL as c_ulong, 0);
        let hdr = Header { version: LINUX_CAPABILITY_VERSION_3, pid: 0 };
        let data = [Data { effective: 0, permitted: 0, inheritable: 0 }; 2];
        // SAFETY: `hdr` and `data` are valid repr(C) structs matching the kernel's capset ABI (v3 takes two
        // data elements).
        unsafe { libc::syscall(libc::SYS_capset, &hdr as *const Header, data.as_ptr()) as c_int }
    }

    /// Bring the `lo` interface up in the current network namespace.
    pub fn bring_up_loopback() -> c_int {
        #[repr(C)]
        struct IfReq {
            name: [u8; 16],
            flags: libc::c_short,
            pad: [u8; 22],
        }
        let mut req = IfReq { name: [0; 16], flags: 0, pad: [0; 22] };
        req.name[0] = b'l';
        req.name[1] = b'o';
        // SAFETY: a datagram socket and two ioctls on a repr(C) struct with the size and layout of `ifreq`
        // (16-byte name followed by a 24-byte union whose first member is the short flags field).
        unsafe {
            let s = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
            if s < 0 {
                return -1;
            }
            let mut rc = libc::ioctl(s, libc::SIOCGIFFLAGS as _, &mut req as *mut IfReq);
            if rc == 0 {
                req.flags |= libc::IFF_UP as libc::c_short;
                rc = libc::ioctl(s, libc::SIOCSIFFLAGS as _, &mut req as *mut IfReq);
            }
            libc::close(s);
            rc
        }
    }
}

/// The PID-namespace init: build the file system, drop privileges, run the command, relay its status.
pub(crate) fn init_main(plan: &Plan, ns: &NsPlan, fds: ChildFds) -> ! {
    // The id maps were written by the parent; clearing the supplementary groups only works if it allowed
    // that (root), so failure is expected and ignored.
    ops::clear_supplementary_groups();
    if let Some(id) = ns.switch_ids {
        check!(fds, stage::SET_IDS, 0, ops::switch_ids(id));
    }
    check!(fds, stage::HOSTNAME, 0, ops::sethostname(&ns.hostname));
    if ns.bring_up_loopback {
        check!(fds, stage::LOOPBACK, 0, ops::bring_up_loopback());
    }

    // Make sure nothing we do below propagates back to the host's mount namespace.
    check!(
        fds,
        stage::MAKE_PRIVATE,
        0,
        ops::mount(None, c"/", None, (libc::MS_REC | libc::MS_PRIVATE) as c_ulong, None)
    );
    check!(
        fds,
        stage::MOUNT_ROOT,
        0,
        ops::mount(
            Some(c"tmpfs"),
            &ns.base,
            Some(c"tmpfs"),
            (libc::MS_NOSUID | libc::MS_NODEV) as c_ulong,
            Some(&ns.root_opts)
        )
    );
    check!(fds, stage::MKDIR_OLD, 0, ops::mkdir(&ns.put_old));
    check!(fds, stage::PIVOT_ROOT, 0, ops::pivot_root(&ns.base, &ns.put_old));
    check!(fds, stage::CHDIR_ROOT, 0, raw::chdir(c"/"));

    // From here the old root is at /oldroot and the plan's bind sources point into it.
    for (i, op) in ns.ops.iter().enumerate() {
        let i = i as u32;
        match op {
            MountOp::Mkdir(p) => check!(fds, stage::OP, i, ops::mkdir(p)),
            MountOp::Touch(p) => check!(fds, stage::OP, i, ops::touch(p)),
            MountOp::Symlink { target, link } => check!(fds, stage::OP, i, ops::symlink(target, link)),
            MountOp::Fs { fstype, target, flags, data } => {
                check!(fds, stage::OP, i, ops::mount(Some(fstype), target, Some(fstype), *flags, data.as_deref()))
            }
            MountOp::Bind { source, target, extra } => {
                check!(fds, stage::OP, i, ops::mount(Some(source), target, None, libc::MS_BIND as c_ulong, None));
                // Add the restrictions while keeping every flag the source mount already carries: in a user
                // namespace the kernel refuses a remount that would clear an inherited ("locked") flag.
                let Some(cur) = ops::mount_flags(target) else { fail(fds.status_w, stage::OP, i) };
                let keep = (libc::MS_RDONLY
                    | libc::MS_NOSUID
                    | libc::MS_NODEV
                    | libc::MS_NOEXEC
                    | libc::MS_NOATIME
                    | libc::MS_NODIRATIME
                    | libc::MS_RELATIME) as c_ulong;
                let flags = (libc::MS_REMOUNT | libc::MS_BIND) as c_ulong | (cur & keep) | *extra;
                check!(fds, stage::OP, i, ops::mount(None, target, None, flags, None));
            }
        }
    }

    // Cut the last tie to the host file system and make the root immutable.
    check!(fds, stage::DETACH_OLD, 0, ops::umount_detach(c"/oldroot"));
    check!(fds, stage::RMDIR_OLD, 0, ops::rmdir(c"/oldroot"));
    {
        let Some(cur) = ops::mount_flags(c"/") else { fail(fds.status_w, stage::REMOUNT_ROOT, 0) };
        let keep =
            (libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOATIME | libc::MS_NODIRATIME | libc::MS_RELATIME) as c_ulong;
        let flags = (libc::MS_REMOUNT | libc::MS_BIND | libc::MS_RDONLY) as c_ulong | (cur & keep);
        check!(fds, stage::REMOUNT_ROOT, 0, ops::mount(None, c"/", None, flags, None));
    }

    check!(fds, stage::CAPS, 0, ops::drop_all_capabilities());
    check!(fds, stage::NO_NEW_PRIVS, 0, raw::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0));
    if !plan.seccomp.is_empty() {
        // SAFETY: the program is a valid sock_filter array that outlives the call; NO_NEW_PRIVS is set above.
        check!(fds, stage::SECCOMP, 0, unsafe { crate::seccomp::install(&plan.seccomp) });
    }

    // Run the command as a child so that this process can stay as init and reap whatever it leaves behind.
    let worker = raw::fork();
    if worker < 0 {
        fail(fds.status_w, stage::FORK, 0);
    }
    if worker == 0 {
        raw::close(fds.result_w);
        worker_main(plan, fds, true);
    }
    // Init keeps no copy of the exec-status pipe, so that the parent sees EOF as soon as the command has
    // successfully exec'd (or a failure record if it did not).
    raw::close(fds.status_w);
    let mut status: c_int = 0;
    loop {
        let r = raw::waitpid_any(&mut status);
        if r == worker {
            break;
        }
        if r < 0 {
            // No children left without having seen the worker: should be impossible.
            raw::exit(crate::spawn::EXIT_SETUP + 1);
        }
    }
    let (kind, value) = if libc::WIFEXITED(status) {
        (1u32, libc::WEXITSTATUS(status))
    } else if libc::WIFSIGNALED(status) {
        (2u32, libc::WTERMSIG(status))
    } else {
        (3u32, status)
    };
    let mut rec = [0u8; 8];
    rec[0..4].copy_from_slice(&kind.to_ne_bytes());
    rec[4..8].copy_from_slice(&value.to_ne_bytes());
    raw::write(fds.result_w, &rec);
    // Exiting init takes every remaining process of the namespace down with it.
    raw::exit(0)
}
