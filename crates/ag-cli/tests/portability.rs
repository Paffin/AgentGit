//! Portability end to end, through the real binary: what a repository holds that Windows or macOS cannot write,
//! how `ag` refuses it, and that it leaves nothing behind in the user's profile.
//!
//! The hostile repository is built with Git *plumbing* (tree objects), because most of its paths cannot be
//! created as files on the machine that runs the Windows leg of this test. See docs/operations/windows.md.
#![allow(missing_docs)]

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn have_git() -> bool {
    Command::new("git").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn git(dir: &Path, args: &[&str], stdin: Option<&str>) -> String {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=T", "-c", "user.email=t@example.com", "-c", "core.autocrlf=false"])
        .args(args)
        .env("GIT_AUTHOR_DATE", "1700000000 +0000")
        .env("GIT_COMMITTER_DATE", "1700000000 +0000")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(s) = stdin {
        child.stdin.as_mut().unwrap().write_all(s.as_bytes()).unwrap();
    }
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A Git repository whose only commit holds paths that are fine on Linux and not on Windows or macOS.
fn hostile_git_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"], None);
    let blob = git(dir, &["hash-object", "-w", "--stdin"], Some("data"));
    let sub = git(dir, &["mktree"], Some(&format!("100644 blob {blob}\tmod.rs\n100644 blob {blob}\tMod.rs\n")));
    let root = git(
        dir,
        &["mktree"],
        Some(&format!(
            "040000 tree {sub}\tsrc\n100644 blob {blob}\taux.c\n100644 blob {blob}\tNUL\n100644 blob {blob}\ta:b\n\
             100644 blob {blob}\tREADME\n100644 blob {blob}\treadme\n100644 blob {blob}\tfine.txt\n"
        )),
    );
    let commit = git(dir, &["commit-tree", &root, "-m", "hostile"], None);
    git(dir, &["update-ref", "refs/heads/main", &commit], None);
}

fn ag(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ag"))
        .current_dir(dir)
        .env("AG_IDENTITY", "user:alice")
        .env_remove("AG_KEY_DIR")
        .args(args)
        .output()
        .expect("run ag")
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn empty(dir: &Path) -> bool {
    fs::read_dir(dir).unwrap().next().is_none()
}

#[test]
fn portability_lists_what_each_platform_cannot_hold_and_checkout_enforces_it_on_this_one() {
    if ag_testkit::require_here("git", "git is not installed", have_git().then_some(())).is_none() {
        return;
    }
    let src = tempfile::tempdir().unwrap();
    hostile_git_repo(src.path());
    let work = tempfile::tempdir().unwrap();
    assert!(ag(work.path(), &["init", "--namespace", "acme/p"]).status.success());
    let imp = ag(work.path(), &["git", "import", src.path().to_str().unwrap()]);
    assert!(imp.status.success(), "{}", text(&imp));

    // Windows: device names, ':' and the case pairs.
    let o = ag(work.path(), &["portability", "imported", "--target", "windows"]);
    assert_eq!(o.status.code(), Some(4), "{}", text(&o));
    let t = text(&o);
    for needle in ["aux.c", "NUL", "a:b", "README and readme", "mod.rs", "Windows"] {
        assert!(t.contains(needle), "{needle}: {t}");
    }
    // macOS: only the case pairs.
    let o = ag(work.path(), &["portability", "imported", "--target", "macos"]);
    assert_eq!(o.status.code(), Some(4), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("README and readme") && t.contains("src/Mod.rs and src/mod.rs"), "{t}");
    assert!(!t.contains("aux.c") && !t.contains("a:b"), "{t}");
    // Linux holds all of it.
    let o = ag(work.path(), &["portability", "imported", "--target", "linux"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("can be checked out on Linux"), "{}", text(&o));

    // The machine-readable form carries the same facts.
    let o = ag(work.path(), &["--json", "portability", "imported", "--target", "windows"]);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["platform"], "Windows");
    assert_eq!(v["paths"], 8);
    assert!(v["hazards"].as_array().unwrap().iter().any(|h| h["path"] == "aux.c"));
    assert!(v["collisions"].as_array().unwrap().iter().any(|c| c == &serde_json::json!(["README", "readme"])));

    // A name nobody knows is a usage error, not a pass.
    let o = ag(work.path(), &["portability", "imported", "--target", "plan9"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o).contains("unknown platform"), "{}", text(&o));

    // `ag checkout` enforces the rules of the machine it runs on, before writing anything.
    let dest = work.path().join("out");
    let o = ag(work.path(), &["checkout", "imported", dest.to_str().unwrap()]);
    if cfg!(windows) {
        assert_eq!(o.status.code(), Some(1), "{}", text(&o));
        assert!(text(&o).contains("cannot be checked out on a Windows file system"), "{}", text(&o));
        assert!(text(&o).contains("Nothing was written"), "{}", text(&o));
        assert!(empty(&dest), "a refused checkout writes nothing");
    } else if cfg!(target_os = "macos") {
        assert_eq!(o.status.code(), Some(1), "{}", text(&o));
        assert!(text(&o).contains("README and readme"), "{}", text(&o));
        assert!(empty(&dest));
    } else {
        assert!(o.status.success(), "{}", text(&o));
        assert_eq!(fs::read_to_string(dest.join("aux.c")).unwrap(), "data");
        assert_eq!(fs::read_to_string(dest.join("README")).unwrap(), "data");
        assert_eq!(fs::read_to_string(dest.join("readme")).unwrap(), "data");
        assert!(!text(&o).contains("note:"), "nothing is adapted on Linux: {}", text(&o));
    }
}

#[test]
fn a_clean_repository_is_clean_everywhere() {
    let work = tempfile::tempdir().unwrap();
    assert!(ag(work.path(), &["init", "--namespace", "acme/p"]).status.success());
    fs::create_dir_all(work.path().join("src")).unwrap();
    fs::write(work.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(work.path().join("README.md"), "# hi\n").unwrap();
    assert!(ag(work.path(), &["snapshot", "-m", "one"]).status.success());
    for target in ["windows", "macos", "linux"] {
        let o = ag(work.path(), &["portability", "--target", target]);
        assert_eq!(o.status.code(), Some(0), "{target}: {}", text(&o));
    }
}

#[test]
fn the_login_name_becomes_the_default_identity_whatever_the_platform_calls_it() {
    let work = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ag"))
            .current_dir(work.path())
            .env_remove("AG_IDENTITY")
            .env_remove("USER")
            .env_remove("LOGNAME")
            .env("USERNAME", "John Smith") // what Windows sets
            .args(args)
            .output()
            .unwrap()
    };
    assert!(run(&["init", "--namespace", "acme/p"]).status.success());
    fs::write(work.path().join("f.txt"), "x").unwrap();
    assert!(run(&["snapshot", "-m", "first"]).status.success());
    let o = run(&["--json", "log"]);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v[0]["author"], "user:John_Smith", "{}", text(&o));
}

#[test]
fn the_cli_keeps_everything_in_the_repository_and_leaves_the_users_profile_alone() {
    // Home, profile, roaming and local application data, XDG directories and the temp directory all point
    // at empty directories; a normal session must leave every one of them empty.
    let work = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    let names = ["home", "userprofile", "appdata", "localappdata", "xdg_config", "xdg_data", "xdg_cache", "tmp"];
    for n in names {
        fs::create_dir(profile.path().join(n)).unwrap();
    }
    let p = |n: &str| profile.path().join(n);
    let run = |args: &[&str]| {
        let o = Command::new(env!("CARGO_BIN_EXE_ag"))
            .current_dir(work.path())
            .env("AG_IDENTITY", "user:alice")
            .env("HOME", p("home"))
            .env("USERPROFILE", p("userprofile"))
            .env("APPDATA", p("appdata"))
            .env("LOCALAPPDATA", p("localappdata"))
            .env("XDG_CONFIG_HOME", p("xdg_config"))
            .env("XDG_DATA_HOME", p("xdg_data"))
            .env("XDG_CACHE_HOME", p("xdg_cache"))
            .env("TMPDIR", p("tmp"))
            .env("TEMP", p("tmp"))
            .env("TMP", p("tmp"))
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "ag {args:?}: {}", text(&o));
    };
    run(&["init", "--namespace", "acme/p"]);
    fs::write(work.path().join("a.txt"), "a").unwrap();
    run(&["snapshot", "-m", "one"]);
    run(&["status"]);
    run(&["log"]);
    run(&["agent", "register", "bot"]);
    run(&["workspace", "create", "w", "--dir", work.path().join("ws").to_str().unwrap()]);
    run(&["verify"]);
    run(&["gc"]);
    run(&["backup", work.path().join("bak").to_str().unwrap()]);
    for n in names {
        assert!(empty(&p(n)), "{n} must stay empty, found {:?}", fs::read_dir(p(n)).unwrap().collect::<Vec<_>>());
    }
}
