//! `--version` carries the commit and its date, and the build script that produces them degrades
//! deterministically when git is not there (a crates.io tarball, a container build context).
#![allow(missing_docs)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `ag 0.1.0 (a1b2c3d4e5f6 2026-10-17)` or `ag 0.1.0 (no git metadata)`.
#[track_caller]
fn assert_version_line(line: &str, name: &str) {
    let rest =
        line.strip_prefix(&format!("{name} ")).unwrap_or_else(|| panic!("`{line}` does not start with `{name} `"));
    let (semver, meta) = rest.split_once(' ').unwrap_or_else(|| panic!("no build metadata in `{line}`"));
    assert_eq!(semver, env!("CARGO_PKG_VERSION"), "{line}");
    let meta = meta.strip_prefix('(').and_then(|m| m.strip_suffix(')')).unwrap_or_else(|| panic!("`{line}`"));
    if meta == "no git metadata" {
        return;
    }
    let mut parts = meta.split(' ');
    let commit = parts.next().unwrap();
    assert!(commit.len() >= 7 && commit.bytes().all(|b| b.is_ascii_hexdigit()), "bad commit in `{line}`");
    if let Some(date) = parts.next() {
        let d: Vec<&str> = date.split('-').collect();
        assert!(
            d.len() == 3
                && d[0].len() == 4
                && d[1].len() == 2
                && d[2].len() == 2
                && d.concat().bytes().all(|b| b.is_ascii_digit()),
            "bad date in `{line}`"
        );
    }
    assert!(parts.next().is_none(), "{line}");
}

fn git_head() -> Option<String> {
    let out = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(manifest_dir()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[test]
fn ag_prints_semver_commit_and_date() {
    let out = Command::new(env!("CARGO_BIN_EXE_ag")).arg("--version").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    assert_version_line(text.trim_end(), "ag");
    // In a checkout of this repository the commit is the one that is checked out.
    if let Some(head) = git_head()
        && manifest_dir().ancestors().nth(2).is_some_and(|root| root.join(".git").exists())
    {
        assert!(text.contains(&head[..12]), "`{text}` does not mention HEAD {head}");
    }
}

/// Compiles `build.rs` and runs it as cargo would, with a hand-made environment; returns its `cargo:` lines.
fn run_build_script(env: &[(&str, &str)], manifest: &Path, path_dirs: &str) -> Vec<String> {
    let tmp = tempfile::tempdir().unwrap();
    let exe = tmp.path().join("build_script");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let status = Command::new(rustc)
        .args(["--edition", "2024", "--crate-name", "build_script", "-o"])
        .arg(&exe)
        .arg(manifest_dir().join("build.rs"))
        .status()
        .expect("rustc is needed to test the build script");
    assert!(status.success());
    let out = Command::new(&exe)
        .env_clear()
        .env("PATH", path_dirs)
        .env("CARGO_PKG_VERSION", "1.2.3")
        .env("CARGO_PKG_NAME", "agentgit-cli")
        .env("CARGO_MANIFEST_DIR", manifest)
        .current_dir(manifest)
        .envs(env.iter().copied())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().lines().map(str::to_string).collect()
}

fn long_version(lines: &[String]) -> &str {
    lines.iter().find_map(|l| l.strip_prefix("cargo:rustc-env=AG_LONG_VERSION=")).expect("AG_LONG_VERSION is set")
}

#[test]
fn without_git_the_version_falls_back_deterministically() {
    let empty = tempfile::tempdir().unwrap(); // a PATH with no `git` in it, and a manifest dir outside any repository
    let a = run_build_script(&[], empty.path(), empty.path().to_str().unwrap());
    assert_eq!(long_version(&a), "1.2.3 (no git metadata)");
    assert!(a.contains(&"cargo:rustc-env=AG_GIT_COMMIT=unknown".to_string()), "{a:?}");
    assert!(a.contains(&"cargo:rustc-env=AG_BUILD_DATE=unknown".to_string()), "{a:?}");
    // Same inputs, same output.
    assert_eq!(a, run_build_script(&[], empty.path(), empty.path().to_str().unwrap()));
}

#[test]
fn environment_overrides_win_and_are_validated() {
    let empty = tempfile::tempdir().unwrap();
    let p = empty.path().to_str().unwrap();
    let run = |env: &[(&str, &str)]| long_version(&run_build_script(env, empty.path(), p)).to_string();
    // A container build passes the commit and date as build arguments.
    assert_eq!(
        run(&[("AG_BUILD_COMMIT", "ABCDEF0123456789ABCDEF0123456789ABCDEF01"), ("AG_BUILD_DATE", "2026-01-02")]),
        "1.2.3 (abcdef012345 2026-01-02)"
    );
    // SOURCE_DATE_EPOCH is honoured (reproducible builds): 1700000000 is 2023-11-14.
    assert_eq!(
        run(&[("AG_BUILD_COMMIT", "abcdef0"), ("SOURCE_DATE_EPOCH", "1700000000")]),
        "1.2.3 (abcdef0 2023-11-14)"
    );
    // The epoch itself and a leap day convert correctly.
    assert_eq!(run(&[("AG_BUILD_COMMIT", "abcdef0"), ("SOURCE_DATE_EPOCH", "0")]), "1.2.3 (abcdef0 1970-01-01)");
    assert_eq!(
        run(&[("AG_BUILD_COMMIT", "abcdef0"), ("SOURCE_DATE_EPOCH", "1709164800")]),
        "1.2.3 (abcdef0 2024-02-29)"
    );
    // Garbage is ignored rather than printed.
    assert_eq!(run(&[("AG_BUILD_COMMIT", "not-a-sha"), ("AG_BUILD_DATE", "yesterday")]), "1.2.3 (no git metadata)");
    // A commit alone is fine.
    assert_eq!(run(&[("AG_BUILD_COMMIT", "abcdef0")]), "1.2.3 (abcdef0)");
}

#[test]
fn a_packaged_crate_reports_the_commit_cargo_recorded() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".cargo_vcs_info.json"),
        r#"{"git":{"sha1":"0123456789abcdef0123456789abcdef01234567"},"path_in_vcs":"crates/ag-cli"}"#,
    )
    .unwrap();
    let lines = run_build_script(&[], dir.path(), dir.path().to_str().unwrap());
    assert_eq!(long_version(&lines), "1.2.3 (0123456789ab)");
}

#[test]
fn the_three_binaries_share_one_build_script() {
    let mine = std::fs::read_to_string(manifest_dir().join("build.rs")).unwrap();
    for sibling in ["ag-server", "ag-worker"] {
        // Not present when this crate is built from its crates.io tarball.
        let Ok(theirs) = std::fs::read_to_string(manifest_dir().join("..").join(sibling).join("build.rs")) else {
            continue;
        };
        assert_eq!(mine, theirs, "crates/{sibling}/build.rs differs from crates/ag-cli/build.rs; copy it over");
    }
}
