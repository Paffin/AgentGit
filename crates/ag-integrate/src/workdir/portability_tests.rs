//! What a destination can hold (docs/operations/windows.md): device names, case collisions, symlinks without
//! symlinks, the executable bit without a bit, line endings, deep paths, read-only files, junctions.
//!
//! Almost every test names the platform it emulates, so the Windows and macOS rules are exercised on Linux
//! too; the few that touch the real file system assert what the host really does.

use std::sync::Arc;

use ag_storage::SystemClock;
use ag_types::{Namespace, StreamName};

use super::*;

fn repo() -> Repository {
    Repository::ephemeral(Namespace::new("t/t").unwrap(), StreamName::new("main").unwrap(), Arc::new(SystemClock))
        .unwrap()
}

fn write(root: &Path, rel: &str, data: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, data).unwrap();
}

/// A tree of regular files (`content` is also the file's text) and symlinks, built directly.
fn tree_of(r: &Repository, files: &[(&str, &str)], links: &[(&str, &str)], exec: &[&str]) -> Tree {
    let mut entries = Vec::new();
    for (path, content) in files {
        let blob = r.put_blob(content.as_bytes().to_vec()).unwrap();
        entries.push((
            RepoPath::new(*path).unwrap(),
            Entry { content: blob, file_type: FileType::Regular, executable: exec.contains(path) },
        ));
    }
    for (path, target) in links {
        let blob = r.put_blob(target.as_bytes().to_vec()).unwrap();
        entries.push((
            RepoPath::new(*path).unwrap(),
            Entry { content: blob, file_type: FileType::Symlink, executable: false },
        ));
    }
    Tree::from_entries(entries, r.limits()).unwrap()
}

fn is_empty_dir(p: &Path) -> bool {
    fs::read_dir(p).unwrap().next().is_none()
}

#[test]
fn a_tree_with_windows_device_names_is_refused_whole_before_anything_is_written() {
    let r = repo();
    // All of these are perfectly good paths on Linux; none can be written on Windows.
    let tree = tree_of(
        &r,
        &[
            ("fine.txt", "ok"),
            ("src/aux.c", "device AUX"),
            ("NUL", "device"),
            ("docs/con.txt", "device CON"),
            ("a:b", "alternate data stream"),
            ("trailing.", "dot"),
            ("space ", "space"),
            ("what?.txt", "wildcard"),
        ],
        &[],
        &[],
    );
    let dst = tempfile::tempdir().unwrap();
    let err = checkout_tree_for(&r, &tree, dst.path(), &Platform::WINDOWS).unwrap_err();
    let msg = err.to_string();
    assert!(matches!(err, Error::UnsafePath(_)), "{msg}");
    for needle in
        ["src/aux.c", "device name AUX", "NUL", "con.txt", "alternate data stream", "trailing dot", "what?.txt"]
    {
        assert!(msg.contains(needle), "the error names '{needle}': {msg}");
    }
    assert!(msg.contains("Nothing was written"), "{msg}");
    assert!(is_empty_dir(dst.path()), "not even fine.txt may be written: a partial checkout hides the problem");
    // The same tree is fine where those names are fine. That needs a real file system that holds them, which a
    // Windows host does not (`a:b` is an alternate data stream, `NUL` a device): there the refusal above is all
    // that can be observed, and it is the same code path, only the platform rules differ.
    #[cfg(unix)]
    {
        checkout_tree_for(&r, &tree, dst.path(), &Platform::POSIX).unwrap();
        assert_eq!(fs::read_to_string(dst.path().join("src/aux.c")).unwrap(), "device AUX");
    }
}

#[test]
fn names_that_differ_only_in_case_are_refused_where_they_would_merge_and_never_overwrite() {
    let r = repo();
    let tree =
        tree_of(&r, &[("README", "upper"), ("readme", "lower"), ("src/Lib.rs", "a"), ("SRC/other.rs", "b")], &[], &[]);
    for platform in [Platform::WINDOWS, Platform::MACOS] {
        let dst = tempfile::tempdir().unwrap();
        let err = checkout_tree_for(&r, &tree, dst.path(), &platform).unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, Error::UnsafePath(_)), "{msg}");
        assert!(msg.contains("README and readme"), "{msg}");
        assert!(msg.contains("SRC and src"), "directories are reported, not every file below: {msg}");
        assert!(is_empty_dir(dst.path()), "nothing written for {}", platform.name());
    }
    // Case-sensitive destination: both exist, with their own content.
    let dst = tempfile::tempdir().unwrap();
    if probe_case_insensitive(dst.path()).unwrap() {
        return; // this machine's temp directory folds case; the real-checkout test below covers it
    }
    checkout_tree_for(&r, &tree, dst.path(), &Platform::POSIX).unwrap();
    assert_eq!(fs::read_to_string(dst.path().join("README")).unwrap(), "upper");
    assert_eq!(fs::read_to_string(dst.path().join("readme")).unwrap(), "lower");
}

