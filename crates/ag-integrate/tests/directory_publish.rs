#![allow(missing_docs)]
use ag_integrate::publish_new_directory;
use std::fs;

#[cfg(windows)]
#[test]
fn publication_rejects_a_nul_instead_of_truncating_the_destination() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("prepared");
    fs::create_dir(&source).unwrap();
    let destination = root.path().join("published\0ignored");
    let error = publish_new_directory(&source, &destination).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(source.exists());
    assert!(!root.path().join("published").exists());
}

#[test]
fn publication_moves_the_complete_directory_once_without_overwriting_even_an_empty_target() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("prepared");
    let destination = root.path().join("published");
    fs::create_dir_all(source.join(".ag/import")).unwrap();
    fs::write(source.join(".ag/import/id"), "migration-one").unwrap();
    fs::write(source.join("file"), "complete content").unwrap();
    fs::create_dir(&destination).unwrap();
    assert!(publish_new_directory(&source, &destination).is_err());
    assert!(source.join(".ag/import/id").exists());
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    fs::remove_dir(&destination).unwrap();
    publish_new_directory(&source, &destination).unwrap();
    assert!(!source.exists());
    assert_eq!(fs::read_to_string(destination.join("file")).unwrap(), "complete content");
    assert_eq!(fs::read_to_string(destination.join(".ag/import/id")).unwrap(), "migration-one");
    assert!(publish_new_directory(&source, &destination).is_err());
    assert_eq!(fs::read_to_string(destination.join("file")).unwrap(), "complete content");
}
