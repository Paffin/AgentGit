//! End-to-end tests of publication and integration.
#![allow(missing_docs)]

mod common;

use std::sync::Arc;

use ag_integrate::{IntegrateRequest, Outcome, Strategy};
use ag_storage::meta::IntentState;
use common::*;

#[test]
fn publish_makes_history_visible_and_is_idempotent() {
    let f = fx();
    let c1 = f.snap(None, &[("a", "1")], "user:alice");
    let c2 = f.snap(Some(c1), &[("a", "1"), ("b", "2")], "user:alice");
    assert!(!f.repo.meta().read(|tx| tx.is_ready(&c2)).unwrap(), "stored but not published");

    let r = f.repo.publish(&c2, &id("user:alice"), None).unwrap();
    assert_eq!(r.state, IntentState::Published);
    assert_eq!(r.new_changesets, 2);
    assert!(f.repo.meta().read(|tx| tx.is_ready(&c1)).unwrap());
    assert!(f.repo.meta().read(|tx| tx.is_ready(&c2)).unwrap());

    // repeating is a replay: same operation, nothing new
    let again = f.repo.publish(&c2, &id("user:alice"), None).unwrap();
    assert_eq!(again.operation_id, r.operation_id);
    assert_eq!(f.repo.meta().read(|tx| tx.count_changesets()).unwrap(), 2);

    // publishing an already-published head is a no-op publication
    let noop = f.repo.publish(&c1, &id("user:alice"), Some("other-key-1")).unwrap();
    assert_eq!((noop.state, noop.new_changesets), (IntentState::Published, 0));
}

#[test]
fn publish_rejects_unusable_histories() {
    let f = fx();
    let alice = id("user:alice");

    // 1. a missing blob
    let ghost_blob = ag_object::hash_blob(b"never stored");
    let bad = f
        .repo
        .commit(
            vec![],
            &alice,
            "refers to a blob that was never uploaded",
            vec![ag_changeset::Op::Put {
                path: ag_types::RepoPath::new("x").unwrap(),
                content: ghost_blob,
                file_type: ag_types::FileType::Regular,
                executable: false,
            }],
        )
        .unwrap();
    let r = f.repo.publish(&bad, &alice, None).unwrap();
    assert_eq!(r.state, IntentState::Rejected);
    assert!(r.reason.unwrap().contains("missing"), "missing blob must be reported");
    assert!(!f.repo.meta().read(|tx| tx.is_ready(&bad)).unwrap());

    // 2. operations that do not apply to the parent (deleting a file that is not there)
    let good = f.snap(None, &[("a", "1")], "user:alice");
    let broken = f
        .repo
        .commit(
            vec![good],
            &alice,
            "bad delete",
            vec![ag_changeset::Op::Delete { path: ag_types::RepoPath::new("nope").unwrap() }],
        )
        .unwrap();
    let r = f.repo.publish(&broken, &alice, None).unwrap();
    assert_eq!(r.state, IntentState::Rejected);
    assert!(r.reason.unwrap().contains("does not apply"));

    // 3. a no-op put (non-minimal delta)
    let blob = f.repo.put_blob(b"1".to_vec()).unwrap();
    let noop = f
        .repo
        .commit(
            vec![good],
            &alice,
            "redundant put",
            vec![ag_changeset::Op::Put {
                path: ag_types::RepoPath::new("a").unwrap(),
                content: blob,
                file_type: ag_types::FileType::Regular,
                executable: false,
            }],
        )
        .unwrap();
    assert_eq!(f.repo.publish(&noop, &alice, None).unwrap().state, IntentState::Rejected);

    // 4. a head that is not in the store at all
    let nowhere = ag_types::ChangesetId::new(ag_types::ObjectId::from_bytes([7; 32]));
    let r = f.repo.publish(&nowhere, &alice, None).unwrap();
    assert_eq!(r.state, IntentState::Rejected);

    // nothing leaked into the published index except the good changeset's absence of publication
    assert_eq!(f.repo.meta().read(|tx| tx.count_changesets()).unwrap(), 0);
}

