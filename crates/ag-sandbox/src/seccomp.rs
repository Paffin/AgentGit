//! The seccomp-bpf deny-list installed by [`crate::NamespaceSandbox`].
//!
//! The filter is assembled here as classic BPF, by hand, so that every instruction is auditable and the
//! program can be unit-tested against a small BPF interpreter without a kernel. What it does and does not
//! block is documented in `docs/architecture/sandbox.md`; a unit test fails if a syscall listed here is
//! missing from that document.
//!
//! Program shape (all jumps are short, no label resolution is needed):
//!
//! ```text
//!   if arch != native            -> KILL_PROCESS      (blocks the 32-bit compat ABI)
//!   if nr >= 0x4000_0000 (x32)   -> KILL_PROCESS      (x86_64 only)
//!   for each denied (nr, errno): if nr == N -> ERRNO(errno)
//!   clone(flags):   flags & CLONE_NEW*          -> ERRNO(EPERM)
//!   unshare(flags): flags & (CLONE_NEW*|NEWTIME) -> ERRNO(EPERM)
//!   socket(family): family not in {UNIX, INET, INET6, NETLINK} -> ERRNO(EAFNOSUPPORT)
//!   otherwise -> ALLOW
//! ```

use crate::error::SandboxError;

const EPERM: u32 = libc::EPERM as u32;
const ENOSYS: u32 = libc::ENOSYS as u32;
const EAFNOSUPPORT: u32 = libc::EAFNOSUPPORT as u32;

const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;

// Classic BPF opcodes (linux/filter.h).
const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JEQ_K: u16 = 0x15;
#[cfg(any(target_arch = "x86_64", test))]
const BPF_JGE_K: u16 = 0x35;
const BPF_JSET_K: u16 = 0x45;
const BPF_RET_K: u16 = 0x06;

// Offsets into `struct seccomp_data { int nr; __u32 arch; __u64 ip; __u64 args[6]; }` (little endian).
const OFF_NR: u32 = 0;
const OFF_ARCH: u32 = 4;
const OFF_ARG0: u32 = 16;

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xC000_003E;
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xC000_00B7;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const AUDIT_ARCH: u32 = 0;

const CLONE_NEWNS: u32 = 0x0002_0000;
const CLONE_NEWCGROUP: u32 = 0x0200_0000;
const CLONE_NEWUTS: u32 = 0x0400_0000;
const CLONE_NEWIPC: u32 = 0x0800_0000;
const CLONE_NEWUSER: u32 = 0x1000_0000;
const CLONE_NEWPID: u32 = 0x2000_0000;
const CLONE_NEWNET: u32 = 0x4000_0000;
const CLONE_NEWTIME: u32 = 0x0000_0080;
const CLONE_NS_MASK: u32 =
    CLONE_NEWNS | CLONE_NEWCGROUP | CLONE_NEWUTS | CLONE_NEWIPC | CLONE_NEWUSER | CLONE_NEWPID | CLONE_NEWNET;
// `CLONE_NEWTIME` shares its bit with part of the exit-signal field of `clone`, so it is only meaningful
// (and only checked) for `unshare`.
const UNSHARE_NS_MASK: u32 = CLONE_NS_MASK | CLONE_NEWTIME;

/// Socket families a sandboxed command may create. Everything else (`AF_ALG`, `AF_PACKET`, `AF_VSOCK`,
/// `AF_BLUETOOTH`, `AF_KEY`, ...) is large kernel attack surface that build and test jobs do not need.
const ALLOWED_FAMILIES: [u32; 4] =
    [libc::AF_UNIX as u32, libc::AF_INET as u32, libc::AF_INET6 as u32, libc::AF_NETLINK as u32];

macro_rules! deny {
    ($errno:expr; $($name:ident),* $(,)?) => {
        &[$((stringify!($name), libc::$name as u32, $errno)),*]
    };
}

/// `(name, syscall number, errno)` for every syscall that is refused outright.
type Denied = &'static [(&'static str, u32, u32)];

