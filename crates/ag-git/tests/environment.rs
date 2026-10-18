//! The bridge reads what git prints, so what git prints must not depend on the machine it runs on: the user's
//! git configuration (`log.showSignature`, `i18n.logOutputEncoding`), the bytes of a commit (separators,
//! encodings) and the environment it inherits. Each test fails on the code as it was before the fix, and uses
//! real `git`.
#![allow(missing_docs)]

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use ag_git::{ExportOptions, GitError, ImportOptions, export, import};
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

/// A repository with one commit that has a file.
fn source() -> tempfile::TempDir {
    let src = tempfile::tempdir().unwrap();
    g(src.path(), &["init", "-q", "-b", "main"]);
    fs::write(src.path().join("f.txt"), "one\n").unwrap();
    g(src.path(), &["add", "."]);
    g(src.path(), &["commit", "-q", "-m", "first"]);
    src
}

/// A commit on top of `HEAD` with exactly these header lines and message bytes, bypassing `git commit`.
fn raw_commit(dir: &Path, headers: &str, message: &[u8]) -> String {
    let tree = g(dir, &["rev-parse", "HEAD^{tree}"]);
    let parent = g(dir, &["rev-parse", "HEAD"]);
    let mut body = format!("tree {tree}\nparent {parent}\n{headers}\n").into_bytes();
    body.extend_from_slice(message);
    body.push(b'\n');
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["hash-object", "-t", "commit", "-w", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&body).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    g(dir, &["update-ref", "refs/heads/main", &sha]);
    sha
}

const AUTHOR: &str = "author A <a@example.com> 1700000000 +0000\ncommitter A <a@example.com> 1700000000 +0000\n";

/// The messages of the imported history, newest first.
fn messages(repo: &Repository, head: &ag_types::ChangesetId) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = Some(*head);
    while let Some(id) = at {
        let cs = repo.changeset(&id).unwrap();
        out.push(cs.message.clone());
        at = cs.parents.first().copied();
    }
    out
}

#[cfg(unix)]
#[test]
fn the_users_git_configuration_does_not_change_what_is_imported() {
    use std::os::unix::fs::PermissionsExt;

    let src = source();
    // A signed-looking commit and a message with characters outside ASCII ...
    raw_commit(
        src.path(),
        &format!("{AUTHOR}gpgsig -----BEGIN PGP SIGNATURE-----\n \n abc\n -----END PGP SIGNATURE-----\n"),
        "signed Café".as_bytes(),
    );
    // ... read by a user whose git verifies and shows signatures with a gpg that talks, and prints in Latin-1.
    let tools = tempfile::tempdir().unwrap();
    let gpg = tools.path().join("noisy-gpg");
    fs::write(&gpg, "#!/bin/sh\necho 'NOISE FROM GPG' >&2\nexit 1\n").unwrap();
    fs::set_permissions(&gpg, fs::Permissions::from_mode(0o755)).unwrap();
    g(src.path(), &["config", "gpg.program", gpg.to_str().unwrap()]);
    g(src.path(), &["config", "log.showSignature", "true"]);
    g(src.path(), &["config", "i18n.logOutputEncoding", "ISO-8859-1"]);
    // (the premise: this configuration really does change what a plain `git log` prints)
    let plain = g(src.path(), &["log", "--format=%H%n%B"]);
    assert!(plain.contains("NOISE FROM GPG") || plain.contains("No signature"), "{plain}");

    let ag_dir = tempfile::tempdir().unwrap();
    let repo = ag_repo(ag_dir.path());
    let report = import(&repo, &import_opts(src.path())).unwrap();
    assert_eq!(report.commits, 2);
    assert_eq!(report.lossy_commits, 0);
    let msgs = messages(&repo, &report.head);
    assert!(msgs[0].starts_with("signed Café\n\nImported-From: git "), "{:?}", msgs[0]);
    assert!(!msgs[0].contains("NOISE") && !msgs[0].contains("No signature"), "{:?}", msgs[0]);
    assert!(msgs[1].starts_with("first\n\nImported-From: git "), "{:?}", msgs[1]);
}

#[test]
fn a_message_may_contain_any_character_except_nul() {
    // U+001E and U+001F were the importer's own separators.
    let src = source();
    raw_commit(src.path(), AUTHOR, "a\u{1e}b\u{1f}c\u{1e}\u{1e}\n\nend".as_bytes());
    let ag_dir = tempfile::tempdir().unwrap();
    let repo = ag_repo(ag_dir.path());
    let report = import(&repo, &import_opts(src.path())).unwrap();
    assert_eq!(report.commits, 2);
    assert!(
        messages(&repo, &report.head)[0].starts_with("a\u{1e}b\u{1f}c\u{1e}\u{1e}\n\nend\n\nImported-From: git "),
        "{:?}",
        messages(&repo, &report.head)[0]
    );
}