#[test]
fn first_integration_fast_forwards_an_empty_stream() {
    let f = fx();
    let c = f.snap(None, &[("README", "hello")], "user:alice");
    let p = f.propose(c, "initial", "user:alice");
    let r = f.integrate(&p, "user:alice");
    match r.outcome {
        Outcome::Integrated { new_head, previous_head, version, strategy, .. } => {
            assert_eq!(new_head, c);
            assert_eq!(previous_head, None);
            assert_eq!(version, 1);
            assert_eq!(strategy, Strategy::FastForward);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(r.state, IntentState::Integrated);
    assert_eq!(f.head(), Some(c));
    let prop = f.repo.meta().read(|tx| tx.proposal(&p)).unwrap().unwrap();
    assert_eq!(prop.state, "integrated");
    assert_eq!(prop.integrated_as, Some(c));
}

#[test]
fn independent_proposals_merge_with_the_head_as_first_parent() {
    let f = fx();
    let base = f.land(None, &[("a.txt", "a\n"), ("b.txt", "b\n")]);
    let pa = f.snap(Some(base), &[("a.txt", "A\n"), ("b.txt", "b\n")], "user:alice");
    let pb = f.snap(Some(base), &[("a.txt", "a\n"), ("b.txt", "B\n")], "user:bob");
    let prop_a = f.propose(pa, "edit a", "user:alice");
    let prop_b = f.propose(pb, "edit b", "user:bob");

    let ra = f.integrate(&prop_a, "user:alice");
    assert!(matches!(ra.outcome, Outcome::Integrated { strategy: Strategy::FastForward, .. }));
    let rb = f.integrate(&prop_b, "user:alice");
    let Outcome::Integrated { new_head, previous_head, strategy, version, changed_files, .. } = rb.outcome else {
        panic!("{:?}", rb.outcome)
    };
    assert_eq!(strategy, Strategy::Merge);
    assert_eq!(previous_head, Some(pa));
    assert_eq!(version, 3);
    assert_eq!(changed_files, 1, "relative to the previous head only b.txt changed");

    let merge = f.repo.changeset(&new_head).unwrap();
    assert_eq!(merge.parents, vec![pa, pb], "first parent is the previous stream head");
    assert!(merge.message.contains("Merge-Algorithm: ag-merge/2+ag-diff/1"));
    assert_eq!(f.read(&new_head, "a.txt").as_deref(), Some("A\n"));
    assert_eq!(f.read(&new_head, "b.txt").as_deref(), Some("B\n"));

    // the whole thing is auditable: reflog, audit chain, derived trees
    let log = f.repo.meta().read(|tx| tx.reflog("main", 10)).unwrap();
    assert_eq!(log.len(), 3);
    assert_eq!(log[0].new_head, Some(new_head));
    assert!(f.repo.fsck().unwrap().is_ok());
}

#[test]
fn overlapping_edits_are_reported_as_conflicts_and_nothing_changes() {
    let f = fx();
    let base = f.land(None, &[("f.txt", "one\ntwo\nthree\n")]);
    let pa = f.snap(Some(base), &[("f.txt", "one\nTWO-A\nthree\n")], "user:alice");
    let pb = f.snap(Some(base), &[("f.txt", "one\nTWO-B\nthree\n")], "user:bob");
    let prop_a = f.propose(pa, "a", "user:alice");
    let prop_b = f.propose(pb, "b", "user:bob");
    f.integrate(&prop_a, "user:alice");
    let before = (f.head(), f.version());

    let r = f.integrate(&prop_b, "user:alice");
    assert_eq!(r.state, IntentState::Rejected);
    let Outcome::Conflict { conflicts, base: b } = r.outcome else { panic!("{:?}", r.outcome) };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, "f.txt");
    assert_eq!(conflicts[0].kind, "content", "a stable code, not prose");
    assert!(conflicts[0].message.starts_with("content conflict"), "{}", conflicts[0].message);
    assert_eq!(b, Some(base));
    assert_eq!((f.head(), f.version()), before, "a conflict never moves the stream");
    // the proposal stays open so it can be fixed
    assert_eq!(f.repo.meta().read(|tx| tx.proposal(&prop_b)).unwrap().unwrap().state, "open");
}

#[test]
fn merge_preview_has_no_side_effects() {
    let f = fx();
    let base = f.land(None, &[("a", "1\n"), ("b", "1\n")]);
    let p1 = f.snap(Some(base), &[("a", "2\n"), ("b", "1\n")], "user:alice");
    let p2 = f.snap(Some(base), &[("a", "1\n"), ("b", "2\n")], "user:bob");
    let prop1 = f.propose(p1, "1", "user:alice");
    let prop2 = f.propose(p2, "2", "user:bob");
    f.integrate(&prop1, "user:alice");
    let before = (f.head(), f.version());
    let intents_before = f.repo.meta().read(|tx| tx.intents_recent(100)).unwrap().len();

    let pv = f.repo.merge_preview(&prop2).unwrap();
    assert_eq!(pv.strategy, Strategy::Merge);
    assert!(pv.conflicts.is_empty());
    assert_eq!(pv.changed_files, vec!["b".to_string()]);
    let tree_id = pv.candidate_tree.unwrap();

    assert_eq!((f.head(), f.version()), before);
    assert_eq!(f.repo.meta().read(|tx| tx.intents_recent(100)).unwrap().len(), intents_before);

    // and the real integration produces exactly the previewed tree
    let r = f.integrate(&prop2, "user:alice");
    let Outcome::Integrated { tree_id: got, .. } = r.outcome else { panic!() };
    assert_eq!(got, tree_id);
}

#[test]
fn integrating_an_already_contained_proposal_is_a_noop() {
    let f = fx();
    let c1 = f.land(None, &[("a", "1")]);
    let c2 = f.land(Some(c1), &[("a", "1"), ("b", "2")]);
    // a proposal pointing at c1, which the stream already contains
    let old = f.repo.propose("main", &c1, "stale", "", &id("user:alice"), None).unwrap();
    let r = f.integrate(&old.id, "user:alice");
    let Outcome::Integrated { strategy, version, .. } = r.outcome else { panic!() };
    assert_eq!(strategy, Strategy::AlreadyIntegrated);
    assert_eq!(version, 2);
    assert_eq!(f.head(), Some(c2));
    assert_eq!(f.version(), 2, "no CAS happened");
}

#[test]
fn unrelated_histories_are_refused() {
    let f = fx();
    f.land(None, &[("a", "1")]);
    let other_root = f.snap(None, &[("z", "9")], "user:bob");
    let p = f.propose(other_root, "unrelated", "user:bob");
    let r = f.integrate(&p, "user:alice");
    assert!(matches!(r.outcome, Outcome::Invalid { .. }), "{:?}", r.outcome);
}

#[test]
fn idempotent_replay_and_unknown_outcome_recovery() {
    let f = fx();
    let c = f.snap(None, &[("a", "1")], "user:alice");
    let p = f.propose(c, "t", "user:alice");
    let req = IntegrateRequest {
        proposal_id: p.clone(),
        actor: id("user:alice"),
        idempotency_key: Some("client-key-1".into()),
        fencing_tokens: vec![],
    };

    // The server commits, but the client never sees the response.
    f.repo.failpoints().arm("integrate.after_commit");
    let lost = f.repo.integrate(&req);
    assert!(matches!(lost, Err(ag_integrate::Error::Injected(_))));

    // The client asks for the outcome instead of blindly re-integrating.
    let found = f.repo.operation_by_key("client-key-1").unwrap();
    assert_eq!(found.state, IntentState::Integrated);
    let Outcome::Integrated { version, .. } = found.outcome else { panic!() };
    assert_eq!(version, 1);

    // Retrying with the same key is a replay: same operation, no second CAS.
    let again = f.repo.integrate(&req).unwrap();
    assert_eq!(again.operation_id, found.operation_id);
    assert_eq!(f.version(), 1);
    assert_eq!(f.repo.meta().read(|tx| tx.reflog("main", 10)).unwrap().len(), 1);
    assert_eq!(f.repo.operation(&found.operation_id).unwrap(), again);
}

#[test]
fn reused_idempotency_key_for_a_different_request_is_refused() {
    let f = fx();
    let c1 = f.snap(None, &[("a", "1")], "user:alice");
    let c2 = f.snap(None, &[("b", "1")], "user:alice");
    let p1 = f.propose(c1, "1", "user:alice");
    let p2 = f.propose(c2, "2", "user:alice");
    let mk = |p: &str| IntegrateRequest {
        proposal_id: p.into(),
        actor: id("user:alice"),
        idempotency_key: Some("same-key".into()),
        fencing_tokens: vec![],
    };
    f.repo.integrate(&mk(&p1)).unwrap();
    assert!(f.repo.integrate(&mk(&p2)).is_err());
}

#[test]
fn crash_at_every_step_recovers_to_exactly_one_integration() {
    for point in [
        "publish.after_upload",
        "publish.after_verify",
        "publish.before_commit",
        "publish.after_commit",
        "integrate.after_ready",
        "integrate.before_commit",
        "integrate.after_commit",
    ] {
        let f = fx();
        let c = f.snap(None, &[("a", "1")], "user:alice");
        // `propose` publishes; arm publish failpoints before it, integrate ones after.
        let prop_id;
        if point.starts_with("publish") {
            f.repo.failpoints().arm(point);
            let first = f.repo.propose("main", &c, "t", "", &id("user:alice"), None);
            assert!(first.is_err(), "{point}: propose must fail");
            // restart: recovery finishes the publication
            let rep = f.repo.recover(3_600_000).unwrap();
            assert!(rep.errors.is_empty(), "{point}: {rep:?}");
            assert!(f.repo.meta().read(|tx| tx.is_ready(&c)).unwrap(), "{point}: head published after recovery");
            prop_id = f.repo.propose("main", &c, "t", "", &id("user:alice"), None).unwrap().id;
        } else {
            prop_id = f.propose(c, "t", "user:alice");
            f.repo.failpoints().arm(point);
            let req = IntegrateRequest {
                proposal_id: prop_id.clone(),
                actor: id("user:alice"),
                idempotency_key: None,
                fencing_tokens: vec![],
            };
            assert!(f.repo.integrate(&req).is_err(), "{point}: integrate must fail");
            let rep = f.repo.recover(3_600_000).unwrap();
            assert!(rep.errors.is_empty(), "{point}: {rep:?}");
        }
        let r = f.repo.integrate(&IntegrateRequest {
            proposal_id: prop_id.clone(),
            actor: id("user:alice"),
            idempotency_key: None,
            fencing_tokens: vec![],
        });
        // after recovery the proposal is either already integrated (replay) or integrates now
        match r {
            Ok(res) => assert!(matches!(res.outcome, Outcome::Integrated { .. }), "{point}: {:?}", res.outcome),
            Err(ag_integrate::Error::Conflict(m)) => assert!(m.contains("integrated"), "{point}: {m}"),
            Err(e) => panic!("{point}: {e}"),
        }
        assert_eq!(f.head(), Some(c), "{point}");
        assert_eq!(f.version(), 1, "{point}: exactly one stream update");
        assert_eq!(f.repo.meta().read(|tx| tx.reflog("main", 10)).unwrap().len(), 1, "{point}");
        assert!(f.repo.fsck().unwrap().is_ok(), "{point}: {:?}", f.repo.fsck().unwrap().issues);
    }
}

#[test]
fn stale_operations_expire_and_waiting_ones_are_left_alone() {
    let f = fx();
    let c = f.snap(None, &[("a", "1")], "user:alice");
    let p = f.propose(c, "t", "user:alice");
    f.repo.set_policy("main", r#"{"approvals":{"min":1}}"#, &id("user:alice")).unwrap();
    let r = f.integrate(&p, "user:alice");
    assert_eq!(r.state, IntentState::Blocked);

    let rep = f.repo.recover(3_600_000).unwrap();
    assert_eq!(rep.waiting, vec![r.operation_id.clone()]);
    f.clock.advance_ms(10_000_000);
    let rep = f.repo.recover(3_600_000).unwrap();
    assert_eq!(rep.aborted, vec![r.operation_id.clone()]);
    assert_eq!(f.repo.operation(&r.operation_id).unwrap().state, IntentState::Aborted);
    // approving later and trying again starts a fresh attempt and succeeds
    f.repo.review(&p, &id("user:carol"), "approve", "ok").unwrap();
    assert!(matches!(f.integrate(&p, "user:alice").outcome, Outcome::Integrated { .. }));
}

#[test]
fn concurrent_integrations_never_lose_an_update() {
    let dir = tempdir();
    let repo = init_repo(&dir);
    let base = snap_in(&repo, None, &[("README", "base\n")], "user:alice");
    let p0 = repo.propose("main", &base, "base", "", &id("user:alice"), None).unwrap();
    repo.integrate(&IntegrateRequest {
        proposal_id: p0.id,
        actor: id("user:alice"),
        idempotency_key: None,
        fencing_tokens: vec![],
    })
    .unwrap();

    // 12 agents each add their own file on top of the same base and integrate at once.
    const N: usize = 12;
    let proposals: Vec<String> = (0..N)
        .map(|i| {
            let c = snap_in(
                &repo,
                Some(base),
                &[("README", "base\n"), (&format!("file{i}.txt"), &format!("content {i}\n"))],
                &format!("user:dev{i}"),
            );
            repo.propose("main", &c, &format!("add file{i}"), "", &id(&format!("user:dev{i}")), None).unwrap().id
        })
        .collect();

    let handles: Vec<_> = proposals
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let r = repo.reopen().unwrap();
            std::thread::spawn(move || {
                for _ in 0..50 {
                    match r.integrate(&IntegrateRequest {
                        proposal_id: p.clone(),
                        actor: id("user:alice"),
                        idempotency_key: None,
                        fencing_tokens: vec![],
                    }) {
                        Ok(res) => return (i, res),
                        Err(ag_integrate::Error::Contention(_)) => continue,
                        Err(e) => panic!("worker {i}: {e}"),
                    }
                }
                panic!("worker {i} starved");
            })
        })
        .collect();
    for h in handles {
        let (i, res) = h.join().unwrap();
        assert!(matches!(res.outcome, Outcome::Integrated { .. }), "worker {i}: {:?}", res.outcome);
    }

    let st = repo.meta().read(|tx| tx.stream("main")).unwrap().unwrap().state;
    assert_eq!(st.version, 1 + N as u64, "every proposal produced exactly one stream update");
    let tree = repo.tree_of(&st.head.unwrap()).unwrap();
    assert_eq!(tree.len(), 1 + N, "no file was lost");
    for i in 0..N {
        assert!(tree.get(&ag_types::RepoPath::new(format!("file{i}.txt")).unwrap()).is_some(), "file{i}.txt missing");
    }
    let log = repo.meta().read(|tx| tx.reflog("main", 100)).unwrap();
    let mut chain: Vec<_> = log.into_iter().rev().collect();
    let mut prev = None;
    for e in chain.drain(..) {
        assert_eq!(e.old_head, prev, "reflog must be a contiguous chain");
        prev = e.new_head;
    }
    let rep = repo.fsck().unwrap();
    assert!(rep.is_ok(), "{:?}", rep.issues);
    // history of the final head contains every proposal's changeset
    let all = ag_dag::ancestors(&repo.graph(), &[st.head.unwrap()], 1000).unwrap();
    assert_eq!(all.len(), 2 * N, "base + N proposals + (N-1) merges");
    let _ = Arc::new(());
}