// Written with the `SYS_` prefix so the names match `libc::SYS_*`; `denied_syscalls` strips it.
const DENIED_COMMON: Denied = deny![EPERM;
    // Debugging and cross-process memory access.
    SYS_ptrace, SYS_process_vm_readv, SYS_process_vm_writev, SYS_kcmp, SYS_pidfd_getfd,
    // Changing the file system namespace after setup (the capability bounding set already stops these;
    // the filter is the second barrier).
    SYS_mount, SYS_umount2, SYS_pivot_root, SYS_chroot, SYS_move_mount, SYS_open_tree, SYS_fsopen,
    SYS_fsconfig, SYS_fsmount, SYS_fspick, SYS_mount_setattr,
    // Joining other namespaces (creating new ones is handled by the argument filters below).
    SYS_setns,
    // Kernel and machine administration.
    SYS_kexec_load, SYS_kexec_file_load, SYS_reboot, SYS_init_module, SYS_finit_module, SYS_delete_module,
    SYS_swapon, SYS_swapoff, SYS_acct, SYS_quotactl, SYS_settimeofday, SYS_clock_settime, SYS_clock_adjtime,
    SYS_adjtimex, SYS_sethostname, SYS_setdomainname, SYS_syslog, SYS_lookup_dcookie,
    // Kernel key management (keyrings are shared across user namespaces).
    SYS_keyctl, SYS_add_key, SYS_request_key,
    // Rarely needed, historically rich in vulnerabilities.
    SYS_bpf, SYS_perf_event_open, SYS_userfaultfd, SYS_io_uring_setup, SYS_io_uring_enter,
    SYS_io_uring_register, SYS_open_by_handle_at, SYS_name_to_handle_at, SYS_fanotify_init,
];

#[cfg(target_arch = "x86_64")]
const DENIED_ARCH: Denied = deny![EPERM;
    SYS_iopl, SYS_ioperm, SYS_modify_ldt, SYS_uselib, SYS__sysctl, SYS_sysfs, SYS_nfsservctl,
];
#[cfg(not(target_arch = "x86_64"))]
const DENIED_ARCH: Denied = &[];

/// `clone3` takes its flags from a user-space struct, which a seccomp filter cannot inspect. It is
/// therefore refused with `ENOSYS`, which makes libc fall back to plain `clone` (whose flags are filtered).
const DENIED_ENOSYS: Denied = deny![ENOSYS; SYS_clone3];

/// All outright-denied syscalls: `(name without the SYS_ prefix, errno)`.
#[cfg(test)]
pub(crate) fn denied_syscalls() -> Vec<(&'static str, u32)> {
    DENIED_COMMON
        .iter()
        .chain(DENIED_ARCH)
        .chain(DENIED_ENOSYS)
        .map(|(name, _, errno)| (name.strip_prefix("SYS_").unwrap_or(name), *errno))
        .collect()
}

/// Whether this crate can build a filter for the host architecture.
pub(crate) const fn supported() -> bool {
    cfg!(all(target_endian = "little", any(target_arch = "x86_64", target_arch = "aarch64")))
}

fn stmt(code: u16, k: u32) -> libc::sock_filter {
    libc::sock_filter { code, jt: 0, jf: 0, k }
}

fn jump(code: u16, k: u32, jt: u8, jf: u8) -> libc::sock_filter {
    libc::sock_filter { code, jt, jf, k }
}

/// Assemble the filter.
pub(crate) fn build() -> Result<Vec<libc::sock_filter>, SandboxError> {
    if !supported() {
        return Err(SandboxError::Unavailable {
            sandbox: "namespace",
            reason: "no seccomp filter is defined for this CPU architecture (supported: x86_64, aarch64 little-endian)"
                .into(),
        });
    }
    let mut p = Vec::with_capacity(256);

    // Architecture gate: syscall numbers are only meaningful for the native ABI.
    p.push(stmt(BPF_LD_W_ABS, OFF_ARCH));
    p.push(jump(BPF_JEQ_K, AUDIT_ARCH, 1, 0));
    p.push(stmt(BPF_RET_K, SECCOMP_RET_KILL_PROCESS));
    p.push(stmt(BPF_LD_W_ABS, OFF_NR));
    #[cfg(target_arch = "x86_64")]
    {
        // The x32 ABI reuses the x86_64 audit arch with bit 30 set in the number; refuse it altogether.
        p.push(jump(BPF_JGE_K, 0x4000_0000, 0, 1));
        p.push(stmt(BPF_RET_K, SECCOMP_RET_KILL_PROCESS));
    }

    for (_, nr, errno) in DENIED_COMMON.iter().chain(DENIED_ARCH).chain(DENIED_ENOSYS) {
        p.push(jump(BPF_JEQ_K, *nr, 0, 1));
        p.push(stmt(BPF_RET_K, SECCOMP_RET_ERRNO | errno));
    }

    // clone(flags, ...) / unshare(flags): refuse any namespace-creating flag. Each block reloads `nr`
    // because the previous block may have clobbered the accumulator.
    for (nr, mask) in [(libc::SYS_clone as u32, CLONE_NS_MASK), (libc::SYS_unshare as u32, UNSHARE_NS_MASK)] {
        p.push(stmt(BPF_LD_W_ABS, OFF_NR));
        p.push(jump(BPF_JEQ_K, nr, 0, 3));
        p.push(stmt(BPF_LD_W_ABS, OFF_ARG0));
        p.push(jump(BPF_JSET_K, mask, 0, 1));
        p.push(stmt(BPF_RET_K, SECCOMP_RET_ERRNO | EPERM));
    }

    // socket(family, ...): allow only the families builds and tests use.
    p.push(stmt(BPF_LD_W_ABS, OFF_NR));
    let block = ALLOWED_FAMILIES.len() + 2; // ld arg0, the compares, the final ret
    p.push(jump(BPF_JEQ_K, libc::SYS_socket as u32, 0, block as u8));
    p.push(stmt(BPF_LD_W_ABS, OFF_ARG0));
    for (i, fam) in ALLOWED_FAMILIES.iter().enumerate() {
        // On a match jump over the remaining compares and the ERRNO return.
        let remaining = ALLOWED_FAMILIES.len() - i - 1;
        p.push(jump(BPF_JEQ_K, *fam, (remaining + 1) as u8, 0));
    }
    p.push(stmt(BPF_RET_K, SECCOMP_RET_ERRNO | EAFNOSUPPORT));

    p.push(stmt(BPF_RET_K, SECCOMP_RET_ALLOW));

    if p.len() > 4096 {
        return Err(SandboxError::invalid("seccomp program exceeds the kernel's 4096-instruction limit"));
    }
    Ok(p)
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const libc::sock_filter,
}

