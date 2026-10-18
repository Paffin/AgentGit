use std::ffi::CString;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::time::Duration;

fn errname(e: i32) -> String {
    let name = match e {
        libc::EPERM => "EPERM",
        libc::ENOENT => "ENOENT",
        libc::ESRCH => "ESRCH",
        libc::EINTR => "EINTR",
        libc::EIO => "EIO",
        libc::EBADF => "EBADF",
        libc::EAGAIN => "EAGAIN",
        libc::ENOMEM => "ENOMEM",
        libc::EACCES => "EACCES",
        libc::EFAULT => "EFAULT",
        libc::EBUSY => "EBUSY",
        libc::EEXIST => "EEXIST",
        libc::ENOTDIR => "ENOTDIR",
        libc::EINVAL => "EINVAL",
        libc::EMFILE => "EMFILE",
        libc::EFBIG => "EFBIG",
        libc::ENOSPC => "ENOSPC",
        libc::EROFS => "EROFS",
        libc::ENOSYS => "ENOSYS",
        libc::EAFNOSUPPORT => "EAFNOSUPPORT",
        libc::ENETUNREACH => "ENETUNREACH",
        libc::ECONNREFUSED => "ECONNREFUSED",
        libc::ETIMEDOUT => "ETIMEDOUT",
        libc::EOVERFLOW => "EOVERFLOW",
        libc::ENOTSUP => "ENOTSUP",
        libc::EXDEV => "EXDEV",
        libc::ENODEV => "ENODEV",
        libc::ENXIO => "ENXIO",
        libc::ELOOP => "ELOOP",
        libc::EPROTONOSUPPORT => "EPROTONOSUPPORT",
        other => return format!("E{other}"),
    };
    name.to_string()
}

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Print `ok` for a non-negative result and `err NAME` for -1.
fn report(ret: i64) {
    if ret < 0 { println!("err {}", errname(last_errno())) } else { println!("ok {ret}") }
}

fn io_report<T>(r: std::io::Result<T>) {
    match r {
        Ok(_) => println!("ok"),
        Err(e) => println!("err {}", errname(e.raw_os_error().unwrap_or(0))),
    }
}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap()
}