#[test]
fn the_backends_under_test_are_the_ones_the_environment_selects() {
    // Guards the parametrization itself: if this passed while the suites silently ran on the
    // defaults, a green PostgreSQL / S3 run would prove nothing.
    let want_pg = ag_storage::testing::postgres_url().is_some();
    let want_s3 = ag_storage::testing::s3_endpoint().is_some();
    let f = fx();
    assert_eq!(meta_is_postgres(&f.repo), want_pg);
    assert_eq!(f.wallclock_objects(), want_s3);
    assert_eq!(f.repo.objects().durability().name == "s3-compatible", want_s3);

    let dir = tempdir();
    let repo = init_repo(&dir);
    assert_eq!(meta_is_postgres(&repo), want_pg);
    assert_eq!(objects_are_s3(&repo), want_s3);
    assert_eq!(repo.config().storage.is_some(), want_pg || want_s3);
    // the configuration names environment variables, never holds a secret
    let json = std::fs::read_to_string(dir.path().join(".ag/config.json")).unwrap();
    assert!(!json.contains("password") && !json.contains("secret_access_key\""), "{json}");
}

#[test]
fn a_rename_and_a_concurrent_edit_integrate_as_one_merged_file() {
    let f = fx();
    let body: String = (0..12).map(|i| format!("line {i}\n")).collect();
    let base = f.land(None, &[("src/old.rs", &body), ("README", "r\n")]);
    // alice moves the file; bob (from the same base) edits its last line.
    let moved = f.snap(Some(base), &[("src/new.rs", &body), ("README", "r\n")], "user:alice");
    let edited_body = body.replace("line 11", "line 11 (bob)");
    let edited = f.snap(Some(base), &[("src/old.rs", &edited_body), ("README", "r\n")], "user:bob");
    let prop_moved = f.propose(moved, "move", "user:alice");
    let prop_edited = f.propose(edited, "edit", "user:bob");
    f.integrate(&prop_moved, "user:alice");

    let r = f.integrate(&prop_edited, "user:alice");
    let Outcome::Integrated { new_head, strategy, .. } = r.outcome else { panic!("{:?}", r.outcome) };
    assert_eq!(strategy, Strategy::Merge);
    assert_eq!(f.read(&new_head, "src/new.rs").as_deref(), Some(edited_body.as_str()));
    assert_eq!(f.read(&new_head, "src/old.rs"), None, "the file moved");
    assert!(f.repo.changeset(&new_head).unwrap().message.contains("Merge-Algorithm: ag-merge/2"));
    assert!(f.repo.fsck().unwrap().is_ok());
}

