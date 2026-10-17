//! Git bridge tests. Require a `git` binary on PATH (as does the bridge itself).
#![allow(missing_docs)]

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use ag_git::{ExportOptions, ImportOptions, export, import};
use ag_integrate::{InitOptions, Repository};
use ag_types::{Identity, Namespace, StreamName};

fn g(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Test User",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .env("GIT_AUTHOR_DATE", "1700000000 +0000")
        .env("GIT_COMMITTER_DATE", "1700000000 +0000")
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write(dir: &Path, rel: &str, data: &[u8]) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, data).unwrap();
}

/// Sets the Unix mode of a file. Windows has no mode bits, so there the history simply lacks the mode-only
/// changes (the Unix run of this test covers them).
#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) {
    fs::set_permissions(p, std::os::unix::fs::PermissionsExt::from_mode(mode)).unwrap();
}
#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) {}

/// Creates a symlink; where the platform cannot (Windows, without a privilege) a plain file stands in, so the
/// history keeps its shape and the Unix run covers real symlinks.
#[cfg(unix)]
fn symlink(target: &str, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}
#[cfg(not(unix))]
fn symlink(target: &str, link: &Path) {
    fs::write(link, target).unwrap();
}

/// Stores `data` as a blob and returns its id.
fn hash_object(dir: &Path, data: &[u8]) -> String {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["hash-object", "-w", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), data).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, msg: &str) -> String {
    g(dir, &["add", "-A"]);
    g(dir, &["commit", "-q", "--allow-empty", "-m", msg]);
    g(dir, &["rev-parse", "HEAD"])
}