fn status_field(name: &str) -> String {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{name}:")).map(|v| v.trim().to_string()))
        .unwrap_or_default()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("");
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
    let num = |i: usize| args.get(i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);

    match cmd {
        // --- identity and the kernel's view of our restrictions -------------------------------------------------
        "status" => {
            for f in [
                "NoNewPrivs",
                "Seccomp",
                "Seccomp_filters",
                "CapInh",
                "CapPrm",
                "CapEff",
                "CapBnd",
                "CapAmb",
                "Uid",
                "Gid",
                "Groups",
            ] {
                println!("{f}={}", status_field(f));
            }
        }
        "ids" => unsafe {
            println!(
                "uid={} euid={} gid={} egid={} pid={}",
                libc::getuid(),
                libc::geteuid(),
                libc::getgid(),
                libc::getegid(),
                libc::getpid()
            );
        },
        "euid" => unsafe { println!("{}", libc::geteuid()) },
        "hostname" => {
            let mut buf = [0u8; 256];
            unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
            println!("{}", String::from_utf8_lossy(&buf).trim_end_matches('\0'));
        }
        "env" => {
            // The environment the kernel handed to this process at `execve`, i.e. exactly what the sandbox passed.
            // `std::env::vars()` would also show what this process added to its own environment afterwards: an
            // instrumented build (cargo llvm-cov) has the profiling runtime set `__LLVM_PROFILE_RT_INIT_ONCE` at start-up.
            let mut v: Vec<String> = match std::fs::read("/proc/self/environ") {
                Ok(raw) => raw
                    .split(|b| *b == 0)
                    .filter(|e| !e.is_empty())
                    .map(|e| String::from_utf8_lossy(e).into_owned())
                    .collect(),
                Err(_) => std::env::vars().map(|(k, v)| format!("{k}={v}")).collect(),
            };
            v.sort();
            for l in v {
                println!("{l}");
            }
        }
        "fds" => {
            let mut v: Vec<String> = std::fs::read_dir("/proc/self/fd")
                .unwrap()
                .flatten()
                .map(|e| {
                    format!(
                        "{} -> {}",
                        e.file_name().to_string_lossy(),
                        std::fs::read_link(e.path()).map(|p| p.display().to_string()).unwrap_or_default()
                    )
                })
                .collect();
            v.sort();
            for l in v {
                println!("{l}");
            }
        }

        // --- file system ---------------------------------------------------------------------------------------
        "read" => match std::fs::read(arg(2)) {
            Ok(b) => println!("ok {}", b.len()),
            Err(e) => println!("err {}", errname(e.raw_os_error().unwrap_or(0))),
        },
        "write" => io_report(std::fs::write(arg(2), arg(3))),
        "mkdir" => io_report(std::fs::create_dir(arg(2))),
        "list" => match std::fs::read_dir(arg(2)) {
            Ok(rd) => {
                let mut v: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
                v.sort();
                println!("ok {}", v.join(" "));
            }
            Err(e) => println!("err {}", errname(e.raw_os_error().unwrap_or(0))),
        },
        "fsize" => {
            // write `n` bytes in 64 KiB pieces; SIGXFSZ normally ends this before the error shows
            let n = num(3) as usize;
            let mut f = match std::fs::File::create(arg(2)) {
                Ok(f) => f,
                Err(e) => return println!("err {}", errname(e.raw_os_error().unwrap_or(0))),
            };
            let chunk = vec![b'z'; 64 * 1024];
            let mut left = n;
            while left > 0 {
                let k = left.min(chunk.len());
                if let Err(e) = f.write_all(&chunk[..k]) {
                    return println!("err {}", errname(e.raw_os_error().unwrap_or(0)));
                }
                left -= k;
            }
            println!("ok");
        }
        "fill-tmp" => {
            // fill a directory with a file bigger than the tmpfs and report the error
            let mut f = std::fs::File::create(format!("{}/fill", arg(2))).unwrap();
            let chunk = vec![1u8; 1 << 20];
            let mut written = 0u64;
            loop {
                match f.write_all(&chunk) {
                    Ok(()) => written += chunk.len() as u64,
                    Err(e) => {
                        println!("err {} after {}", errname(e.raw_os_error().unwrap_or(0)), written);
                        break;
                    }
                }
                if written > (1 << 30) {
                    println!("ok unlimited {written}");
                    break;
                }
            }
        }
        "openfiles" => {
            let mut files = Vec::new();
            loop {
                match std::fs::File::open("/dev/null") {
                    Ok(f) => files.push(f),
                    Err(e) => {
                        println!("err {} after {}", errname(e.raw_os_error().unwrap_or(0)), files.len());
                        break;
                    }
                }
                if files.len() > 100_000 {
                    println!("ok unlimited");
                    break;
                }
            }
        }

        // --- system calls the sandbox must refuse -------------------------------------------------------------------
        "syscall" => syscall(&arg(2), &args),
        "mount" => report(unsafe {
            libc::mount(cstr("none").as_ptr(), cstr(&arg(2)).as_ptr(), cstr("tmpfs").as_ptr(), 0, std::ptr::null())
        } as i64),
        "remount-rw" => report(unsafe {
            libc::mount(
                std::ptr::null(),
                cstr(&arg(2)).as_ptr(),
                std::ptr::null(),
                libc::MS_REMOUNT | libc::MS_BIND,
                std::ptr::null(),
            )
        } as i64),
        "umount" => report(unsafe { libc::umount2(cstr(&arg(2)).as_ptr(), 0) } as i64),
        "kill" => report(unsafe { libc::kill(num(2) as i32, 0) } as i64),

        // --- network --------------------------------------------------------------------------------------------
        "connect" => {
            let addr = arg(2).to_socket_addrs().ok().and_then(|mut a| a.next());
            match addr.map(|a| TcpStream::connect_timeout(&a, Duration::from_secs(3))) {
                Some(Ok(_)) => println!("ok connected"),
                Some(Err(e)) => println!("err {}", errname(e.raw_os_error().unwrap_or(0))),
                None => println!("err EINVAL"),
            }
        }
        "unix-abstract" => {
            use std::os::linux::net::SocketAddrExt;
            let addr = std::os::unix::net::SocketAddr::from_abstract_name(arg(2)).unwrap();
            match std::os::unix::net::UnixStream::connect_addr(&addr) {
                Ok(_) => println!("ok connected"),
                Err(e) => println!("err {}", errname(e.raw_os_error().unwrap_or(0))),
            }
        }
        "loopback" => {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = l.local_addr().unwrap().port();
            let t = std::thread::spawn(move || {
                let (mut s, _) = l.accept().unwrap();
                let mut b = [0u8; 4];
                s.read_exact(&mut b).unwrap();
                s.write_all(&b).unwrap();
            });
            let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
            c.write_all(b"ping").unwrap();
            let mut b = [0u8; 4];
            c.read_exact(&mut b).unwrap();
            t.join().unwrap();
            println!("ok {}", String::from_utf8_lossy(&b));
        }
        "shmat" => {
            let p = unsafe { libc::shmat(num(2) as i32, std::ptr::null(), 0) };
            if p as isize == -1 { println!("err {}", errname(last_errno())) } else { println!("ok attached") }
        }

        // --- resources ---------------------------------------------------------------------------------------------
        "forkbomb" => forkbomb(num(2), num(3)),
        "spawner" => loop {
            // keep trying to create sleeping children forever; failures only make it wait a moment
            let pid = unsafe { libc::fork() };
            if pid == 0 {
                loop {
                    unsafe { libc::pause() };
                }
            }
            if pid < 0 {
                std::thread::sleep(Duration::from_micros(300));
            }
        },
        "threadbomb" => threadbomb(num(2)),
        "alloc" => alloc(num(2), num(3).max(1)),
        "alloc-procs" => alloc_procs(num(2), num(3)),
        "spin" => loop {
            std::hint::black_box(0u64);
        },
        "flood" => {
            let n = num(2) as usize;
            let chunk = vec![b'x'; 64 * 1024];
            let mut left = n;
            let to_err = arg(3) == "stderr";
            while left > 0 {
                let k = left.min(chunk.len());
                let r = if to_err {
                    std::io::stderr().write_all(&chunk[..k])
                } else {
                    std::io::stdout().write_all(&chunk[..k])
                };
                if r.is_err() {
                    return;
                }
                left -= k;
            }
            let _ = std::io::stdout().flush();
        }
        "sleep" => std::thread::sleep(Duration::from_millis(num(2))),
        "stdin-sum" => {
            let mut data = Vec::new();
            std::io::stdin().read_to_end(&mut data).unwrap();
            let sum: u64 = data.iter().map(|&b| u64::from(b)).sum();
            println!("len={} sum={sum}", data.len());
        }
        "exit" => std::process::exit(num(2) as i32),
        "raise" => unsafe {
            // Rust installs handlers for SIGSEGV/SIGBUS; the default action is what we want to observe
            libc::signal(num(2) as i32, libc::SIG_DFL);
            libc::raise(num(2) as i32);
            std::thread::sleep(Duration::from_secs(5));
        },
        other => {
            eprintln!("unknown probe command {other:?}");
            std::process::exit(64);
        }
    }
}

