//! Complete repository migration: shared history, branches, tags and loss reporting.
#![allow(missing_docs)]
use ag_git::{MigrationOptions, migrate};
use ag_integrate::{InitOptions, Repository};
use ag_types::{Identity, Namespace, StreamName};
use std::{fs, path::Path, process::Command};
fn git(root: &Path, args: &[&str]) -> String {
    let r = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    String::from_utf8(r.stdout).unwrap().trim().into()
}
fn repo(path: &Path) -> Repository {
    Repository::init(
        path,
        &InitOptions {
            namespace: Namespace::new("test/migration").unwrap(),
            default_stream: StreamName::new("main").unwrap(),
            actor: Identity::new("user:test").unwrap(),
            storage: None,
        },
    )
    .unwrap()
}
#[test]
fn every_branch_and_tag_is_imported_once_and_the_source_stays_unchanged() {
    let source = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    git(source.path(), &["init", "-q", "-b", "main"]);
    fs::write(source.path().join("one.txt"), "one\n").unwrap();
    git(source.path(), &["add", "."]);
    git(source.path(), &["commit", "-qm", "one"]);
    git(source.path(), &["tag", "-a", "v1", "-m", "version one"]);
    git(source.path(), &["checkout", "-qb", "feature"]);
    fs::write(source.path().join("two.txt"), "two\n").unwrap();
    git(source.path(), &["add", "."]);
    git(source.path(), &["commit", "-qm", "two"]);
    git(source.path(), &["checkout", "-q", "main"]);
    let before = git(source.path(), &["show-ref"]);
    let r = repo(dest.path());
    let report = migrate(
        &r,
        &MigrationOptions {
            git_dir: source.path().into(),
            actor: Identity::new("user:test").unwrap(),
            allow_lossy: false,
        },
    )
    .unwrap();
    assert_eq!(report.commits, 2);
    assert_eq!(report.branches.len(), 2);
    assert_eq!(report.tags.len(), 1);
    assert_eq!(report.default_branch, "main");
    assert_eq!(r.pins().unwrap()[0].name, "git-tag/v1");
    assert_eq!(r.resolve_rev("main").unwrap(), report.branches.iter().find(|b| b.name == "main").unwrap().changeset);
    assert_eq!(
        r.resolve_rev("feature").unwrap(),
        report.branches.iter().find(|b| b.name == "feature").unwrap().changeset
    );
    let exported = tempfile::tempdir().unwrap();
    for branch in &report.branches {
        ag_git::export(
            &r,
            &ag_git::ExportOptions {
                stream: branch.destination.clone(),
                git_dir: exported.path().into(),
                branch: branch.name.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            git(source.path(), &["rev-parse", &format!("{}^{{tree}}", branch.name)]),
            git(exported.path(), &["rev-parse", &format!("{}^{{tree}}", branch.name)])
        );
    }
    assert_eq!(git(source.path(), &["show-ref"]), before);
    assert!(
        migrate(
            &r,
            &MigrationOptions {
                git_dir: source.path().into(),
                actor: Identity::new("user:test").unwrap(),
                allow_lossy: false
            }
        )
        .is_err()
    );
    assert_eq!(r.pins().unwrap().len(), 1);
}
#[test]
fn lfs_requires_an_explicit_decision_and_leaves_no_stream_history() {
    let source = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    git(source.path(), &["init", "-q", "-b", "main"]);
    fs::write(
        source.path().join("asset.bin"),
        format!("version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize 123\n", "a".repeat(64)),
    )
    .unwrap();
    git(source.path(), &["add", "."]);
    git(source.path(), &["commit", "-qm", "pointer"]);
    let r = repo(dest.path());
    let mut options = MigrationOptions {
        git_dir: source.path().into(),
        actor: Identity::new("user:test").unwrap(),
        allow_lossy: false,
    };
    let error = migrate(&r, &options).unwrap_err().to_string();
    assert!(error.contains("LFS"), "{error}");
    assert!(r.meta().read(|t| Ok(t.stream("main")?.unwrap().state.head)).unwrap().is_none());
    options.allow_lossy = true;
    let report = migrate(&r, &options).unwrap();
    assert_eq!(report.lfs_pointers, 1);
}
#[test]
fn a_conflicting_destination_never_leaves_other_branches_or_tags() {
    let source = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    git(source.path(), &["init", "-q", "-b", "main"]);
    fs::write(source.path().join("a"), "a").unwrap();
    git(source.path(), &["add", "."]);
    git(source.path(), &["commit", "-qm", "a"]);
    git(source.path(), &["branch", "feature"]);
    git(source.path(), &["tag", "v1"]);
    let r = repo(dest.path());
    ag_git::import(
        &r,
        &ag_git::ImportOptions {
            git_dir: source.path().into(),
            rev: "HEAD".into(),
            stream: StreamName::new("feature").unwrap(),
            actor: Identity::new("user:test").unwrap(),
        },
    )
    .unwrap();
    assert!(
        migrate(
            &r,
            &MigrationOptions {
                git_dir: source.path().into(),
                actor: Identity::new("user:test").unwrap(),
                allow_lossy: false
            }
        )
        .is_err()
    );
    assert!(r.meta().read(|t| Ok(t.stream("main")?.unwrap().state.head)).unwrap().is_none());
    assert!(r.pins().unwrap().is_empty());
}

#[test]
fn git_branch_names_are_mapped_without_dropping_case_sensitive_refs() {
    let source = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    git(source.path(), &["init", "-q", "-b", "Main"]);
    fs::write(source.path().join("a"), "a").unwrap();
    git(source.path(), &["add", "."]);
    git(source.path(), &["commit", "-qm", "a"]);
    // Keep case-distinct refs packed so the fixture also works on default macOS filesystems.
    git(source.path(), &["pack-refs", "--all"]);
    git(source.path(), &["branch", "main"]);
    git(source.path(), &["pack-refs", "--all"]);
    git(source.path(), &["branch", "Änderung"]);
    let report = migrate(
        &repo(dest.path()),
        &MigrationOptions {
            git_dir: source.path().into(),
            actor: Identity::new("user:test").unwrap(),
            allow_lossy: false,
        },
    )
    .unwrap();
    assert_eq!(report.branches.len(), 3);
    assert_ne!(report.branches[0].destination, report.branches[1].destination);
    assert!(report.branches.iter().any(|b| b.name == "Main" && b.destination == report.default_branch));
}
