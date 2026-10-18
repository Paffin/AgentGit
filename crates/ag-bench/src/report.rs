//! What a run records about the machine and the code, and how results become Markdown.

use std::process::Command;

use serde::Serialize;

use crate::stats::fmt_ms;

/// The conditions of a run. A number without these is not a measurement.
#[derive(Clone, Debug, Serialize)]
pub struct Environment {
    /// `git rev-parse HEAD` of the tree the binary was run from (empty outside a checkout).
    pub commit: String,
    /// True if the working tree had uncommitted changes.
    pub dirty: bool,
    /// CPU model from `/proc/cpuinfo` (empty if unavailable).
    pub cpu: String,
    /// Logical CPUs available to the process.
    pub cpus: usize,
    /// Total memory in MiB from `/proc/meminfo` (0 if unavailable).
    pub mem_mib: u64,
    /// Operating system and kernel.
    pub os: String,
    /// `rustc -V`.
    pub rustc: String,
    /// False for a debug build: such numbers are not comparable.
    pub release_build: bool,
    /// `ag-bench` version.
    pub version: String,
    /// Unix time of the run.
    pub unix_time: u64,
}

fn cmd(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn proc_field(file: &str, key: &str) -> String {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_default()
}

impl Environment {
    /// Looks at the machine and the checkout.
    pub fn capture() -> Self {
        Self {
            commit: cmd("git", &["rev-parse", "HEAD"]),
            dirty: !cmd("git", &["status", "--porcelain", "--untracked-files=no"]).is_empty(),
            cpu: proc_field("/proc/cpuinfo", "model name"),
            cpus: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
            mem_mib: proc_field("/proc/meminfo", "MemTotal")
                .split_whitespace()
                .next()
                .and_then(|kb| kb.parse::<u64>().ok())
                .map(|kb| kb / 1024)
                .unwrap_or(0),
            os: format!("{} {}", cmd("uname", &["-s"]), cmd("uname", &["-r"])).trim().to_string(),
            rustc: cmd("rustc", &["-V"]),
            release_build: !cfg!(debug_assertions),
            version: env!("CARGO_PKG_VERSION").to_string(),
            unix_time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }

    /// A short paragraph for the top of a results table.
    pub fn markdown(&self) -> String {
        let commit = if self.commit.len() >= 7 { &self.commit[..7] } else { "unknown" };
        format!(
            "Commit `{commit}`{}, {}, {} logical CPUs, {} MiB RAM, {}, {}, {} build.",
            if self.dirty { " (+ uncommitted changes)" } else { "" },
            if self.cpu.is_empty() { "unknown CPU" } else { &self.cpu },
            self.cpus,
            self.mem_mib,
            self.os,
            self.rustc,
            if self.release_build { "release" } else { "DEBUG (not comparable)" },
        )
    }
}

/// `| a | b |` rows with a header and right-aligned numeric columns.
pub fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let mut out = format!("| {} |\n", header.join(" | "));
    out.push_str(&format!(
        "|{}|\n",
        header.iter().enumerate().map(|(i, _)| if i == 0 { "---" } else { "---:" }).collect::<Vec<_>>().join("|")
    ));
    for r in rows {
        out.push_str(&format!("| {} |\n", r.join(" | ")));
    }
    out
}

/// Milliseconds for a table cell.
pub fn ms(v: f64) -> String {
    fmt_ms(v)
}