fn syscall(name: &str, args: &[String]) {
    let _ = args;
    let null = std::ptr::null::<u8>();
    let r: i64 = unsafe {
        match name {
            "ptrace" => libc::ptrace(libc::PTRACE_TRACEME, 0, null, null) as i64,
            "unshare-user" => libc::unshare(libc::CLONE_NEWUSER) as i64,
            "unshare-net" => libc::unshare(libc::CLONE_NEWNET) as i64,
            "unshare-mount" => libc::unshare(libc::CLONE_NEWNS) as i64,
            "unshare-pid" => libc::unshare(libc::CLONE_NEWPID) as i64,
            "unshare-files" => libc::unshare(libc::CLONE_FILES) as i64,
            "clone-newuser" => {
                libc::syscall(libc::SYS_clone, (libc::CLONE_NEWUSER | libc::SIGCHLD) as u64, 0u64, 0u64, 0u64, 0u64)
            }
            "clone-newnet" => {
                libc::syscall(libc::SYS_clone, (libc::CLONE_NEWNET | libc::SIGCHLD) as u64, 0u64, 0u64, 0u64, 0u64)
            }
            "clone3" => libc::syscall(libc::SYS_clone3, null, 0usize),
            "setns" => libc::setns(0, libc::CLONE_NEWNET) as i64,
            "bpf" => libc::syscall(libc::SYS_bpf, 0, null, 0),
            "perf_event_open" => libc::syscall(libc::SYS_perf_event_open, null, 0, -1, -1, 0),
            "io_uring_setup" => libc::syscall(libc::SYS_io_uring_setup, 8, null),
            "keyctl" => {
                libc::syscall(libc::SYS_keyctl, 0 /* KEYCTL_GET_KEYRING_ID */, -3i64 /* session keyring */, 0)
            }
            "userfaultfd" => libc::syscall(libc::SYS_userfaultfd, 0),
            "process_vm_readv" => libc::syscall(libc::SYS_process_vm_readv, 1, null, 0, null, 0, 0),
            "kexec_load" => libc::syscall(libc::SYS_kexec_load, 0, 0, null, 0),
            "reboot" => libc::syscall(libc::SYS_reboot, 0, 0, 0, null),
            "swapon" => libc::syscall(libc::SYS_swapon, cstr("/dev/null").as_ptr(), 0),
            "settimeofday" => libc::syscall(libc::SYS_settimeofday, null, null),
            "sethostname" => libc::sethostname(cstr("evil").as_ptr(), 4) as i64,
            "chroot" => libc::chroot(cstr("/tmp").as_ptr()) as i64,
            "pivot_root" => libc::syscall(libc::SYS_pivot_root, cstr("/tmp").as_ptr(), cstr("/tmp").as_ptr()),
            "mknod" => libc::mknod(cstr("/tmp/null2").as_ptr(), libc::S_IFCHR | 0o666, libc::makedev(1, 3)) as i64,
            "setuid0" => libc::setuid(0) as i64,
            "socket-alg" => libc::socket(libc::AF_ALG, libc::SOCK_SEQPACKET, 0) as i64,
            "socket-packet" => libc::socket(libc::AF_PACKET, libc::SOCK_RAW, 0) as i64,
            "socket-vsock" => libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0) as i64,
            "socket-netlink-raw" => libc::socket(libc::AF_NETLINK, libc::SOCK_RAW, libc::NETLINK_ROUTE) as i64,
            "socket-unix" => libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) as i64,
            "socket-inet" => libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) as i64,
            "socket-inet6" => libc::socket(libc::AF_INET6, libc::SOCK_STREAM, 0) as i64,
            "memfd" => libc::syscall(libc::SYS_memfd_create, cstr("x").as_ptr(), 0),
            other => {
                eprintln!("unknown syscall {other:?}");
                std::process::exit(64);
            }
        }
    };
    report(r);
}

