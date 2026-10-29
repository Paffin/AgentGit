//! Rename-aware conflicts retain source locations when converted or persisted.
use ag_integrate::ConflictInfo;
use ag_merge::{Conflict, ConflictKind, ConflictOrigin};
use ag_types::RepoPath;
use serde_json::json;

#[test]
fn conflict_conversion_preserves_all_three_source_paths_and_reads_legacy_outcomes() {
    let path = |value| RepoPath::new(value).unwrap();
    let conflict = Conflict {
        path: path("new.txt"),
        kind: ConflictKind::Content { regions: 1 },
        origin: Some(ConflictOrigin { base: path("old.txt"), ours: path("new.txt"), theirs: path("old.txt") }),
    };
    let converted = ConflictInfo::from_conflict(&conflict);
    let encoded = serde_json::to_value(&converted).unwrap();
    assert_eq!(encoded["origin"], json!({"base":"old.txt","ours":"new.txt","theirs":"old.txt"}));
    let decoded: ConflictInfo = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, converted);
    let legacy: ConflictInfo = serde_json::from_value(json!({"path":"file","kind":"content"})).unwrap();
    assert_eq!(legacy.describe(), "content");
    assert!(serde_json::to_value(legacy).unwrap().get("origin").is_none_or(|v| v.is_null()));
}

#[test]
fn ordinary_and_legacy_conflicts_keep_their_existing_serialized_shape() {
    let old = json!({"path":"file","kind":"content","message":"overlap"});
    let decoded: ConflictInfo = serde_json::from_value(old.clone()).unwrap();
    assert!(decoded.resolution_scope.is_none());
    assert_eq!(serde_json::to_value(&decoded).unwrap(), old);
    let fresh = ConflictInfo::from_conflict(&Conflict {
        path: RepoPath::new("file").unwrap(),
        kind: ConflictKind::Content { regions: 1 },
        origin: None,
    });
    assert_eq!(
        serde_json::to_value(fresh).unwrap(),
        json!({"path":"file","kind":"content","message":"content conflict (1 region(s))"})
    );
}