/// A history exercising every case the bridge claims to handle.
fn build_history(dir: &Path) -> Vec<String> {
    g(dir, &["init", "-q", "-b", "main"]);
    let mut shas = vec![];
    write(dir, "README.md", b"# project\n");
    write(dir, "src/a.txt", b"alpha\nbeta\n");
    write(dir, "bin/run.sh", b"#!/bin/sh\necho hi\n");
    set_mode(&dir.join("bin/run.sh"), 0o755);
    symlink("src/a.txt", &dir.join("link"));
    write(dir, "empty.txt", b"");
    write(dir, "blob.bin", &[0, 1, 2, 255, 254, 0, 7]);
    write(dir, "документ/файл.txt", "юникод\n".as_bytes());
    shas.push(commit(dir, "initial import\n\nwith a body"));

    write(dir, "src/a.txt", b"alpha\nbeta\ngamma\n");
    fs::remove_file(dir.join("README.md")).unwrap();
    shas.push(commit(dir, "edit and delete"));

    g(dir, &["checkout", "-q", "-b", "feature"]);
    write(dir, "feature/x.txt", b"x\n");
    shas.push(commit(dir, "feature work 1"));
    write(dir, "feature/y.txt", b"y\n");
    shas.push(commit(dir, "feature work 2"));

    g(dir, &["checkout", "-q", "main"]);
    write(dir, "main-only.txt", b"m\n");
    shas.push(commit(dir, "main work"));
    g(dir, &["merge", "-q", "--no-ff", "-m", "merge feature", "feature"]);
    shas.push(g(dir, &["rev-parse", "HEAD"]));

    // directory -> file replacement
    write(dir, "d/inner.txt", b"inner\n");
    shas.push(commit(dir, "add dir d"));
    fs::remove_dir_all(dir.join("d")).unwrap();
    write(dir, "d", b"now a file\n");
    shas.push(commit(dir, "dir becomes file"));

    // mode-only change
    set_mode(&dir.join("bin/run.sh"), 0o644);
    shas.push(commit(dir, "drop exec bit"));

    // file -> symlink type change
    fs::remove_file(dir.join("main-only.txt")).unwrap();
    symlink("src/a.txt", &dir.join("main-only.txt"));
    shas.push(commit(dir, "file becomes symlink"));
    shas
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

fn tree_of(dir: &Path, commit: &str) -> String {
    g(dir, &["rev-parse", &format!("{commit}^{{tree}}")])
}

#[test]
fn history_survives_a_round_trip_with_identical_git_trees() {
    let src = tempfile::tempdir().unwrap();
    let shas = build_history(src.path());

    let ag_dir = tempfile::tempdir().unwrap();
    let repo = ag_repo(ag_dir.path());
    let report = import(
        &repo,
        &ImportOptions {
            git_dir: src.path().to_path_buf(),
            rev: "HEAD".into(),
            stream: StreamName::new("imported").unwrap(),
            actor: Identity::new("user:importer").unwrap(),
        },
    )
    .unwrap();
    let total: usize = g(src.path(), &["rev-list", "--count", "HEAD"]).parse().unwrap();
    assert_eq!(report.commits, total);
    assert!(report.skipped_paths.is_empty(), "{:?}", report.skipped_paths);
    assert_eq!(repo.resolve_rev("imported").unwrap(), report.head);
    assert!(repo.fsck().unwrap().is_ok());

    // the imported history is a faithful DAG: same number of parents per commit
    let merge = repo.changeset(&report.head).unwrap();
    assert!(merge.message.contains("Imported-From: git "));

    // export it into a brand new git repository
    let out = tempfile::tempdir().unwrap();
    let rep = export(
        &repo,
        &ExportOptions { stream: "imported".into(), git_dir: out.path().to_path_buf(), branch: "main".into() },
    )
    .unwrap();
    assert_eq!(rep.commits, total);

    // pair commits via the Imported-From trailer and compare git trees byte for byte
    let log = g(out.path(), &["log", "--format=%H%x1f%P%x1f%B%x1e", "main"]);
    let mut by_orig: HashMap<String, (String, usize)> = HashMap::new();
    for rec in log.split('\u{1e}').filter(|r| !r.trim().is_empty()) {
        let f: Vec<&str> = rec.trim().splitn(3, '\u{1f}').collect();
        let orig = f[2].lines().find_map(|l| l.strip_prefix("Imported-From: git ")).expect("trailer").to_string();
        by_orig.insert(orig, (f[0].to_string(), f[1].split_whitespace().count()));
    }
    assert_eq!(by_orig.len(), total);
    for sha in &shas {
        let (exported, parents) = &by_orig[sha];
        assert_eq!(
            tree_of(out.path(), exported),
            tree_of(src.path(), sha),
            "tree of {sha} differs after the round trip"
        );
        assert_eq!(
            *parents,
            g(src.path(), &["rev-list", "--parents", "-n", "1", sha]).split_whitespace().count() - 1,
            "parent count of {sha}"
        );
    }
    // the working tree of the exported repo equals the original's (executable bits, symlinks and all)
    let a = g(src.path(), &["ls-files", "-s"]);
    let b = g(out.path(), &["ls-files", "-s"]);
    assert_eq!(a, b);
}

#[test]
fn import_reports_what_it_cannot_represent_and_skips_submodules() {
    let src = tempfile::tempdir().unwrap();
    let dir = src.path();
    g(dir, &["init", "-q", "-b", "main"]);
    // Built with plumbing, not a working tree: a name with a backslash cannot exist as a file on Windows and
    // Git for Windows refuses it in the index, but a tree object can hold it.
    let ok = hash_object(dir, b"fine\n");
    let notes_readme = hash_object(dir, b"component .git-notes is fine\n");
    let weird = hash_object(dir, b"backslash\n");
    let notes = mktree(dir, &format!("100644 blob {notes_readme}\treadme\n"));
    let a = mktree(dir, &format!("040000 tree {notes}\t.git-notes\n"));
    let sub = mktree(dir, "160000 commit 1111111111111111111111111111111111111111\tsub\n");
    let root = mktree(
        dir,
        &format!(
            "040000 tree {a}\ta\n040000 tree {sub}\tvendor\n100644 blob {ok}\tok.txt\n100644 blob {weird}\tweird\\name.txt\n"
        ),
    );
    let commit = g(dir, &["commit-tree", &root, "-m", "tricky"]);
    g(dir, &["update-ref", "refs/heads/main", &commit]);

    let ag_dir = tempfile::tempdir().unwrap();
    let repo = ag_repo(ag_dir.path());
    let report = import(
        &repo,
        &ImportOptions {
            git_dir: dir.to_path_buf(),
            rev: "HEAD".into(),
            stream: StreamName::new("imported").unwrap(),
            actor: Identity::new("user:importer").unwrap(),
        },
    )
    .unwrap();
    assert_eq!(report.skipped_submodules, 1);
    assert_eq!(report.skipped_paths.len(), 1);
    assert!(report.skipped_paths[0].0.contains("weird"), "{:?}", report.skipped_paths);
    let tree = repo.tree_of(&report.head).unwrap();
    let names: Vec<_> = tree.iter().map(|(p, _)| p.as_str().to_string()).collect();
    assert_eq!(names, vec!["a/.git-notes/readme", "ok.txt"]);
}

fn mktree(dir: &Path, entries: &str) -> String {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["mktree", "--missing"]) // the submodule's commit does not exist here
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), entries.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn hostile_git_content_cannot_escape_or_plant_dot_git() {
    // `git mktree` happily builds trees with `.git` entries. Importing them would let a
    // repository smuggle a hook-bearing `.git` directory into every checkout.
    let src = tempfile::tempdir().unwrap();
    let dir = src.path();
    g(dir, &["init", "-q", "-b", "main"]);
    let evil = hash_object(dir, b"hook");

    let dot_git_dir = mktree(dir, &format!("100644 blob {evil}\tconfig\n"));
    let nested = mktree(dir, &format!("040000 tree {dot_git_dir}\t.GIT\n100644 blob {evil}\tfine.txt\n"));
    let root = mktree(
        dir,
        &format!("040000 tree {dot_git_dir}\t.git\n040000 tree {nested}\tsub\n100644 blob {evil}\tsafe.txt\n"),
    );
    let commit = g(dir, &["commit-tree", &root, "-m", "hostile"]);
    g(dir, &["update-ref", "refs/heads/main", &commit]);

    let ag_dir = tempfile::tempdir().unwrap();
    let repo = ag_repo(ag_dir.path());
    let report = import(
        &repo,
        &ImportOptions {
            git_dir: dir.to_path_buf(),
            rev: "HEAD".into(),
            stream: StreamName::new("imported").unwrap(),
            actor: Identity::new("user:importer").unwrap(),
        },
    )
    .unwrap();
    assert_eq!(report.skipped_paths.len(), 2, "{:?}", report.skipped_paths);
    let names: Vec<_> = repo.tree_of(&report.head).unwrap().iter().map(|(p, _)| p.as_str().to_string()).collect();
    assert_eq!(names, vec!["safe.txt", "sub/fine.txt"], "only the harmless files are imported");

    // and an attacker cannot smuggle such a path in through the engine either
    assert!(ag_types::RepoPath::new(".git/hooks/post-checkout").is_err());
    assert!(ag_types::RepoPath::new("sub/.GIT/config").is_err());
}

