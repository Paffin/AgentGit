//! Private discussion drafts are independent of review decisions and public comments.
mod common;
use ag_integrate::{CommentDraftInput, ReviewDraftInput};
use common::*;
use std::sync::{Arc, Barrier};

#[test]
fn comment_and_review_drafts_are_separate_private_slots() {
    let f = fx();
    let owner = id("user:alice");
    let other = id("user:bob");
    let head = f.snap(None, &[("file", "content")], owner.as_str());
    let proposal = f.propose(head, "change", owner.as_str());
    f.repo
        .save_review_draft(
            &proposal,
            &owner,
            ReviewDraftInput { head: &head, decision: "approve", body: "review notes", expected_version: 0 },
        )
        .unwrap();
    let comment = f
        .repo
        .save_comment_draft(
            &proposal,
            &owner,
            CommentDraftInput { head: &head, body: "private discussion", expected_version: 0 },
        )
        .unwrap();
    assert_eq!(comment.version, 1);
    assert!(f.repo.comment_draft(&proposal, &other).unwrap().is_none());
    assert_eq!(f.repo.review_draft(&proposal, &owner).unwrap().unwrap().body, "review notes");
    f.repo.clear_review_draft(&proposal, &owner, 1).unwrap();
    assert_eq!(f.repo.comment_draft(&proposal, &owner).unwrap().unwrap().body, "private discussion");
    f.repo
        .save_review_draft(
            &proposal,
            &owner,
            ReviewDraftInput { head: &head, decision: "comment", body: "independent", expected_version: 2 },
        )
        .unwrap();
    f.repo.clear_comment_draft(&proposal, &owner, 1).unwrap();
    assert_eq!(f.repo.review_draft(&proposal, &owner).unwrap().unwrap().body, "independent");
    assert!(f.repo.meta().read(|tx| tx.reviews(&proposal)).unwrap().is_empty());
    let audit = f.repo.meta().read(|tx| tx.audit_list(Some(&proposal), 100)).unwrap();
    assert!(
        !audit.iter().any(|row| row.details.contains("private discussion") || row.details.contains("review notes"))
    );
}

#[test]
fn versions_tombstones_and_identical_retries_keep_newer_notes() {
    let f = fx();
    let owner = id("user:alice");
    let head = f.snap(None, &[("file", "content")], owner.as_str());
    let proposal = f.propose(head, "change", owner.as_str());
    assert!(f.repo.clear_comment_draft(&proposal, &owner, 0).unwrap().is_none());
    let input = CommentDraftInput { head: &head, body: "first", expected_version: 0 };
    assert_eq!(f.repo.save_comment_draft(&proposal, &owner, input.clone()).unwrap().version, 1);
    assert_eq!(f.repo.save_comment_draft(&proposal, &owner, input).unwrap().version, 1);
    assert!(
        f.repo
            .save_comment_draft(
                &proposal,
                &owner,
                CommentDraftInput { head: &head, body: "stale", expected_version: 0 }
            )
            .is_err()
    );
    let another_head = f.snap(Some(head), &[("file", "updated")], owner.as_str());
    assert!(
        f.repo
            .save_comment_draft(
                &proposal,
                &owner,
                CommentDraftInput { head: &another_head, body: "first", expected_version: 0 }
            )
            .is_err()
    );
    let cleared = f.repo.clear_comment_draft(&proposal, &owner, 1).unwrap().unwrap();
    assert!(cleared.head.is_none() && cleared.body.is_empty());
    assert_eq!(cleared.version, 2);
    assert_eq!(f.repo.clear_comment_draft(&proposal, &owner, 1).unwrap().unwrap().version, 2);
    assert!(
        f.repo
            .save_comment_draft(
                &proposal,
                &owner,
                CommentDraftInput { head: &head, body: "late device", expected_version: 1 }
            )
            .is_err()
    );
    assert_eq!(
        f.repo
            .save_comment_draft(
                &proposal,
                &owner,
                CommentDraftInput { head: &head, body: "newer", expected_version: 2 }
            )
            .unwrap()
            .version,
        3
    );
    assert!(f.repo.clear_comment_draft(&proposal, &owner, 1).is_err());
    assert_eq!(f.repo.comment_draft(&proposal, &owner).unwrap().unwrap().body, "newer");
}

#[test]
fn closed_proposals_keep_editable_drafts_and_enforce_utf8_byte_limit() {
    let f = fx();
    let owner = id("user:alice");
    let first_run = id("agent:worker#first");
    let second_run = id("agent:worker#second");
    let head = f.snap(None, &[("file", "content")], owner.as_str());
    let proposal = f.propose(head, "change", owner.as_str());
    f.repo.close_proposal(&proposal, &owner).unwrap();
    let body = "é".repeat(8 * 1024);
    let input = CommentDraftInput { head: &head, body: &body, expected_version: 0 };
    assert_eq!(f.repo.save_comment_draft(&proposal, &first_run, input).unwrap().version, 1);
    assert!(f.repo.comment_draft(&proposal, &second_run).unwrap().is_none());
    let too_big = format!("{body}x");
    assert!(
        f.repo
            .save_comment_draft(
                &proposal,
                &first_run,
                CommentDraftInput { head: &head, body: &too_big, expected_version: 1 }
            )
            .is_err()
    );
    assert!(
        f.repo
            .save_comment_draft(
                &proposal,
                &first_run,
                CommentDraftInput { head: &head, body: "bad\0note", expected_version: 1 }
            )
            .is_err()
    );
    assert_eq!(f.repo.comment_draft(&proposal, &first_run).unwrap().unwrap().body, body);
    assert_eq!(f.repo.clear_comment_draft(&proposal, &first_run, 1).unwrap().unwrap().version, 2);
}

#[test]
fn simultaneous_devices_cannot_replace_the_same_comment_version() {
    let f = fx();
    let owner = id("user:alice");
    let head = f.snap(None, &[("file", "content")], owner.as_str());
    let proposal = f.propose(head, "change", owner.as_str());
    let repo = Arc::new(f.repo.with_object_store(|store| store));
    let start = Arc::new(Barrier::new(2));
    let workers = ["first", "second"]
        .into_iter()
        .map(|body| {
            let (repo, start, owner, proposal) = (repo.clone(), start.clone(), owner.clone(), proposal.clone());
            std::thread::spawn(move || {
                start.wait();
                repo.save_comment_draft(&proposal, &owner, CommentDraftInput { head: &head, body, expected_version: 0 })
            })
        })
        .collect::<Vec<_>>();
    let results = workers.into_iter().map(|worker| worker.join().unwrap()).collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(repo.comment_draft(&proposal, &owner).unwrap().unwrap().version, 1);
}
