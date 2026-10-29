//! Explicit workspace starting points are committed with their creation.
#![allow(missing_docs)]
mod common;
use ag_types::StreamName;
use common::*;

#[test]
fn workspace_creation_records_its_explicit_base_and_head_atomically() {
    let f = fx();
    let start = f.snap(None, &[("seed.txt", "existing content")], "user:alice");
    let row = f
        .repo
        .workspace_create_at("from-revision", &StreamName::new("main").unwrap(), &id("user:alice"), Some(&start))
        .unwrap();
    assert_eq!(row.base, Some(start));
    assert_eq!(row.head, Some(start));
    let stored = f.repo.workspace_row("from-revision").unwrap();
    assert_eq!(stored.head, row.head);
    let events = f.repo.meta().read(|tx| tx.audit_list(Some("from-revision"), 100)).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, "workspace.create");
}
