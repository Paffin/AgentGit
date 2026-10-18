//! Starting and watching a real `ag-server` process.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;

/// Settings of the server under test that scenarios vary (everything else is the server's default).
#[derive(Clone, Debug, Serialize)]
pub struct ServerSettings {
    /// `limits.requests_per_second` (0 = unlimited).
    pub requests_per_second: u32,
    /// `limits.burst`.
    pub burst: u32,
    /// `limits.max_concurrent_requests`.
    pub max_concurrent_requests: usize,
}

impl ServerSettings {
    /// No per-caller rate limit and the server's default concurrency cap: what a throughput measurement needs.
    pub fn unlimited() -> Self {
        Self { requests_per_second: 0, burst: 0, max_concurrent_requests: 512 }
    }

    fn toml(&self) -> String {
        format!(
            "[limits]\nrequests_per_second = {}\nburst = {}\nmax_concurrent_requests = {}\n",
            self.requests_per_second, self.burst, self.max_concurrent_requests
        )
    }
}

/// A running server owned by this process. Killed on drop.
pub struct Server {
    child: Option<Child>,
    /// `http://127.0.0.1:port`
    pub url: String,
    /// The bootstrap administrator credential.
    pub admin_token: String,
    /// The settings it was started with.
    pub settings: ServerSettings,
    _dir: crate::tmp::TempDir,
    log: PathBuf,
}

/// A free local port (bound and released; another process could take it, in which case startup fails loudly).
fn free_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// 128 random bits as hex, from the operating system.
pub fn random_hex() -> Result<String> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom").context("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

impl Server {
    /// Starts `bin serve` on a free port with a fresh data directory and waits until `/readyz` answers.
    pub fn start(bin: &Path, settings: ServerSettings, data_parent: Option<&Path>) -> Result<Self> {
        let dir = crate::tmp::TempDir::new(data_parent)?;
        let cfg = dir.path().join("server.toml");
        std::fs::write(&cfg, settings.toml())?;
        let port = free_port()?;
        let admin_token = format!("bench-{}", random_hex()?);
        let log = dir.path().join("server.log");
        let logf = std::fs::File::create(&log)?;
        let child = Command::new(bin)
            .args(["serve", "--config"])
            .arg(&cfg)
            .args(["--listen", &format!("127.0.0.1:{port}"), "--data-dir"])
            .arg(dir.path().join("data"))
            .env("AG_BOOTSTRAP_TOKEN", &admin_token)
            .stdin(Stdio::null())
            .stdout(logf.try_clone()?)
            .stderr(logf)
            .spawn()
            .with_context(|| format!("starting {}", bin.display()))?;
        let mut s =
            Self { child: Some(child), url: format!("http://127.0.0.1:{port}"), admin_token, settings, _dir: dir, log };
        let deadline = Instant::now() + Duration::from_secs(30);
        let probe = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(2)))
            .build()
            .into();
        let probe: ureq::Agent = probe;
        loop {
            if let Some(c) = s.child.as_mut()
                && let Some(status) = c.try_wait()?
            {
                bail!("ag-server exited during startup ({status}):\n{}", s.log_tail());
            }
            if probe.get(format!("{}/readyz", s.url)).call().is_ok_and(|r| r.status().as_u16() == 200) {
                return Ok(s);
            }
            if Instant::now() > deadline {
                bail!("ag-server was not ready within 30 s:\n{}", s.log_tail());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The last lines of the server's output.
    pub fn log_tail(&self) -> String {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(20)..].join("\n")
    }

    /// The server's process id.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Whether the process is still alive.
    pub fn alive(&mut self) -> bool {
        self.child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// CPU time (user + system, seconds) and peak resident memory (MiB) of a process, from `/proc` (None elsewhere).
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct ProcUse {
    /// Cumulative CPU seconds.
    pub cpu_secs: f64,
    /// Peak resident set size in MiB (`VmHWM`).
    pub peak_rss_mib: f64,
}

/// Reads `/proc/<pid>` (Linux).
pub fn proc_use(pid: u32) -> Option<ProcUse> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Fields after the parenthesised command name; utime and stime are the 14th and 15th fields overall.
    let rest = stat.rsplit_once(')')?.1;
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks: f64 = f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let hwm_kb: f64 = status.lines().find(|l| l.starts_with("VmHWM:"))?.split_whitespace().nth(1)?.parse().ok()?;
    // Clock ticks per second is 100 on every Linux the project supports (USER_HZ is fixed at 100 in the ABI).
    Some(ProcUse { cpu_secs: ticks / 100.0, peak_rss_mib: hwm_kb / 1024.0 })
}
