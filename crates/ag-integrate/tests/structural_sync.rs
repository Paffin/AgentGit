//! Resolving one structural conflict must preserve every clean merge change.
mod common;

use ag_integrate::{SyncOutcome, VirtualChange, VirtualContent};
use ag_types::{FileType, RepoPath, StreamName};
use common::*;

fn delete(path: &str) -> VirtualChange {
    VirtualChange::Delete { path: RepoPath::new(path).unwrap() }
}
fn put(path: &str, text: &str) -> VirtualChange {
    VirtualChange::Put {
        path: RepoPath::new(path).unwrap(),
        content: VirtualContent::Bytes(text.as_bytes().to_vec()),
        file_type: FileType::Regular,
        executable: false,
    }
}

#[test]
fn structural_resolution_preserves_clean_add_edit_delete_for_both_sides_and_choices() {
    for workspace_has_file in [false, true] {
        for choose_file in [false, true] {
            let f = fx();
            let actor = id("user:alice");
            let base = f.land(None, &[("stable", "same"), ("edited", "before"), ("removed", "old")]);
            let mut our_files = vec![("stable", "same"), ("edited", "before"), ("removed", "old"), ("local", "ours")];
            let mut their_files = vec![("stable", "same"), ("edited", "after"), ("added", "theirs")];
            let file = [("clash", "file")];
            let directory = [("clash/a", "a"), ("clash/sub/b", "b")];
            our_files.extend(if workspace_has_file { &file[..] } else { &directory[..] });
            their_files.extend(if workspace_has_file { &directory[..] } else { &file[..] });
            let ours = f.snap(Some(base), &our_files, "user:alice");
            let theirs = f.land(Some(base), &their_files);
            let observed =
                f.repo.workspace_create_at("work", &StreamName::new("main").unwrap(), &actor, Some(&ours)).unwrap();
            let before = f.repo.workspace_sync_virtual_checked_at(&observed, &actor, &[], Some(&theirs)).unwrap();
            assert!(matches!(before, SyncOutcome::Conflicted { .. }));
            assert_eq!(f.repo.workspace_row("work").unwrap().head, Some(ours));
            let resolutions = if choose_file {
                vec![put("clash", "chosen file"), delete("clash/a"), delete("clash/sub/b")]
            } else {
                vec![delete("clash")]
            };
            let merged =
                f.repo.workspace_sync_virtual_checked_at(&observed, &actor, &resolutions, Some(&theirs)).unwrap();
            let SyncOutcome::Merged { id: head } = merged else { panic!("{merged:?}") };
            assert_eq!(
                f.read(&head, "added").as_deref(),
                Some("theirs"),
                "clean addition, ours-file={workspace_has_file}, choose-file={choose_file}"
            );
            assert_eq!(f.read(&head, "edited").as_deref(), Some("after"), "clean edit must survive");
            assert_eq!(f.read(&head, "removed"), None, "clean deletion must survive");
            assert_eq!(f.read(&head, "local").as_deref(), Some("ours"));
            assert_eq!(f.read(&head, "stable").as_deref(), Some("same"));
            assert_eq!(f.read(&head, "clash").as_deref(), choose_file.then_some("chosen file"));
            assert_eq!(f.read(&head, "clash/a").as_deref(), (!choose_file).then_some("a"));
            assert_eq!(f.read(&head, "clash/sub/b").as_deref(), (!choose_file).then_some("b"));
            assert_eq!(f.repo.changeset(&head).unwrap().parents, [ours, theirs]);
            assert_eq!(f.repo.workspace_row("work").unwrap().base, Some(theirs));
            assert_eq!(f.head(), Some(theirs), "sync must not move the shared stream");
            assert!(f.repo.fsck().unwrap().is_ok());
        }
    }
}

#[test]
fn incomplete_structural_resolution_cannot_silently_discard_descendants() {
    let f = fx();
    let actor = id("user:alice");
    let base = f.land(None, &[("base", "same")]);
    let ours = f.snap(Some(base), &[("base", "same"), ("clash", "ours")], "user:alice");
    let theirs = f.land(Some(base), &[("base", "same"), ("clash/child", "theirs"), ("added", "keep")]);
    let observed = f.repo.workspace_create_at("work", &StreamName::new("main").unwrap(), &actor, Some(&ours)).unwrap();
    let result = f.repo.workspace_sync_virtual_checked_at(&observed, &actor, &[put("clash", "chosen")], Some(&theirs));
    assert!(result.is_err(), "keeping a file must not implicitly delete the unseen child: {result:?}");
    let after = f.repo.workspace_row("work").unwrap();
    assert_eq!(after.head, observed.head);
    assert_eq!(after.base, observed.base);
    assert_eq!(f.head(), Some(theirs));
}

