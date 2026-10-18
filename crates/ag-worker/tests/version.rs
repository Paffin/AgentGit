//! `ag-worker --version` prints semver, commit and date (the build script itself is exercised in
//! `crates/ag-cli/tests/version.rs`).
//!
//! Unix only, on purpose: where the sandbox does not exist the binary is a stub that refuses *every* invocation with
//! status 2 (`--version` included; `tests/unsupported.rs` pins that), so there is no version line to read.
#![cfg(unix)]
#![allow(missing_docs)]

#[test]
fn prints_semver_commit_and_date() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_ag-worker")).arg("--version").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    let line = text.trim_end();
    let rest = line.strip_prefix("ag-worker ").unwrap_or_else(|| panic!("unexpected: {line}"));
    assert!(rest.starts_with(env!("CARGO_PKG_VERSION")), "{line}");
    assert!(rest.ends_with(')') && rest.contains(" ("), "no build metadata in: {line}");
}