#[test]
fn a_rename_conflict_is_reported_at_the_new_path() {
    let f = fx();
    let base = f.land(None, &[("a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n")]);
    let moved = f.snap(Some(base), &[("b.txt", "1\n2\nOURS\n4\n5\n6\n7\n8\n")], "user:alice");
    let edited = f.snap(Some(base), &[("a.txt", "1\n2\nTHEIRS\n4\n5\n6\n7\n8\n")], "user:bob");
    let prop_moved = f.propose(moved, "move+edit", "user:alice");
    let prop_edited = f.propose(edited, "edit", "user:bob");
    f.integrate(&prop_moved, "user:alice");
    let before = (f.head(), f.version());
    let r = f.integrate(&prop_edited, "user:alice");
    let Outcome::Conflict { conflicts, .. } = r.outcome else { panic!("{:?}", r.outcome) };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, "b.txt");
    assert_eq!((f.head(), f.version()), before);
}

#[test]
fn every_kind_of_conflict_has_a_stable_code_on_the_wire() {
    // both sides add a different file at the same path
    let f = fx();
    let base = f.land(None, &[("keep", "1\n")]);
    let a = f.snap(Some(base), &[("keep", "1\n"), ("new.txt", "mine\n")], "user:alice");
    let b = f.snap(Some(base), &[("keep", "1\n"), ("new.txt", "yours\n")], "user:bob");
    let (pa, pb) = (f.propose(a, "a", "user:alice"), f.propose(b, "b", "user:bob"));
    f.integrate(&pa, "user:alice");
    let Outcome::Conflict { conflicts, .. } = f.integrate(&pb, "user:alice").outcome else { panic!() };
    assert_eq!((conflicts[0].kind.as_str(), conflicts[0].path.as_str()), ("both_added", "new.txt"));
    assert!(conflicts[0].message.contains("both sides added"), "{}", conflicts[0].message);

    // one side deletes a file the other modifies
    let f = fx();
    let base = f.land(None, &[("f.txt", "1\n2\n3\n"), ("other", "x\n")]);
    let deleted = f.snap(Some(base), &[("other", "x\n")], "user:alice");
    let modified = f.snap(Some(base), &[("f.txt", "1\nTWO\n3\n"), ("other", "x\n")], "user:bob");
    let (pd, pm) = (f.propose(deleted, "del", "user:alice"), f.propose(modified, "mod", "user:bob"));
    f.integrate(&pd, "user:alice");
    let Outcome::Conflict { conflicts, .. } = f.integrate(&pm, "user:alice").outcome else { panic!() };
    assert_eq!((conflicts[0].kind.as_str(), conflicts[0].path.as_str()), ("delete_modify", "f.txt"));

    // the codes are a closed, documented set; a message never stands in for one
    let all = [
        "content",
        "both_added",
        "delete_modify",
        "binary",
        "type_change",
        "too_large",
        "file_directory",
        "rename_rename",
        "rename_delete",
    ];
    for c in &conflicts {
        assert!(all.contains(&c.kind.as_str()), "{}", c.kind);
    }
}