#[test]
fn text_that_is_not_utf8_is_repaired_and_the_report_says_so() {
    let src = source();
    // No `encoding` header: git takes the bytes for UTF-8, and they are not.
    let mut message = b"caf".to_vec();
    message.push(0xe9);
    raw_commit(src.path(), AUTHOR, &message);
    let ag_dir = tempfile::tempdir().unwrap();
    let repo = ag_repo(ag_dir.path());
    let report = import(&repo, &import_opts(src.path())).unwrap();
    assert_eq!(report.lossy_commits, 1, "one commit was repaired");
    assert!(messages(&repo, &report.head)[0].starts_with("caf\u{fffd}\n\nImported-From"));

    // With the encoding named, git converts the commit and nothing is lost.
    let src = source();
    let mut message = b"caf".to_vec();
    message.push(0xe9);
    raw_commit(src.path(), &format!("{AUTHOR}encoding ISO-8859-1\n"), &message);
    let ag_dir = tempfile::tempdir().unwrap();
    let repo = ag_repo(ag_dir.path());
    let report = import(&repo, &import_opts(src.path())).unwrap();
    assert_eq!(report.lossy_commits, 0);
    assert!(
        messages(&repo, &report.head)[0].starts_with("café\n\nImported-From"),
        "{:?}",
        messages(&repo, &report.head)
    );
}

#[test]
fn the_log_parser_rejects_what_is_not_whole_records() {
    use ag_git::fuzzing::parse_log;
    assert!(parse_log(b"").unwrap().is_empty());
    let record = b"abc\0\0A\0a@example.com\x001700000000\0message\n\0";
    let one = parse_log(record).unwrap();
    assert_eq!(
        (one.len(), one[0].sha.as_str(), one[0].parents.len(), one[0].message.as_str()),
        (1, "abc", 0, "message")
    );
    let two = parse_log(&[&record[..], &record[..]].concat()).unwrap();
    assert_eq!(two.len(), 2);
    // a record cut short, a record that is not terminated, a message with a NUL in it: errors, never a guess
    assert!(parse_log(&record[..record.len() - 4]).is_err());
    assert!(parse_log(&record[..record.len() - 1]).is_err());
    assert!(parse_log(b"abc\0\0A\0a@example.com\x001700000000\0mess\0age\0").is_err());
    assert!(parse_log(b"abc\0\0A\0a@example.com\0notatime\0message\0").is_err());
}

#[cfg(unix)]
#[test]
fn when_cat_file_dies_the_error_is_the_reason_it_gave() {
    let src = source();
    // Cut the end off the one file's loose object: git can name the blob and its size, then fails to stream it,
    // and says so on standard error before it exits.
    let sha = g(src.path(), &["rev-parse", "HEAD:f.txt"]);
    let object = src.path().join(".git/objects").join(&sha[..2]).join(&sha[2..]);
    let whole = fs::read(&object).unwrap();
    fs::remove_file(&object).unwrap();
    fs::write(&object, &whole[..whole.len() - 4]).unwrap();

    let ag_dir = tempfile::tempdir().unwrap();
    let repo = ag_repo(ag_dir.path());
    let err = import(&repo, &import_opts(src.path())).expect_err("a corrupt object cannot be imported");
    match &err {
        GitError::Failed { args, stderr } => {
            assert!(args.contains("cat-file"), "{err}");
            assert!(!stderr.trim().is_empty(), "git's own message is passed on: {err}");
        }
        other => panic!("expected git's own message, not a guess about its answer: {other}"),
    }
}

#[test]
fn a_branch_name_is_judged_by_git_and_before_anything_is_created() {
    // Names git refuses; each used to be accepted here, to fail later inside fast-import ...
    let src = source();
    let ag_dir = tempfile::tempdir().unwrap();
    let repo = ag_repo(ag_dir.path());
    import(&repo, &import_opts(src.path())).unwrap();
    let out = tempfile::tempdir().unwrap();
    for bad in [
        "x.lock", "a/", "/a", ".hidden", "a/.b", "a@{b}", "a//b", "a\u{1}b", "a\tb", "a b", "a..b", "a~1", "a^", "a:b",
        "a?b", "a*b", "a[b", "a\\b", "", "-x", "--force",
    ] {
        let dest = out.path().join("never-created");
        let err =
            export(&repo, &ExportOptions { stream: "imported".into(), git_dir: dest.clone(), branch: bad.into() })
                .expect_err(bad);
        assert!(matches!(&err, GitError::Unsupported(m) if m.contains("not a usable branch name")), "{bad:?}: {err}");
        // ... and a refused export leaves nothing behind.
        assert!(!dest.exists(), "{bad:?} created {}", dest.display());
    }
    // Names git accepts, including ones the old rules did not think of.
    for (i, good) in ["main", "feature/x", "release-1.0", "naïve", "a@b", "v1.2.3"].iter().enumerate() {
        let dest = out.path().join(format!("ok{i}"));
        export(&repo, &ExportOptions { stream: "imported".into(), git_dir: dest.clone(), branch: (*good).into() })
            .unwrap_or_else(|e| panic!("{good:?}: {e}"));
        assert_eq!(g(&dest, &["rev-parse", "--abbrev-ref", "HEAD"]), *good);
    }
}
