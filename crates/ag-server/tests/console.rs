//! The server as a console process: its log is plain text when redirected, and it stops gracefully when the
//! console (or the service manager) asks it to — SIGINT / SIGTERM on Unix, Ctrl+C / Ctrl+Break on Windows.
//!
//! These run the real `ag-server` binary. See docs/operations/windows.md.
#![allow(missing_docs)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct Running {
    child: Child,
    lines: mpsc::Receiver<String>,
    seen: Vec<String>,
    _dir: tempfile::TempDir,
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn spawn(configure: impl FnOnce(&mut Command)) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let config = dir.path().join("ag.toml");
    std::fs::write(
        &config,
        format!("[server]\nlisten = \"127.0.0.1:{port}\"\ndata_dir = {:?}\n", dir.path().join("data")),
    )
    .unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ag-server"));
    cmd.args(["serve", "--config"])
        .arg(&config)
        .env_remove("NO_COLOR")
        .env_remove("RUST_LOG")
        .env("AG_BOOTSTRAP_TOKEN", "console-test-bootstrap-token-0123456789")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    configure(&mut cmd);
    let mut child = cmd.spawn().unwrap();
    let (tx, rx) = mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        // Raw bytes, lossily: an escape sequence must show up as such, not fail a UTF-8 conversion.
        let mut r = BufReader::new(stdout);
        let mut buf = Vec::new();
        while r.read_until(b'\n', &mut buf).unwrap_or(0) > 0 {
            let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
            buf.clear();
        }
    });
    let mut running = Running { child, lines: rx, seen: Vec::new(), _dir: dir };
    running.wait_for("listening");
    running
}

impl Running {
    /// Reads log lines until one contains `needle` (they are all kept in `seen`).
    fn wait_for(&mut self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok(l) = self.lines.recv_timeout(Duration::from_millis(200)) {
                let hit = l.contains(needle);
                self.seen.push(l);
                if hit {
                    return;
                }
            }
        }
        panic!("never saw {needle:?}; log so far: {:?}", self.seen);
    }

    /// Waits for the process to exit by itself and returns its status; the log is drained into `seen`.
    fn wait_exit(&mut self, within: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + within;
        loop {
            while let Ok(l) = self.lines.try_recv() {
                self.seen.push(l);
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                std::thread::sleep(Duration::from_millis(100));
                while let Ok(l) = self.lines.try_recv() {
                    self.seen.push(l);
                }
                return status;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("the server did not stop within {within:?}; log: {:?}", self.seen);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_redirected_log_is_plain_text() {
    let mut s = spawn(|_| {});
    // Give a few more lines a chance to arrive, then stop it the hard way: this test is about the bytes.
    std::thread::sleep(Duration::from_millis(300));
    let _ = s.child.kill();
    s.wait_exit(Duration::from_secs(10));
    let log = s.seen.join("");
    assert!(log.contains("listening"), "{log:?}");
    assert!(!log.contains('\u{1b}'), "terminal escape sequences in a redirected log: {log:?}");
}

#[test]
fn no_color_is_honoured() {
    let mut s = spawn(|c| {
        c.env("NO_COLOR", "1");
    });
    let _ = s.child.kill();
    s.wait_exit(Duration::from_secs(10));
    assert!(!s.seen.join("").contains('\u{1b}'));
}

#[cfg(unix)]
fn signal(child: &Child, name: &str) {
    let status = Command::new("kill").args([&format!("-{name}"), &child.id().to_string()]).status().unwrap();
    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn sigterm_and_sigint_stop_the_server_gracefully() {
    for name in ["TERM", "INT"] {
        let mut s = spawn(|_| {});
        signal(&s.child, name);
        let status = s.wait_exit(Duration::from_secs(20));
        assert!(status.success(), "SIG{name}: {status:?}; log: {:?}", s.seen);
        assert!(s.seen.join("").contains("shutting down"), "SIG{name}: {:?}", s.seen);
    }
}

/// Sends a console control event to a process that owns its own console, from a helper PowerShell process
/// (the only way to call `GenerateConsoleCtrlEvent` here: the workspace forbids `unsafe`). The helper detaches
/// from its own console, attaches to the target's, ignores the event itself and raises it.
#[cfg(windows)]
fn send_console_event(pid: u32, event: u32, group: u32) {
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
Add-Type -Namespace AgTest -Name K -MemberDefinition @'
[DllImport("kernel32.dll", SetLastError=true)] public static extern bool FreeConsole();
[DllImport("kernel32.dll", SetLastError=true)] public static extern bool AttachConsole(uint pid);
[DllImport("kernel32.dll", SetLastError=true)] public static extern bool SetConsoleCtrlHandler(IntPtr h, bool add);
[DllImport("kernel32.dll", SetLastError=true)] public static extern bool GenerateConsoleCtrlEvent(uint ev, uint group);
'@
[void][AgTest.K]::FreeConsole()
if (-not [AgTest.K]::AttachConsole({pid})) {{ Write-Error ("AttachConsole failed: " + [Runtime.InteropServices.Marshal]::GetLastWin32Error()); exit 3 }}
[void][AgTest.K]::SetConsoleCtrlHandler([IntPtr]::Zero, $true)
if (-not [AgTest.K]::GenerateConsoleCtrlEvent({event}, {group})) {{ Write-Error ("GenerateConsoleCtrlEvent failed: " + [Runtime.InteropServices.Marshal]::GetLastWin32Error()); exit 4 }}
Start-Sleep -Milliseconds 500
"#
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "could not send console event {event}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(windows)]
#[test]
fn ctrl_c_stops_a_console_server_gracefully() {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    // Its own console, and not its own process group (a new group would make it ignore Ctrl+C).
    let mut s = spawn(|c| {
        c.creation_flags(CREATE_NEW_CONSOLE);
    });
    let pid = s.child.id();
    send_console_event(pid, 0, 0); // CTRL_C_EVENT to every process on that console
    let status = s.wait_exit(Duration::from_secs(20));
    assert!(status.success(), "{status:?}; log: {:?}", s.seen);
    assert!(s.seen.join("").contains("shutting down"), "{:?}", s.seen);
}

#[cfg(windows)]
#[test]
fn ctrl_break_stops_a_console_server_gracefully() {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    let mut s = spawn(|c| {
        c.creation_flags(CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP);
    });
    let pid = s.child.id();
    send_console_event(pid, 1, pid); // CTRL_BREAK_EVENT to the child's process group
    let status = s.wait_exit(Duration::from_secs(20));
    assert!(status.success(), "{status:?}; log: {:?}", s.seen);
    assert!(s.seen.join("").contains("shutting down"), "{:?}", s.seen);
}
