//! Runs `examples/quickstart.sh` (the demo shown in the README) against the freshly built binary — and, on
//! Windows, its PowerShell twin `examples/quickstart.ps1`.
#![allow(missing_docs)]

use std::path::PathBuf;
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn the_readme_demo_script_passes() {
    // On Windows `bash` may be a stub (WSL) and `python3` the Store alias: the PowerShell script covers it.
    if cfg!(windows) {
        eprintln!("skipping on Windows: see the_powershell_demo_script_passes");
        return;
    }
    if !have("bash") || !have("python3") {
        eprintln!("skipping: bash and python3 are required");
        return;
    }
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/quickstart.sh");
    let out = Command::new("bash").arg(&script).env("AG", env!("CARGO_BIN_EXE_ag")).output().expect("run bash");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "quickstart failed\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}");
    assert!(stdout.contains("Demo finished"), "{stdout}");
}

/// The same demo in PowerShell. PowerShell 7 (`pwsh`) if installed, else the Windows PowerShell 5.1 that ships with
/// every Windows; CI runs the script under both.
#[cfg(windows)]
#[test]
fn the_powershell_demo_script_passes() {
    let usable = |shell: &str| {
        Command::new(shell)
            .args(["-NoProfile", "-NonInteractive", "-Command", "exit 0"])
            .output()
            .is_ok_and(|o| o.status.success())
    };
    let shell = ["pwsh", "powershell"].into_iter().find(|s| usable(s)).expect("no PowerShell found");
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/quickstart.ps1");
    let out = Command::new(shell)
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .env("AG", env!("CARGO_BIN_EXE_ag"))
        .output()
        .expect("run PowerShell");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "quickstart.ps1 failed under {shell}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(stdout.contains("Demo finished"), "{stdout}");
}
