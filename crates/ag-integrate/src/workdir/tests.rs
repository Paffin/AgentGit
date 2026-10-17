use std::sync::Arc;

use ag_storage::SystemClock;
use ag_types::{Namespace, StreamName};

use super::*;

fn repo() -> Repository {
    Repository::ephemeral(Namespace::new("t/t").unwrap(), StreamName::new("main").unwrap(), Arc::new(SystemClock)).unwrap()
}

fn write(root: &Path, rel: &str, data: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, data).unwrap();
}

#[test]
fn scan_and_checkout_roundtrip() {
    let r = repo();
    let src = tempfile::tempdir().unwrap();
    write(src.path(), "a.txt", b"alpha");
    write(src.path(), "dir/b.txt", b"beta");
    write(src.path(), "dir/sub/c.txt", b"");
    write(src.path(), "ünï/файл.txt", b"unicode");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        write(src.path(), "bin/run.sh", b"#!/bin/sh\n");
        fs::set_permissions(src.path().join("bin/run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("dir/b.txt", src.path().join("link")).unwrap();
    }
    let (tree, report) = scan(&r, src.path(), &IgnoreRules::none(), true).unwrap();
    assert!(report.skipped.is_empty());

    let dst = tempfile::tempdir().unwrap();
    checkout_tree(&r, &tree, dst.path()).unwrap();
    let (back, _) = scan(&r, dst.path(), &IgnoreRules::none(), false).unwrap();
    assert_eq!(back.id(), tree.id(), "checkout reproduces the exact tree");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dst.path().join("bin/run.sh")).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111);
        assert_eq!(fs::read_link(dst.path().join("link")).unwrap().to_str(), Some("dir/b.txt"));
    }
}

#[test]
fn ignore_rules() {
    let r = repo();
    let src = tempfile::tempdir().unwrap();
    write(src.path(), "keep.rs", b"k");
    write(src.path(), "target/debug/x", b"x");
    write(src.path(), "sub/target/y", b"y");
    write(src.path(), "notes.log", b"l");
    write(src.path(), "sub/deep/trace.log", b"l");
    write(src.path(), ".git/config", b"g");
    write(src.path(), ".ag/meta.db", b"m");
    write(src.path(), "docs/draft.md", b"d");
    write(src.path(), "other/draft.md", b"d");
    let rules = IgnoreRules::parse("# comment\ntarget/\n*.log\n/docs/draft.md\n").unwrap();
    let (tree, _) = scan(&r, src.path(), &rules, false).unwrap();
    let paths: Vec<_> = tree.iter().map(|(p, _)| p.as_str().to_string()).collect();
    assert_eq!(paths, ["keep.rs", "other/draft.md"]);
}

#[test]
fn scan_rejects_unrecordable_names_with_a_helpful_message() {
    let r = repo();
    let src = tempfile::tempdir().unwrap();
    write(src.path(), "bad\\name", b"x");
    let err = scan(&r, src.path(), &IgnoreRules::none(), false).unwrap_err();
    assert!(err.to_string().contains(".agignore"), "{err}");
}

#[cfg(unix)]
#[test]
fn checkout_refuses_to_write_through_symlinked_directories() {
    let r = repo();
    let outside = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    // A previous (hostile) state of the directory: `sub` is a symlink to outside.
    std::os::unix::fs::symlink(outside.path(), dst.path().join("sub")).unwrap();
    let blob = r.put_blob(b"pwned".to_vec()).unwrap();
    let tree = Tree::from_entries(
        [(RepoPath::new("sub/evil.txt").unwrap(), Entry { content: blob, file_type: FileType::Regular, executable: false })],
        r.limits(),
    )
    .unwrap();
    let err = checkout_tree(&r, &tree, dst.path()).unwrap_err();
    assert!(matches!(err, Error::UnsafePath(_)), "{err}");
    assert!(!outside.path().join("evil.txt").exists());
}

