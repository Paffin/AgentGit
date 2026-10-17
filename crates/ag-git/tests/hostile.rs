//! Regression tests for three bugs in the Git bridge, found while writing the `git_parse` fuzz target in `fuzz/`
//! (which now covers them) and reproduced against real `git`. Each test fails on the code as it was before the
//! fix. Require a `git` binary on PATH, as does the bridge itself.
#![allow(missing_docs)]

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use ag_git::{ExportOptions, ImportOptions, export, fuzzing, import};
use ag_integrate::{InitOptions, Repository};
use ag_types::{Identity, Namespace, StreamName};

fn g(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test User", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_AUTHOR_DATE", "1700000000 +0000")
        .env("GIT_COMMITTER_DATE", "1700000000 +0000")
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn ag_repo(dir: &Path) -> Repository {
    Repository::init(
        dir,
        &InitOptions {
            namespace: Namespace::new("acme/imported").unwrap(),
            default_stream: StreamName::new("main").unwrap(),
            actor: Identity::new("user:importer").unwrap(),
            storage: None,
        },
    )
    .unwrap()
}

fn import_opts(git_dir: &Path) -> ImportOptions {
    ImportOptions {
        git_dir: git_dir.to_path_buf(),
        rev: "HEAD".into(),
        stream: StreamName::new("imported").unwrap(),
        actor: Identity::new("user:importer").unwrap(),
    }
}

/// A commit object with exactly the given author line, bypassing the checks of `git commit`.
fn raw_commit(dir: &Path, author_line: &str, message: &str) -> String {
    let tree = g(dir, &["hash-object", "-t", "tree", "-w", "/dev/null"]);
    let body = format!("tree {tree}\nauthor {author_line}\ncommitter {author_line}\n\n{message}\n");
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["hash-object", "-t", "commit", "-w", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(body.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    g(dir, &["update-ref", "refs/heads/main", &sha]);
    sha
}

#[test]
fn a_long_message_is_cut_on_a_character_boundary() {
    // Byte 60 000 falls inside a two-byte character: `String::truncate` there used to panic.
    let message = format!("{}{}", "a".repeat(59_999), "я".repeat(10));
    assert!(!message.is_char_boundary(60_000));
    let kept = fuzzing::import_message(&message, "abc123");
    assert!(kept.starts_with(&"a".repeat(59_999)));
    assert!(kept.contains("[message truncated by the git importer]"));
    assert!(kept.ends_with("\n\nImported-From: git abc123"));
    assert!(kept.len() < 60_100);

    // The same through the whole importer, from a real commit.
    let src = tempfile::tempdir().unwrap();
    g(src.path(), &["init", "-q", "-b", "main"]);
    fs::write(src.path().join("f"), "x").unwrap();
    fs::write(src.path().join("msg.txt"), &message).unwrap();
    g(src.path(), &["add", "f"]);
    g(src.path(), &["commit", "-q", "-F", "msg.txt"]);
    let ag_dir = tempfile::tempdir().unwrap();
    let repo = ag_repo(ag_dir.path());
    let report = import(&repo, &import_opts(src.path())).unwrap();
    let cs = repo.changeset(&report.head).unwrap();
    assert!(cs.message.starts_with(&"a".repeat(59_999)));
    assert!(cs.message.contains("[message truncated by the git importer]"));
}

#[test]
fn an_author_date_beyond_the_range_of_milliseconds_is_refused_not_wrapped() {
    assert_eq!(fuzzing::import_timestamp(1_700_000_000).unwrap().as_millis(), 1_700_000_000_000);
    assert_eq!(fuzzing::import_timestamp(-1).unwrap().as_millis(), -1000);
    assert!(fuzzing::import_timestamp(i64::MAX / 1000 + 1).is_err());
    assert!(fuzzing::import_timestamp(i64::MIN).is_err());

    // Git itself accepts a date of 10^16 seconds; times 1000 that overflows an i64.
    let src = tempfile::tempdir().unwrap();
    g(src.path(), &["init", "-q", "-b", "main"]);
    raw_commit(src.path(), "A <a@example.com> 9999999999999999 +0000", "far future");
    let ag_dir = tempfile::tempdir().unwrap();
    let repo = ag_repo(ag_dir.path());
    let err = import(&repo, &import_opts(src.path())).expect_err("an unrepresentable date must be an error");
    assert!(err.to_string().contains("out of range"), "{err}");
}

#[test]
fn a_path_that_starts_with_a_quote_survives_the_export() {
    // `fast-import` reads a path starting with `"` as a C-style quoted string, so `"abc"` used to come out
    // as `abc`: a different file name, silently.
    assert_eq!(fuzzing::fast_import_path("a/b.txt"), "a/b.txt");
    assert_eq!(fuzzing::fast_import_path("\"abc\""), "\"\\\"abc\\\"\"");
    assert_eq!(fuzzing::fast_import_path("\"x"), "\"\\\"x\"");

    let src = tempfile::tempdir().unwrap();
    g(src.path(), &["init", "-q", "-b", "main"]);
    for name in ["\"abc\"", "\"half", "dir/\"inner\".txt", "plain.txt"] {
        let p = src.path().join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, name).unwrap();
    }
    g(src.path(), &["add", "-A"]);
    g(src.path(), &["commit", "-q", "-m", "quotes"]);

    let ag_dir = tempfile::tempdir().unwrap();
    let repo = ag_repo(ag_dir.path());
    let report = import(&repo, &import_opts(src.path())).unwrap();
    assert!(report.skipped_paths.is_empty(), "{:?}", report.skipped_paths);

    let out = tempfile::tempdir().unwrap();
    export(
        &repo,
        &ExportOptions { stream: "imported".into(), git_dir: out.path().to_path_buf(), branch: "main".into() },
    )
    .unwrap();
    assert_eq!(
        g(out.path(), &["ls-files", "-s"]),
        g(src.path(), &["ls-files", "-s"]),
        "the exported repository must hold exactly the files that were imported"
    );
}
