#![allow(missing_docs)]
use std::{fs, process::Command};
fn source_with_refs(root: &std::path::Path) -> std::path::PathBuf {
    let source = root.join("source");
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
    source
}

#[test]
fn standalone_migration_keeps_source_and_preserves_all_refs_and_backup_archive() {
    let temp = tempfile::tempdir().unwrap();
    let source = source_with_refs(temp.path());
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

#[test]
fn preview_verifies_all_refs_without_creating_the_destination_or_its_parent() {
    let temp = tempfile::tempdir().unwrap();
    let source = source_with_refs(temp.path());
    let dest = temp.path().join("new parent/destination");
    fs::write(source.join("hello.txt"), "uncommitted edits\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ag"))
        .args(["--json", "migrate", "--dry-run"])
        .arg(&source)
        .arg(&dest)
        .args(["--namespace", "test/preview"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["commits"], 1);
    assert_eq!(report["branches"].as_array().unwrap().len(), 2);
    assert_eq!(report["tags"].as_array().unwrap().len(), 1);
    assert!(!dest.parent().unwrap().exists());
    assert_eq!(fs::read_to_string(source.join("hello.txt")).unwrap(), "uncommitted edits\n");
}

#[test]
fn preview_reports_losses_without_authorizing_publication() {
    let temp = tempfile::tempdir().unwrap();
    let source = source_with_refs(temp.path());
    fs::write(
        source.join("asset.bin"),
        format!("version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize 123\n", "a".repeat(64)),
    )
    .unwrap();
    for args in [vec!["add", "."], vec!["-c", "commit.gpgsign=false", "commit", "-qm", "pointer"]] {
        assert!(Command::new("git").arg("-C").arg(&source).args(args).status().unwrap().success());
    }
    let dest = temp.path().join("destination");
    let run = |flags: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ag"))
            .arg("migrate")
            .arg(&source)
            .arg(&dest)
            .args(["--namespace", "test/preview"])
            .args(flags)
            .output()
            .unwrap()
    };
    let preview = run(&["--json", "--dry-run"]);
    assert!(preview.status.success(), "{}", String::from_utf8_lossy(&preview.stderr));
    let report: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(report["lfs_pointers"], 1);
    assert!(!dest.exists());
    let human = run(&["--dry-run"]);
    assert!(human.status.success());
    let text = String::from_utf8(human.stdout).unwrap();
    assert!(
        text.contains("not published") && text.contains("1 LFS pointer") && text.contains("--allow-lossy"),
        "{text}"
    );
    assert!(!run(&[]).status.success());
    assert!(!dest.exists());
    let published = run(&["--json", "--allow-lossy"]);
    assert!(published.status.success(), "{}", String::from_utf8_lossy(&published.stderr));
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&published.stdout).unwrap()["lfs_pointers"], 1);
    assert!(dest.join(".ag/import/report.json").is_file());
}