/// The Windows and macOS versions of the same attack: names that *are not* `.git` to Git on Linux but are to
/// the file system that writes them (NTFS drops trailing dots and spaces, knows `GIT~1` and the
/// `::$INDEX_ALLOCATION` stream; HFS+ ignores some Unicode). They must never reach a checkout; and paths that
/// are merely unwritable on Windows (`aux.c`, `a:b`) or merge on a case-folding disk are real data that is
/// imported, then refused whole — before a byte is written — where they cannot be held.
#[test]
fn hostile_windows_and_macos_content_cannot_plant_dot_git_or_overwrite_a_sibling() {
    use ag_integrate::workdir::{Platform, checkout_tree_for};

    let src = tempfile::tempdir().unwrap();
    let dir = src.path();
    g(dir, &["init", "-q", "-b", "main"]);
    let evil = hash_object(dir, b"hook");
    let dot_git_dir = mktree(dir, &format!("100644 blob {evil}\tconfig\n"));
    let sub = mktree(dir, &format!("040000 tree {dot_git_dir}\t.git::$INDEX_ALLOCATION\n100644 blob {evil}\tok.txt\n"));
    let root = mktree(
        dir,
        &format!(
            "040000 tree {dot_git_dir}\t.git.\n\
             040000 tree {dot_git_dir}\tgit~1\n\
             040000 tree {dot_git_dir}\t.GIT \n\
             040000 tree {dot_git_dir}\t.g\u{200c}it\n\
             040000 tree {dot_git_dir}\t.ag.\n\
             040000 tree {sub}\tsub\n\
             100644 blob {evil}\taux.c\n\
             100644 blob {evil}\tNUL\n\
             100644 blob {evil}\ta:b\n\
             100644 blob {evil}\tREADME\n\
             100644 blob {evil}\treadme\n\
             100644 blob {evil}\tsafe.txt\n"
        ),
    );
    let commit = g(dir, &["commit-tree", &root, "-m", "hostile"]);
    g(dir, &["update-ref", "refs/heads/main", &commit]);

    let ag_dir = tempfile::tempdir().unwrap();
    let repo = ag_repo(ag_dir.path());
    let report = import(
        &repo,
        &ImportOptions {
            git_dir: dir.to_path_buf(),
            rev: "HEAD".into(),
            stream: StreamName::new("imported").unwrap(),
            actor: Identity::new("user:importer").unwrap(),
        },
    )
    .unwrap();
    // every alias of .git / .ag is skipped, not imported
    let skipped: Vec<&str> = report.skipped_paths.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(report.skipped_paths.len(), 6, "{skipped:?}");
    let tree = repo.tree_of(&report.head).unwrap();
    let names: Vec<_> = tree.iter().map(|(p, _)| p.as_str().to_string()).collect();
    assert_eq!(names, vec!["NUL", "README", "a:b", "aux.c", "readme", "safe.txt", "sub/ok.txt"]);

    // Windows cannot hold aux.c, NUL, a:b and merges README/readme: the whole checkout is refused, untouched.
    let dst = tempfile::tempdir().unwrap();
    let err = checkout_tree_for(&repo, &tree, dst.path(), &Platform::WINDOWS).unwrap_err().to_string();
    for needle in ["aux.c", "NUL", "a:b", "README and readme"] {
        assert!(err.contains(needle), "{needle}: {err}");
    }
    assert!(fs::read_dir(dst.path()).unwrap().next().is_none(), "nothing written");
    // macOS merges only the case pair
    let err = checkout_tree_for(&repo, &tree, dst.path(), &Platform::MACOS).unwrap_err().to_string();
    assert!(err.contains("README and readme") && !err.contains("aux.c"), "{err}");
    assert!(fs::read_dir(dst.path()).unwrap().next().is_none(), "nothing written");
    // and no spelling of .git was ever written anywhere
    assert!(!dst.path().join(".git.").exists() && !dst.path().join("git~1").exists());
}

