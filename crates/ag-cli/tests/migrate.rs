#![allow(missing_docs)]
use std::{fs, process::Command};
#[test]
fn standalone_migration_keeps_source_and_preserves_all_refs_and_backup_archive() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    for args in [
        vec!["init", "-qb", "main"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@example.invalid"],
    ] {
        assert!(Command::new("git").arg("-C").arg(&source).args(args).status().unwrap().success());
    }
    fs::write(source.join("hello.txt"), "original content\n").unwrap();
    for args in [
        vec!["add", "."],
        vec!["-c", "commit.gpgsign=false", "commit", "-qm", "original"],
        vec!["branch", "feature"],
        vec!["tag", "v1"],
    ] {
        assert!(Command::new("git").arg("-C").arg(&source).args(args).status().unwrap().success());
    }
    let dest = temp.path().join("destination");
    let output = Command::new(env!("CARGO_BIN_EXE_ag"))
        .args(["--json", "migrate"])
        .arg(&source)
        .arg(&dest)
        .args(["--namespace", "test/moved"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["branches"].as_array().unwrap().len(), 2);
    let repo = ag_integrate::Repository::open(&dest).unwrap();
    assert_eq!(repo.pins().unwrap()[0].name, "git-tag/v1");
    assert!(source.join(".git").is_dir());
    assert_eq!(fs::read_to_string(source.join("hello.txt")).unwrap(), "original content\n");
    assert_eq!(fs::read_to_string(dest.join("hello.txt")).unwrap(), "original content\n");
    let backup = temp.path().join("backup");
    repo.backup(&backup, false).unwrap();
    assert_eq!(
        fs::read(dest.join(".ag/import/source.bundle")).unwrap(),
        fs::read(backup.join(".ag/import/source.bundle")).unwrap()
    );
    let again = Command::new(env!("CARGO_BIN_EXE_ag"))
        .arg("migrate")
        .arg(&source)
        .arg(&dest)
        .args(["--namespace", "test/moved"])
        .output()
        .unwrap();
    assert!(!again.status.success());
    assert!(repo.fsck().unwrap().is_ok());
}
