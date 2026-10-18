//! The commands in the documentation, run against this build of `ag`.
//!
//! `scripts/doc_snippets.py` executes the shell blocks of the pages under `docs/` that carry doctest markers (the
//! tutorials), in a scratch directory with its own `HOME`, checks each command's exit status (`# expect-exit N`) and
//! compares the output the pages show with what the commands printed. This test runs it with the `ag` that cargo just
//! built, so `cargo test` catches a change of behaviour that a tutorial describes.
//!
//! Pages that need `ag-server` are left to the `Docs` workflow (`.github/workflows/docs.yml`), which builds the
//! server and runs every page: a `target/` directory may hold a stale `ag-server` from an earlier build, and the
//! point is to run the pages against binaries of this very commit. Pages that need `git` are skipped where `git` is not
//! installed. Needs bash and python3; skipped on Windows and where they are missing, unless `AG_REQUIRE_DOC_SNIPPETS`
//! is set (CI sets it), which turns "cannot run" into a failure.
#![allow(missing_docs)]

use std::path::PathBuf;
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn required() -> bool {
    std::env::var("AG_REQUIRE_DOC_SNIPPETS").is_ok_and(|v| !v.is_empty() && v != "0")
}

#[test]
fn the_commands_in_the_tutorials_run_and_print_what_the_pages_show() {
    if cfg!(windows) {
        assert!(!required(), "the doc-snippet harness drives bash and does not run on Windows");
        eprintln!("skipping on Windows: the harness drives bash");
        return;
    }
    if !have("bash") || !have("python3") {
        assert!(!required(), "bash and python3 are required (AG_REQUIRE_DOC_SNIPPETS is set)");
        eprintln!("skipping: bash and python3 are required");
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ag = PathBuf::from(env!("CARGO_BIN_EXE_ag"));
    let mut cmd = Command::new("python3");
    cmd.arg("-B")
        .arg(root.join("scripts/doc_snippets.py"))
        .arg("--bin-dir")
        .arg(ag.parent().expect("the ag binary has a directory"))
        .args(["--skip-needing", "ag-server"]);
    if !required() {
        cmd.args(["--optional", "git"]);
    }
    let out = cmd.output().expect("run scripts/doc_snippets.py");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "the documentation's commands do not behave as the pages say\n{stdout}\n{stderr}");
    assert!(stdout.contains("ok   "), "no page was run at all\n{stdout}");
}
