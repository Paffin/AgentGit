//! Requested reviewers are durable routing, not approval or additional access.
#![allow(missing_docs)]
mod common;
use common::*;

#[test]
fn reviewer_requests_are_sorted_retryable_and_compare_the_inspected_set() {
    let f = fx();
    let author = id("user:alice");
    let head = f.snap(None, &[("file", "content")], author.as_str());
    let proposal = f.repo.propose("main", &head, "review", "", &author, None).unwrap();
    assert!(f.repo.requested_reviewers(&proposal.id).unwrap().is_empty());
    let desired = [id("user:carol"), id("user:bob")];
    let result = f.repo.set_requested_reviewers(&proposal.id, &desired, &[], &author).unwrap();
    assert_eq!(result, ["user:bob", "user:carol"]);
    assert_eq!(f.repo.requested_reviewers(&proposal.id).unwrap(), result);
    // A lost response can be retried without another audit event.
    assert_eq!(f.repo.set_requested_reviewers(&proposal.id, &desired, &[], &author).unwrap(), result);
    assert!(f.repo.set_requested_reviewers(&proposal.id, &[], &[], &author).is_err());
    assert_eq!(f.repo.requested_reviewers(&proposal.id).unwrap(), result);
    let events = f.repo.meta().read(|tx| tx.audit_list(Some(&proposal.id), 100)).unwrap();
    assert_eq!(events.iter().filter(|e| e.action == "proposal.reviewers").count(), 1);
    assert!(f.repo.meta().read(|tx| tx.reviews(&proposal.id)).unwrap().is_empty());
    f.repo.set_requested_reviewers(&proposal.id, &[], &desired, &author).unwrap();
    assert!(f.repo.requested_reviewers(&proposal.id).unwrap().is_empty());
}

#[test]
fn only_the_author_can_request_other_reviewers_on_an_open_proposal() {
    let f = fx();
    let author = id("user:alice");
    let head = f.snap(None, &[("file", "content")], author.as_str());
    let proposal = f.repo.propose("main", &head, "review", "", &author, None).unwrap();
    assert!(f.repo.set_requested_reviewers(&proposal.id, &[id("user:carol")], &[], &id("user:bob")).is_err());
    assert!(f.repo.set_requested_reviewers(&proposal.id, std::slice::from_ref(&author), &[], &author).is_err());
    let many = (0..21).map(|i| id(&format!("user:u{i}"))).collect::<Vec<_>>();
    assert!(f.repo.set_requested_reviewers(&proposal.id, &many, &[], &author).is_err());
    assert!(f.repo.requested_reviewers(&proposal.id).unwrap().is_empty());
    f.repo.close_proposal(&proposal.id, &author).unwrap();
    assert!(f.repo.set_requested_reviewers(&proposal.id, &[id("user:bob")], &[], &author).is_err());
}

#[test]
fn routing_uses_principals_without_crossing_run_ownership() {
    let f = fx();
    let author = id("agent:builder#run-a");
    let head = f.snap(None, &[("file", "content")], author.as_str());
    let p = f.repo.propose("main", &head, "review", "", &author, None).unwrap();
    assert!(f.repo.set_requested_reviewers(&p.id, &[id("user:bob")], &[], &id("agent:builder#run-b")).is_err());
    assert!(f.repo.set_requested_reviewers(&p.id, &[id("agent:builder#run-b")], &[], &author).is_err());
    assert_eq!(
        f.repo.set_requested_reviewers(&p.id, &[id("agent:reviewer#run-a")], &[], &author).unwrap(),
        ["agent:reviewer"]
    );
}

#[test]
fn requested_reviewer_filter_is_applied_before_keyset_paging() {
    let f = fx();
    let author = id("user:alice");
    let head = f.snap(None, &[("file", "content")], author.as_str());
    let requested = f.repo.propose("main", &head, "requested", "", &author, None).unwrap();
    f.repo.set_requested_reviewers(&requested.id, &[id("user:bob")], &[], &author).unwrap();
    f.repo.propose("main", &head, "not requested", "", &author, None).unwrap();
    let rows = f
        .repo
        .meta()
        .read(|tx| {
            tx.proposals_page(
                &ag_storage::meta::ProposalFilter { requested_reviewer: Some("user:bob"), ..Default::default() },
                None,
                1,
            )
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, requested.id);
}