#[test]
fn directory_sync_refuses_structural_conflicts_unchanged_and_can_retry_after_snapshot() {
    for workspace_has_file in [false, true] {
        let f = fx();
        let actor = id("user:alice");
        let base = f.land(None, &[("stable", "same"), ("edited", "before"), ("removed", "old")]);
        let mut our_files = vec![("stable", "same"), ("edited", "before"), ("removed", "old"), ("local", "ours")];
        let mut their_files = vec![("stable", "same"), ("edited", "after"), ("added", "theirs")];
        let file = [("clash", "file")];
        let directory = [("clash/child", "child")];
        our_files.extend(if workspace_has_file { &file[..] } else { &directory[..] });
        their_files.extend(if workspace_has_file { &directory[..] } else { &file[..] });
        let ours = f.snap(Some(base), &our_files, "user:alice");
        let theirs = f.land(Some(base), &their_files);
        let observed =
            f.repo.workspace_create_at("work", &StreamName::new("main").unwrap(), &actor, Some(&ours)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        f.repo.workspace_checkout("work", dir.path()).unwrap();
        let result = f.repo.sync_workspace("work", dir.path(), &actor);
        assert!(result.is_err(), "directory sync must not treat its fallback as the full merge: {result:?}");
        let error = result.unwrap_err().to_string();
        assert!(error.contains("file/directory") && error.contains("unchanged"), "{error}");
        let after = f.repo.workspace_row("work").unwrap();
        assert_eq!(after.head, observed.head);
        assert_eq!(after.base, observed.base);
        assert!(!f.repo.workspace_is_merging("work").unwrap());
        assert!(f.repo.workspace_status("work", dir.path()).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(dir.path().join("edited")).unwrap(), "before");
        assert!(dir.path().join("removed").exists());
        assert!(!dir.path().join("added").exists());

        // Adopt the branch's structural shape explicitly, then retry the ordinary merge.
        if workspace_has_file {
            std::fs::remove_file(dir.path().join("clash")).unwrap();
            std::fs::create_dir(dir.path().join("clash")).unwrap();
            std::fs::write(dir.path().join("clash/child"), "child").unwrap();
        } else {
            std::fs::remove_dir_all(dir.path().join("clash")).unwrap();
            std::fs::write(dir.path().join("clash"), "file").unwrap();
        }
        let chosen = f.repo.snapshot("work", dir.path(), "Choose the branch layout", &actor, false).unwrap().unwrap();
        assert_eq!(f.repo.changeset(&chosen).unwrap().parents, [ours]);
        let result = f.repo.sync_workspace("work", dir.path(), &actor).unwrap();
        let SyncOutcome::Merged { id: head } = result else { panic!("{result:?}") };
        assert_eq!(f.read(&head, "added").as_deref(), Some("theirs"));
        assert_eq!(f.read(&head, "edited").as_deref(), Some("after"));
        assert_eq!(f.read(&head, "removed"), None);
        assert_eq!(f.read(&head, "local").as_deref(), Some("ours"));
        assert_eq!(f.repo.changeset(&head).unwrap().parents, [chosen, theirs]);
        assert_eq!(f.head(), Some(theirs));
        assert!(!f.repo.workspace_is_merging("work").unwrap());
        assert!(f.repo.workspace_status("work", dir.path()).unwrap().is_empty());
    }
}

#[test]
fn legacy_structural_pending_snapshot_is_refused_even_with_force() {
    for force in [false, true] {
        let f = fx();
        let actor = id("user:alice");
        let base = f.land(None, &[("base", "same")]);
        let ours = f.snap(Some(base), &[("base", "same"), ("clash", "ours")], "user:alice");
        let theirs = f.land(Some(base), &[("base", "same"), ("clash/child", "theirs"), ("added", "keep")]);
        f.repo.workspace_create_at("work", &StreamName::new("main").unwrap(), &actor, Some(&ours)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        f.repo.workspace_checkout("work", dir.path()).unwrap();
        // Old versions recorded this pending merge after copying only the fallback tree.
        let pending = serde_json::json!({"theirs":theirs,"paths":["clash"]}).to_string();
        f.repo
            .meta()
            .write(|tx| {
                tx.workspace_update("work", Some(&theirs), Some(&ours))?;
                tx.meta_set("workspace.work.merging", &pending)
            })
            .unwrap();
        std::fs::write(dir.path().join("clash"), "user's unsaved resolution").unwrap();
        let observed = f.repo.workspace_row("work").unwrap();
        let result = f.repo.snapshot("work", dir.path(), "Attempt legacy resolution", &actor, force);
        assert!(result.is_err(), "force={force} must not commit a legacy incomplete tree: {result:?}");
        let message = result.unwrap_err().to_string();
        assert!(message.contains("file/directory") && message.contains("back up"), "{message}");
        let after = f.repo.workspace_row("work").unwrap();
        assert_eq!(after.head, observed.head);
        assert_eq!(after.base, observed.base);
        assert!(f.repo.workspace_is_merging("work").unwrap());
        assert_eq!(f.repo.meta().read(|tx| tx.meta_get("workspace.work.merging")).unwrap(), Some(pending));
        assert_eq!(std::fs::read_to_string(dir.path().join("clash")).unwrap(), "user's unsaved resolution");
        assert!(!dir.path().join("added").exists());
        assert_eq!(f.head(), Some(theirs));
    }
}