#[test]
fn stored_outcomes_without_a_message_still_read() {
    // operation outcomes are stored as JSON; those written before `message` existed have no such field
    let old: ag_integrate::ConflictInfo =
        serde_json::from_str(r#"{"path":"f.txt","kind":"content conflict (1 region(s))"}"#).unwrap();
    assert_eq!((old.path.as_str(), old.message.as_str()), ("f.txt", ""));
}

/// Every committed write transaction is a durable flush, so the cost of an integration is the number of them. The
/// operation record and its move to `ready` are one commit, the tokens are not rewritten when they did not change,
/// and the post-commit derive pass acknowledges its events, caches the new head's tree and prunes in one commit:
/// a fast-forward costs the record, the stream update and that derive commit.
#[test]
fn an_integration_is_a_few_durable_commits() {
    let f = fx();
    let c = f.snap(None, &[("a", "1")], "user:alice");
    let p = f.propose(c, "first", "user:alice");
    let before = f.repo.meta().committed_writes();
    let r = f.integrate(&p, "user:alice");
    let ff = f.repo.meta().committed_writes() - before;
    assert!(matches!(r.outcome, Outcome::Integrated { strategy: Strategy::FastForward, .. }), "{:?}", r.outcome);
    assert_eq!(
        ff, 3,
        "operation record, stream update, derive pass (it was 7: record, tokens, ready, update, tree, ack, prune)"
    );

    // A three-way merge: two proposals on the same base.
    let base = f.head().unwrap();
    let left = f.snap(Some(base), &[("a", "1"), ("l", "1")], "user:alice");
    let right = f.snap(Some(base), &[("a", "1"), ("r", "1")], "user:bob");
    let (pl, pr) = (f.propose(left, "left", "user:alice"), f.propose(right, "right", "user:bob"));
    f.integrate(&pl, "user:alice");
    let before = f.repo.meta().committed_writes();
    let r = f.integrate(&pr, "user:bob");
    let merge = f.repo.meta().committed_writes() - before;
    assert!(matches!(r.outcome, Outcome::Integrated { strategy: Strategy::Merge, .. }), "{:?}", r.outcome);
    assert_eq!(merge, 3, "the merge changeset and its events ride in the stream-update and derive commits (it was 8)");
}

/// An operation recorded by an earlier build is still in the state `prepared` (it wrote `prepared` and `ready` as two
/// commits): recovery must carry it through, as it carries one that stopped between the two.
#[test]
fn an_operation_left_prepared_by_an_earlier_build_is_still_recovered() {
    use ag_storage::meta::NewIntent;
    let f = fx();
    let c = f.snap(None, &[("a", "1")], "user:alice");
    let p = f.propose(c, "t", "user:alice");
    let payload = serde_json::json!({"proposal": p, "stream": "main", "head": c, "tokens": []});
    f.repo
        .meta()
        .write(|tx| {
            tx.intent_create(&NewIntent {
                operation_id: "op_old",
                idempotency_key: "old-key",
                request_hash: "hash-of-the-old-request",
                kind: "integrate",
                actor: "user:alice",
                stream: Some("main"),
                payload: &payload,
            })
            .map(|_| ())
        })
        .unwrap();
    assert_eq!(f.repo.operation("op_old").unwrap().state, IntentState::Prepared);
    let rep = f.repo.recover(3_600_000).unwrap();
    assert!(rep.errors.is_empty(), "{rep:?}");
    assert_eq!(f.repo.operation("op_old").unwrap().state, IntentState::Integrated);
    assert_eq!(f.head(), Some(c));
    assert_eq!(f.version(), 1);
}