#[test]
fn a_real_checkout_never_silently_overwrites_on_a_case_folding_file_system() {
    let r = repo();
    let tree = tree_of(&r, &[("Makefile", "big"), ("makefile", "small")], &[], &[]);
    let dst = tempfile::tempdir().unwrap();
    let folds = probe_case_insensitive(dst.path()).unwrap();
    // The probe measures this very directory: default NTFS and APFS fold case, ext4 and tmpfs do not.
    assert_eq!(folds, cfg!(any(windows, target_os = "macos")), "the probe disagrees with the platform default");
    assert_eq!(Platform::for_dir(dst.path()).target.case_insensitive, folds);
    match checkout_tree(&r, &tree, dst.path()) {
        Ok(()) => {
            assert!(!folds, "a folding file system must refuse, not pick a winner");
            assert_eq!(fs::read_to_string(dst.path().join("Makefile")).unwrap(), "big");
            assert_eq!(fs::read_to_string(dst.path().join("makefile")).unwrap(), "small");
        }
        Err(e) => {
            assert!(folds, "{e}");
            assert!(e.to_string().contains("Makefile and makefile"), "{e}");
            assert!(is_empty_dir(dst.path()));
        }
    }
}

#[test]
fn the_probe_leaves_nothing_behind_and_a_probe_that_cannot_run_is_not_fatal() {
    let dst = tempfile::tempdir().unwrap();
    probe_case_insensitive(dst.path()).unwrap();
    assert!(is_empty_dir(dst.path()));
    let missing = dst.path().join("does-not-exist");
    assert!(probe_case_insensitive(&missing).is_err());
    assert_eq!(Platform::for_dir(&missing), Platform::host());
}

#[test]
fn an_update_that_would_create_a_collision_with_an_existing_file_is_refused_untouched() {
    let r = repo();
    let old = tree_of(&r, &[("README", "upper")], &[], &[]);
    let new = tree_of(&r, &[("README", "upper"), ("readme", "lower")], &[], &[]);
    let work = tempfile::tempdir().unwrap();
    checkout_tree_for(&r, &old, work.path(), &Platform::POSIX).unwrap();
    let err = update_dir_for(&r, &old, &new, work.path(), &Platform::WINDOWS).unwrap_err();
    assert!(err.to_string().contains("README and readme"), "{err}");
    let names: Vec<_> = fs::read_dir(work.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names.len(), 1, "{names:?}");
    assert_eq!(fs::read_to_string(work.path().join("README")).unwrap(), "upper");
}

#[test]
fn a_case_only_rename_is_applied_by_deleting_first() {
    let r = repo();
    let old = tree_of(&r, &[("notes/todo.txt", "same"), ("keep", "k")], &[], &[]);
    let new = tree_of(&r, &[("Notes/ToDo.txt", "same"), ("keep", "k")], &[], &[]);
    let work = tempfile::tempdir().unwrap();
    checkout_tree(&r, &old, work.path()).unwrap();
    update_dir(&r, &old, &new, work.path()).unwrap();
    let names = |p: &Path| {
        let mut v: Vec<String> =
            fs::read_dir(p).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        v.sort();
        v
    };
    assert_eq!(
        names(work.path()),
        ["Notes", "keep"],
        "the directory has its new spelling, whatever the file system folds"
    );
    assert_eq!(names(&work.path().join("Notes")), ["ToDo.txt"]);
    let (now, _) = scan(&r, work.path(), &IgnoreRules::none(), false).unwrap();
    assert_eq!(now.id(), new.id());
}

