//! On a platform without the sandbox the worker drives, the binary is a clear, tested refusal — not a
//! build failure and not a silent degradation. (On Unix the same binary is exercised by `e2e.rs`.)
#![cfg(not(unix))]
#![allow(missing_docs)]

use std::process::Command;

fn run(args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_ag-worker")).args(args).output().expect("run ag-worker");
    (out.status.code(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn every_invocation_says_it_is_unsupported_and_exits_2() {
    for args in [&[][..], &["doctor"][..], &["run", "--help"][..], &["--version"][..]] {
        let (code, stderr) = run(args);
        assert_eq!(code, Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains("not supported on this platform"), "{args:?}: {stderr}");
        assert!(stderr.contains("Linux"), "the message must say where it does work: {stderr}");
    }
}

#[test]
fn the_message_points_at_the_documentation() {
    assert!(ag_worker::UNSUPPORTED_PLATFORM.contains("docs/operations/windows.md"));
}