#[cfg(unix)]
#[test]
fn symlink_targets_must_stay_inside_the_root() {
    let r = repo();
    let blob = |t: &str| r.put_blob(t.as_bytes().to_vec()).unwrap();
    let mk = |path: &str, target: &str| {
        Tree::from_entries(
            [(RepoPath::new(path).unwrap(), Entry { content: blob(target), file_type: FileType::Symlink, executable: false })],
            r.limits(),
        )
        .unwrap()
    };
    for (path, target) in [("l", "/etc/passwd"), ("l", "../outside"), ("a/l", "../../outside"), ("l", ""), ("a/l", "x/../../../y")] {
        let dst = tempfile::tempdir().unwrap();
        let err = checkout_tree(&r, &mk(path, target), dst.path()).unwrap_err();
        assert!(matches!(err, Error::UnsafePath(_)), "{path} -> {target}: {err}");
    }
    for (path, target) in [("l", "a"), ("a/l", "../b"), ("a/b/l", "../../c"), ("l", "./x/./y")] {
        let dst = tempfile::tempdir().unwrap();
        checkout_tree(&r, &mk(path, target), dst.path()).unwrap_or_else(|e| panic!("{path} -> {target}: {e}"));
    }
}

#[test]
fn a_damaged_blob_is_an_error_not_a_silent_checkout() {
    use ag_object::RawObject;
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(
        dir.path(),
        &crate::InitOptions {
            namespace: Namespace::new("t/t").unwrap(),
            default_stream: StreamName::new("main").unwrap(),
            actor: ag_types::Identity::new("user:t").unwrap(),
        },
    )
    .unwrap();
    let blob = repo.put_blob(b"precious".to_vec()).unwrap();
    // damage the stored file
    let hex = blob.to_hex();
    let p = repo.layout().objects_dir().join("objects").join(&hex[..2]).join(&hex[2..]);
    let mut perms = fs::metadata(&p).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&p, perms).unwrap();
    let env = RawObject::blob(b"tampered".to_vec()).encode_envelope();
    fs::write(&p, env).unwrap();
    let tree = Tree::from_entries(
        [(RepoPath::new("f").unwrap(), Entry { content: blob, file_type: FileType::Regular, executable: false })],
        repo.limits(),
    )
    .unwrap();
    let dst = tempfile::tempdir().unwrap();
    assert!(checkout_tree(&repo, &tree, dst.path()).is_err());
    assert!(!dst.path().join("f").exists());
}

#[test]
fn update_dir_applies_only_the_difference() {
    let r = repo();
    let src = tempfile::tempdir().unwrap();
    write(src.path(), "keep.txt", b"k");
    write(src.path(), "gone/x.txt", b"x");
    write(src.path(), "chg.txt", b"1");
    let (old, _) = scan(&r, src.path(), &IgnoreRules::none(), true).unwrap();

    let src2 = tempfile::tempdir().unwrap();
    write(src2.path(), "keep.txt", b"k");
    write(src2.path(), "chg.txt", b"2");
    write(src2.path(), "new/y.txt", b"y");
    let (new, _) = scan(&r, src2.path(), &IgnoreRules::none(), true).unwrap();

    let work = tempfile::tempdir().unwrap();
    checkout_tree(&r, &old, work.path()).unwrap();
    update_dir(&r, &old, &new, work.path()).unwrap();
    let (now, _) = scan(&r, work.path(), &IgnoreRules::none(), false).unwrap();
    assert_eq!(now.id(), new.id());
    assert!(!work.path().join("gone").exists(), "empty directories are pruned");
}

#[test]
fn directory_replaced_by_file_in_one_update() {
    let r = repo();
    let a = tempfile::tempdir().unwrap();
    write(a.path(), "x/y.txt", b"1");
    let (old, _) = scan(&r, a.path(), &IgnoreRules::none(), true).unwrap();
    let b = tempfile::tempdir().unwrap();
    write(b.path(), "x", b"now a file");
    let (new, _) = scan(&r, b.path(), &IgnoreRules::none(), true).unwrap();
    let work = tempfile::tempdir().unwrap();
    checkout_tree(&r, &old, work.path()).unwrap();
    update_dir(&r, &old, &new, work.path()).unwrap();
    assert_eq!(fs::read(work.path().join("x")).unwrap(), b"now a file");
}
