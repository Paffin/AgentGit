//! Runs `examples/quickstart.sh` (the demo shown in the README) against the freshly built binary.
#![allow(missing_docs)]

use std::path::PathBuf;
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn the_readme_demo_script_passes() {
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
