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
    assert!(merge.message.contains("Merge-Algorithm: ag-merge/1+ag-diff/1"));
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
    let dir = tempfile::tempdir().unwrap();
    let repo = ag_integrate::Repository::init(
        dir.path(),
        &ag_integrate::InitOptions {
            namespace: ag_types::Namespace::new("acme/app").unwrap(),
            default_stream: ag_types::StreamName::new("main").unwrap(),
            actor: id("user:alice"),
        },
    )
    .unwrap();
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