#[test]
fn importing_into_a_stream_that_has_history_is_refused() {
    let src = tempfile::tempdir().unwrap();
    g(src.path(), &["init", "-q", "-b", "main"]);
    write(src.path(), "a", b"1");
    commit(src.path(), "one");
    let ag_dir = tempfile::tempdir().unwrap();
    let repo = ag_repo(ag_dir.path());
    let opts = ImportOptions {
        git_dir: src.path().to_path_buf(),
        rev: "HEAD".into(),
        stream: StreamName::new("main").unwrap(),
        actor: Identity::new("user:importer").unwrap(),
    };
    import(&repo, &opts).unwrap();
    assert!(import(&repo, &opts).is_err(), "a second import into the same stream must fail, not rewrite history");
}

#[test]
fn bad_inputs_are_errors() {
    let ag_dir = tempfile::tempdir().unwrap();
    let repo = ag_repo(ag_dir.path());
    let not_git = tempfile::tempdir().unwrap();
    let opts = ImportOptions {
        git_dir: not_git.path().to_path_buf(),
        rev: "HEAD".into(),
        stream: StreamName::new("imported").unwrap(),
        actor: Identity::new("user:importer").unwrap(),
    };
    assert!(import(&repo, &opts).is_err());
    assert!(
        export(
            &repo,
            &ExportOptions { stream: "main".into(), git_dir: not_git.path().join("out"), branch: "--evil".into() }
        )
        .is_err()
    );
}