/// Install `prog` on the calling thread (and so on everything it later forks).
///
/// # Safety
/// Async-signal-safe: one `prctl`. `PR_SET_NO_NEW_PRIVS` must already be set (or the caller privileged),
/// `prog` must stay valid for the duration of the call.
#[allow(unsafe_code)]
pub(crate) unsafe fn install(prog: &[libc::sock_filter]) -> libc::c_int {
    let fprog = SockFprog { len: prog.len() as u16, filter: prog.as_ptr() };
    // SAFETY: `fprog` points at `prog`, which outlives the call; the kernel copies the program.
    unsafe {
        libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER as libc::c_ulong, &fprog as *const SockFprog, 0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A just-big-enough classic-BPF interpreter: the subset of instructions `build` emits.
    fn run(prog: &[libc::sock_filter], arch: u32, nr: u32, arg0: u32) -> u32 {
        let load = |off: u32| match off {
            OFF_NR => nr,
            OFF_ARCH => arch,
            OFF_ARG0 => arg0,
            other => panic!("unexpected load at offset {other}"),
        };
        let (mut a, mut pc) = (0u32, 0usize);
        loop {
            let i = prog[pc];
            pc += 1;
            match i.code {
                BPF_LD_W_ABS => a = load(i.k),
                BPF_RET_K => return i.k,
                c @ (BPF_JEQ_K | BPF_JGE_K | BPF_JSET_K) => {
                    let hit = match c {
                        BPF_JEQ_K => a == i.k,
                        BPF_JGE_K => a >= i.k,
                        _ => a & i.k != 0,
                    };
                    pc += usize::from(if hit { i.jt } else { i.jf });
                }
                other => panic!("unexpected opcode {other:#x}"),
            }
            assert!(pc < prog.len(), "jumped past the end of the program");
        }
    }

    fn verdict(prog: &[libc::sock_filter], nr: i64, arg0: u32) -> u32 {
        run(prog, AUDIT_ARCH, nr as u32, arg0)
    }

    fn errno(e: u32) -> u32 {
        SECCOMP_RET_ERRNO | e
    }

    #[test]
    fn denied_syscalls_get_their_documented_errno() {
        let prog = build().unwrap();
        for (name, nr, e) in DENIED_COMMON.iter().chain(DENIED_ARCH).chain(DENIED_ENOSYS) {
            assert_eq!(verdict(&prog, *nr as i64, 0), errno(*e), "{name}");
        }
    }

    #[test]
    fn ordinary_syscalls_are_allowed() {
        let prog = build().unwrap();
        for nr in [
            libc::SYS_read,
            libc::SYS_write,
            libc::SYS_openat,
            libc::SYS_mmap,
            libc::SYS_execve,
            libc::SYS_futex,
            libc::SYS_wait4,
            libc::SYS_kill,
            libc::SYS_prctl,
            libc::SYS_ioctl,
            libc::SYS_memfd_create,
            libc::SYS_seccomp,
            libc::SYS_exit_group,
        ] {
            assert_eq!(verdict(&prog, nr, 0), SECCOMP_RET_ALLOW, "syscall {nr}");
        }
    }

    #[test]
    fn clone_and_unshare_are_filtered_by_their_namespace_flags_only() {
        let prog = build().unwrap();
        let clone = libc::SYS_clone;
        let thread_flags =
            (libc::CLONE_VM | libc::CLONE_FS | libc::CLONE_FILES | libc::CLONE_SIGHAND | libc::CLONE_THREAD) as u32;
        assert_eq!(verdict(&prog, clone, thread_flags), SECCOMP_RET_ALLOW);
        assert_eq!(verdict(&prog, clone, libc::SIGCHLD as u32), SECCOMP_RET_ALLOW);
        for bit in [CLONE_NEWNS, CLONE_NEWCGROUP, CLONE_NEWUTS, CLONE_NEWIPC, CLONE_NEWUSER, CLONE_NEWPID, CLONE_NEWNET]
        {
            assert_eq!(verdict(&prog, clone, thread_flags | bit), errno(EPERM), "clone flag {bit:#x}");
            assert_eq!(verdict(&prog, libc::SYS_unshare, bit), errno(EPERM), "unshare flag {bit:#x}");
        }
        assert_eq!(verdict(&prog, libc::SYS_unshare, CLONE_NEWTIME), errno(EPERM));
        // unshare of non-namespace state is harmless
        assert_eq!(verdict(&prog, libc::SYS_unshare, libc::CLONE_FILES as u32), SECCOMP_RET_ALLOW);
        // the accumulator clobbered by the clone block must not break later syscalls
        assert_eq!(verdict(&prog, libc::SYS_socket, libc::AF_UNIX as u32), SECCOMP_RET_ALLOW);
    }

    #[test]
    fn only_ordinary_socket_families_are_allowed() {
        let prog = build().unwrap();
        for fam in ALLOWED_FAMILIES {
            assert_eq!(verdict(&prog, libc::SYS_socket, fam), SECCOMP_RET_ALLOW, "family {fam}");
        }
        for fam in [libc::AF_PACKET, libc::AF_ALG, libc::AF_VSOCK, libc::AF_KEY, libc::AF_BLUETOOTH, 0, 255] {
            assert_eq!(verdict(&prog, libc::SYS_socket, fam as u32), errno(EAFNOSUPPORT), "family {fam}");
        }
        // other syscalls are unaffected by the family list
        assert_eq!(verdict(&prog, libc::SYS_socketpair, libc::AF_UNIX as u32), SECCOMP_RET_ALLOW);
    }

    #[test]
    fn a_foreign_architecture_is_killed_not_interpreted() {
        let prog = build().unwrap();
        // 32-bit x86 compat (AUDIT_ARCH_I386) reaches the same numbers with other meanings
        assert_eq!(run(&prog, 0x4000_0003, libc::SYS_read as u32, 0), SECCOMP_RET_KILL_PROCESS);
        assert_eq!(run(&prog, 0, 0, 0), SECCOMP_RET_KILL_PROCESS);
        #[cfg(target_arch = "x86_64")]
        assert_eq!(run(&prog, AUDIT_ARCH, 0x4000_0000 | libc::SYS_read as u32, 0), SECCOMP_RET_KILL_PROCESS);
    }

    #[test]
    fn the_program_is_small_and_has_no_duplicate_numbers() {
        let prog = build().unwrap();
        assert!(prog.len() < 400, "{} instructions", prog.len());
        let mut nrs: Vec<u32> = DENIED_COMMON.iter().chain(DENIED_ARCH).chain(DENIED_ENOSYS).map(|d| d.1).collect();
        let n = nrs.len();
        nrs.sort_unstable();
        nrs.dedup();
        assert_eq!(n, nrs.len(), "a syscall is listed twice");
    }

    #[test]
    fn every_denied_syscall_is_documented() {
        let doc_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/architecture/sandbox.md");
        let doc = std::fs::read_to_string(&doc_path).expect("docs/architecture/sandbox.md exists");
        for (name, _) in denied_syscalls() {
            assert!(doc.contains(&format!("`{name}`")), "docs/architecture/sandbox.md does not mention `{name}`");
        }
        for fam in ["AF_UNIX", "AF_INET6", "AF_NETLINK", "AF_ALG", "AF_PACKET"] {
            assert!(doc.contains(fam), "docs do not mention {fam}");
        }
    }
}
