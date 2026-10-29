//! Private review drafts are independent of submitted reviews.
#![allow(missing_docs)]
mod common;
use ag_integrate::ReviewDraftInput;
use common::*;
use std::sync::{Arc, Barrier};

#[test]
fn private_drafts_compare_versions_and_keep_tombstones() {
    let f = fx();
    let alice = id("user:alice");
    let bob = id("user:bob");
    let head = f.snap(None, &[("file", "content")], alice.as_str());
    let p = f.repo.propose("main", &head, "change", "", &alice, None).unwrap();
    let input = ReviewDraftInput { head: &head, decision: "comment", body: "private notes", expected_version: 0 };
    let saved = f.repo.save_review_draft(&p.id, &bob, input.clone()).unwrap();
    assert_eq!(saved.version, 1);
    assert_eq!(saved.body, "private notes");
    assert!(f.repo.review_draft(&p.id, &alice).unwrap().is_none());
    assert_eq!(f.repo.save_review_draft(&p.id, &bob, input).unwrap().version, 1);
    assert!(
        f.repo
            .save_review_draft(
                &p.id,
                &bob,
                ReviewDraftInput { head: &head, decision: "comment", body: "stale", expected_version: 0 }
            )
            .is_err()
    );
    let cleared = f.repo.clear_review_draft(&p.id, &bob, 1).unwrap().unwrap();
    assert_eq!(cleared.version, 2);
    assert!(cleared.head.is_none());
    assert!(cleared.body.is_empty());
    assert!(
        f.repo
            .save_review_draft(
                &p.id,
                &bob,
                ReviewDraftInput { head: &head, decision: "comment", body: "old device", expected_version: 1 }
            )
            .is_err()
    );
    let replacement = f
        .repo
        .save_review_draft(
            &p.id,
            &bob,
            ReviewDraftInput { head: &head, decision: "approve", body: "checked", expected_version: 2 },
        )
        .unwrap();
    assert_eq!(replacement.version, 3);
    assert!(f.repo.clear_review_draft(&p.id, &bob, 2).is_err());
    assert_eq!(f.repo.review_draft(&p.id, &bob).unwrap().unwrap().body, "checked");
    assert!(f.repo.meta().read(|tx| tx.reviews(&p.id)).unwrap().is_empty());
    let audit = f.repo.meta().read(|tx| tx.audit_list(Some(&p.id), 100)).unwrap();
    assert!(!audit.iter().any(|row| row.details.contains("private notes") || row.details.contains("checked")));
}

#[test]
fn two_devices_cannot_overwrite_the_same_draft_version() {
    let f = fx();
    let alice = id("user:alice");
    let reviewer = id("user:bob");
    let head = f.snap(None, &[("file", "content")], alice.as_str());
    let p = f.repo.propose("main", &head, "change", "", &alice, None).unwrap();
    let repo = Arc::new(f.repo.with_object_store(|store| store));
    let start = Arc::new(Barrier::new(2));
    let threads: Vec<_> = ["first device", "second device"]
        .into_iter()
        .map(|body| {
            let repo = repo.clone();
            let start = start.clone();
            let owner = reviewer.clone();
            let proposal = p.id.clone();
            std::thread::spawn(move || {
                start.wait();
                repo.save_review_draft(
                    &proposal,
                    &owner,
                    ReviewDraftInput { head: &head, decision: "comment", body, expected_version: 0 },
                )
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|thread| thread.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let saved = f.repo.review_draft(&p.id, &reviewer).unwrap().unwrap();
    assert_eq!(saved.version, 1);
    assert!(saved.body == "first device" || saved.body == "second device");
}

#[test]
fn draft_limits_and_closed_proposals_do_not_discard_private_notes() {
    let f = fx();
    let author = id("user:alice");
    let reviewer = id("user:bob");
    let head = f.snap(None, &[("file", "content")], author.as_str());
    let p = f.repo.propose("main", &head, "change", "", &author, None).unwrap();
    let body = "é".repeat(32 * 1024);
    assert_eq!(
        f.repo
            .save_review_draft(
                &p.id,
                &reviewer,
                ReviewDraftInput { head: &head, decision: "comment", body: &body, expected_version: 0 }
            )
            .unwrap()
            .version,
        1
    );
    let oversized = format!("{body}x");
    assert!(
        f.repo
            .save_review_draft(
                &p.id,
                &reviewer,
                ReviewDraftInput { head: &head, decision: "comment", body: &oversized, expected_version: 1 }
            )
            .is_err()
    );
    assert!(
        f.repo
            .save_review_draft(
                &p.id,
                &reviewer,
                ReviewDraftInput { head: &head, decision: "publish", body: "", expected_version: 1 }
            )
            .is_err()
    );
    f.repo.close_proposal(&p.id, &author).unwrap();
    assert!(
        f.repo
            .save_review_draft(
                &p.id,
                &reviewer,
                ReviewDraftInput { head: &head, decision: "approve", body: "closed", expected_version: 1 }
            )
            .is_err()
    );
    assert_eq!(f.repo.review_draft(&p.id, &reviewer).unwrap().unwrap().body, body);
    assert!(f.repo.clear_review_draft(&p.id, &reviewer, 1).unwrap().unwrap().head.is_none());
}

#[test]
fn agent_runs_do_not_share_private_drafts() {
    let f = fx();
    let author = id("user:alice");
    let first = id("agent:reviewer#run-a");
    let second = id("agent:reviewer#run-b");
    let head = f.snap(None, &[("file", "content")], author.as_str());
    let p = f.repo.propose("main", &head, "change", "", &author, None).unwrap();
    f.repo
        .save_review_draft(
            &p.id,
            &first,
            ReviewDraftInput { head: &head, decision: "comment", body: "run a", expected_version: 0 },
        )
        .unwrap();
    assert!(f.repo.review_draft(&p.id, &second).unwrap().is_none());
    assert!(f.repo.clear_review_draft(&p.id, &second, 0).unwrap().is_none());
    assert_eq!(f.repo.review_draft(&p.id, &first).unwrap().unwrap().body, "run a");
}