#[test]
fn symlinks_become_placeholder_files_on_platforms_without_them_and_status_stays_clean() {
    let r = repo();
    let tree = tree_of(&r, &[("dir/b.txt", "beta")], &[("link", "dir/b.txt"), ("dir/up", "../dir/b.txt")], &[]);
    let dst = tempfile::tempdir().unwrap();
    checkout_tree_for(&r, &tree, dst.path(), &Platform::WINDOWS).unwrap();
    // plain files holding the target, nothing else
    assert_eq!(fs::read_to_string(dst.path().join("link")).unwrap(), "dir/b.txt");
    assert!(!fs::symlink_metadata(dst.path().join("link")).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_to_string(dst.path().join("dir/up")).unwrap(), "../dir/b.txt");

    let adapted = adaptations(&tree, &Platform::WINDOWS);
    assert_eq!(adapted.symlinks_as_files, ["dir/up", "link"]);
    let text = adapted.describe().join("\n");
    assert!(text.contains("2 symlink(s)") && text.contains("plain files"), "{text}");
    assert!(adaptations(&tree, &Platform::POSIX).is_empty());

    // Scanned against the tree they came from, the placeholders are the symlinks (no phantom change)...
    let (same, _) = scan_with(&r, dst.path(), &IgnoreRules::none(), false, Some(&tree), &Platform::WINDOWS).unwrap();
    assert_eq!(same.id(), tree.id());
    // ...scanned blind they are files, and an edited placeholder is a real change to a regular file.
    let (blind, _) = scan_with(&r, dst.path(), &IgnoreRules::none(), false, None, &Platform::WINDOWS).unwrap();
    assert_ne!(blind.id(), tree.id());
    fs::write(dst.path().join("link"), "other.txt").unwrap();
    let (edited, _) = scan_with(&r, dst.path(), &IgnoreRules::none(), false, Some(&tree), &Platform::WINDOWS).unwrap();
    let link = edited.get(&RepoPath::new("link").unwrap()).unwrap();
    assert_eq!(link.file_type, FileType::Regular);
    assert_eq!(tree.diff(&edited).len(), 1);
}

#[test]
fn placeholder_symlinks_are_still_checked_for_escapes() {
    let r = repo();
    for target in ["/etc/passwd", "../outside", "C:/Windows/win.ini", "c:secret", "..\\..\\x"] {
        let tree = tree_of(&r, &[], &[("l", target)], &[]);
        let dst = tempfile::tempdir().unwrap();
        let err = checkout_tree_for(&r, &tree, dst.path(), &Platform::WINDOWS).unwrap_err();
        assert!(matches!(err, Error::UnsafePath(_)), "{target}: {err}");
        assert!(is_empty_dir(dst.path()), "{target}");
    }
}

#[test]
fn the_executable_bit_is_remembered_from_the_tree_where_the_platform_has_none() {
    let r = repo();
    let tree = tree_of(&r, &[("bin/run.sh", "#!/bin/sh\n"), ("plain.txt", "p")], &[], &["bin/run.sh"]);
    let dst = tempfile::tempdir().unwrap();
    checkout_tree_for(&r, &tree, dst.path(), &Platform::WINDOWS).unwrap();
    assert_eq!(adaptations(&tree, &Platform::WINDOWS).executables_without_bit, ["bin/run.sh"]);
    assert!(adaptations(&tree, &Platform::WINDOWS).describe()[0].contains("no executable bit"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dst.path().join("bin/run.sh")).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0, "the bit is not applied where the platform has none");
    }
    let (same, _) = scan_with(&r, dst.path(), &IgnoreRules::none(), false, Some(&tree), &Platform::WINDOWS).unwrap();
    assert_eq!(same.id(), tree.id(), "the flag survives a status on a platform that cannot see it");
    // content changes keep the flag; a new file never has it
    fs::write(dst.path().join("bin/run.sh"), "#!/bin/sh\necho changed\n").unwrap();
    fs::write(dst.path().join("new.sh"), "x").unwrap();
    let (now, _) = scan_with(&r, dst.path(), &IgnoreRules::none(), false, Some(&tree), &Platform::WINDOWS).unwrap();
    assert!(now.get(&RepoPath::new("bin/run.sh").unwrap()).unwrap().executable);
    assert!(!now.get(&RepoPath::new("new.sh").unwrap()).unwrap().executable);
}

