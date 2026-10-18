//! `ag git import|export` run from inside a git hook inherit `GIT_DIR`, `GIT_WORK_TREE` and `GIT_INDEX_FILE`; they
//! win over `git -C <dir>`, so without care the bridge would read or write the hook's repository instead of the one
//! it was given.
#![allow(missing_docs)]

mod common;

use std::process::Command;

use common::Ag;

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=T", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("git");
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

#[test]
fn a_hook_environment_does_not_redirect_the_bridge_to_another_repository() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("SKIP: git is not installed");
        return;
    }
    let scratch = tempfile::tempdir().unwrap();
    // The repository whose hook is "running": the environment points at it.
    let hook_repo = scratch.path().join("hook-repo");
    std::fs::create_dir_all(&hook_repo).unwrap();
    git(&hook_repo, &["init", "-q", "-b", "elsewhere"]);
    let hook_git_dir = hook_repo.join(".git");
    let hook_index = hook_git_dir.join("index");
    let env = [
        ("GIT_DIR", hook_git_dir.to_str().unwrap()),
        ("GIT_WORK_TREE", hook_repo.to_str().unwrap()),
        ("GIT_INDEX_FILE", hook_index.to_str().unwrap()),
    ];

    // The repository the user asked to import.
    let src = scratch.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    std::fs::write(src.join("a.txt"), "from src\n").unwrap();
    git(&src, &["add", "."]);
    git(&src, &["commit", "-q", "-m", "the source commit"]);

    let a = Ag::new();
    let out =
        a.run_env("user:alice", &["--json", "git", "import", src.to_str().unwrap(), "--stream", "imported"], &env).ok();
    assert_eq!(out.json()["commits"], 1, "the commit of the repository that was named: {}", out.stdout());

    // Export into a directory that is not a repository yet: it must become one, with the history in it.
    let dest = scratch.path().join("dest");
    let out = a
        .run_env(
            "user:alice",
            &["--json", "git", "export", "imported", dest.to_str().unwrap(), "--branch", "trunk"],
            &env,
        )
        .ok();
    assert_eq!(out.json()["commits"], 1);
    assert_eq!(git(&dest, &["log", "--format=%s", "trunk"]).lines().count(), 1);
    assert!(git(&dest, &["show", "trunk:a.txt"]).contains("from src"));

    // Nothing was written to the repository the environment pointed at.
    assert_eq!(
        git(&hook_repo, &["for-each-ref", "--format=%(refname)"]),
        "",
        "no refs were created in the hook's repository"
    );
    assert_eq!(git(&hook_repo, &["rev-list", "--all", "--count"]), "0");
}