fn forkbomb(limit: u64, pause_us: u64) {
    let mut pids = Vec::new();
    let mut err = 0;
    for _ in 0..limit {
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            err = last_errno();
            break;
        }
        if pid == 0 {
            // child: stay alive until the parent has counted and killed us
            unsafe {
                libc::pause();
                libc::_exit(0)
            }
        }
        pids.push(pid);
        if pause_us > 0 {
            std::thread::sleep(Duration::from_micros(pause_us));
        }
    }
    println!("forked {} stopped_by {}", pids.len(), if err == 0 { "limit-not-hit".to_string() } else { errname(err) });
    let _ = std::io::stdout().flush();
    for p in &pids {
        unsafe { libc::kill(*p, libc::SIGKILL) };
    }
    for p in &pids {
        unsafe { libc::waitpid(*p, std::ptr::null_mut(), 0) };
    }
}

fn threadbomb(limit: u64) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let mut handles = Vec::new();
    let mut err = String::from("limit-not-hit");
    for _ in 0..limit {
        let rx = rx.clone();
        match std::thread::Builder::new().stack_size(64 * 1024).spawn(move || {
            let _ = rx.lock().map(|r| r.recv());
        }) {
            Ok(h) => handles.push(h),
            Err(e) => {
                err = errname(e.raw_os_error().unwrap_or(0));
                break;
            }
        }
    }
    println!("threads {} stopped_by {err}", handles.len());
    let _ = std::io::stdout().flush();
    drop(tx);
    for h in handles {
        let _ = h.join();
    }
}

/// Allocate and touch `total_mib` in 8 MiB steps, reporting progress; hold the memory briefly at the end.
fn alloc(total_mib: u64, hold_ms: u64) {
    let mut held: Vec<Vec<u8>> = Vec::new();
    let mut mib = 0;
    while mib < total_mib {
        let mut v: Vec<u8> = Vec::new();
        if v.try_reserve_exact(8 << 20).is_err() {
            println!("alloc-failed after {mib} MiB");
            return;
        }
        v.resize(8 << 20, 0xAB);
        std::hint::black_box(&v);
        held.push(v);
        mib += 8;
        println!("allocated {mib}");
        let _ = std::io::stdout().flush();
    }
    println!("done {mib}");
    std::thread::sleep(Duration::from_millis(hold_ms * 100));
    std::hint::black_box(&held);
}

fn alloc_procs(n: u64, mib_each: u64) {
    let mut pids = Vec::new();
    for _ in 0..n {
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            break;
        }
        if pid == 0 {
            let mut v = vec![0u8; (mib_each as usize) << 20];
            for i in (0..v.len()).step_by(4096) {
                v[i] = 1;
            }
            std::hint::black_box(&v);
            std::thread::sleep(Duration::from_secs(30));
            unsafe { libc::_exit(0) };
        }
        pids.push(pid);
    }
    println!("started {}", pids.len());
    let _ = std::io::stdout().flush();
    for p in pids {
        unsafe { libc::waitpid(p, std::ptr::null_mut(), 0) };
    }
}