#[test]
fn line_endings_are_bytes_never_converted() {
    let r = repo();
    let crlf = "one\r\ntwo\r\n";
    let mixed = "a\nb\r\nc\rd\n";
    let tree = tree_of(&r, &[("crlf.txt", crlf), ("mixed.txt", mixed), ("lf.txt", "x\ny\n")], &[], &[]);
    let dst = tempfile::tempdir().unwrap();
    checkout_tree(&r, &tree, dst.path()).unwrap();
    assert_eq!(fs::read(dst.path().join("crlf.txt")).unwrap(), crlf.as_bytes());
    assert_eq!(fs::read(dst.path().join("mixed.txt")).unwrap(), mixed.as_bytes());
    assert_eq!(fs::read(dst.path().join("lf.txt")).unwrap(), b"x\ny\n");
    let (back, _) = scan(&r, dst.path(), &IgnoreRules::none(), true).unwrap();
    assert_eq!(back.id(), tree.id(), "scan(checkout(tree)) is the same tree, byte for byte");
}

#[test]
fn deep_paths_beyond_the_legacy_windows_limit_round_trip() {
    let r = repo();
    // 8 directories of 40 characters plus a file name: more than 330 characters below the root, past MAX_PATH.
    let dir = (0..8).map(|i| format!("{i}{}", "d".repeat(39))).collect::<Vec<_>>().join("/");
    let path = format!("{dir}/{}", "f".repeat(60));
    assert!(path.len() > 300);
    let tree = tree_of(&r, &[(path.as_str(), "deep")], &[], &[]);
    let dst = tempfile::tempdir().unwrap();
    checkout_tree(&r, &tree, dst.path()).unwrap();
    let (back, _) = scan(&r, dst.path(), &IgnoreRules::none(), false).unwrap();
    assert_eq!(back.id(), tree.id());
    // and it can be updated away again, pruning every emptied directory
    let empty = tree_of(&r, &[], &[], &[]);
    update_dir(&r, &tree, &empty, dst.path()).unwrap();
    assert!(is_empty_dir(dst.path()));
}

#[test]
fn a_read_only_file_is_replaced_and_removed_by_an_update() {
    let r = repo();
    let old = tree_of(&r, &[("locked.txt", "v1"), ("gone.txt", "g")], &[], &[]);
    let new = tree_of(&r, &[("locked.txt", "v2")], &[], &[]);
    let work = tempfile::tempdir().unwrap();
    checkout_tree(&r, &old, work.path()).unwrap();
    for f in ["locked.txt", "gone.txt"] {
        let mut perms = fs::metadata(work.path().join(f)).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(work.path().join(f), perms).unwrap();
    }
    // On Windows the read-only attribute blocks both replacing and deleting; elsewhere the directory decides.
    update_dir(&r, &old, &new, work.path()).unwrap();
    assert_eq!(fs::read_to_string(work.path().join("locked.txt")).unwrap(), "v2");
    assert!(!work.path().join("gone.txt").exists());
}

#[test]
fn a_leftover_case_probe_is_not_part_of_the_project() {
    let r = repo();
    let src = tempfile::tempdir().unwrap();
    write(src.path(), "a.txt", b"a");
    write(src.path(), ".ag-case-probe-p_0123456789", b"");
    let (tree, _) = scan(&r, src.path(), &IgnoreRules::none(), false).unwrap();
    let paths: Vec<_> = tree.iter().map(|(p, _)| p.as_str().to_string()).collect();
    assert_eq!(paths, ["a.txt"]);
}

#[test]
fn the_platform_names_round_trip() {
    assert_eq!(Platform::named("Windows"), Some(Platform::WINDOWS));
    assert_eq!(Platform::named("macos"), Some(Platform::MACOS));
    assert_eq!(Platform::named("linux"), Some(Platform::POSIX));
    assert_eq!(Platform::named("plan9"), None);
    assert_eq!(Platform::WINDOWS.name(), "Windows");
    assert_eq!(Platform::MACOS.name(), "macOS");
    assert_eq!(Platform::POSIX.name(), "Linux");
}

/// A planted Windows junction (a directory link that needs no privilege to create) must not be written
/// through: the same defence as the Unix symlink test, for the Windows way of planting a link.
#[cfg(windows)]
#[test]
fn checkout_refuses_to_write_through_a_junction() {
    let r = repo();
    let outside = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(dst.path().join("sub"))
        .arg(outside.path())
        .output()
        .unwrap();
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stdout));
    let tree = tree_of(&r, &[("sub/evil.txt", "pwned")], &[], &[]);
    let err = checkout_tree(&r, &tree, dst.path()).unwrap_err();
    assert!(matches!(err, Error::UnsafePath(_)), "{err}");
    assert!(!outside.path().join("evil.txt").exists());
}
