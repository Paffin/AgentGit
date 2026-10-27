//! `ConflictKind::CODES` lists exactly the codes the variants produce, so explanations can be tested against it.
#![allow(missing_docs)]

use ag_merge::ConflictKind;
use ag_types::RepoPath;

fn p(s: &str) -> RepoPath {
    RepoPath::new(s).unwrap()
}

#[test]
fn the_list_of_codes_is_the_list_of_variants() {
    let all = vec![
        ConflictKind::Content { regions: 1 },
        ConflictKind::BothAdded,
        ConflictKind::DeleteModify { ours_deleted: true },
        ConflictKind::Binary,
        ConflictKind::TypeChange,
        ConflictKind::TooLarge,
        ConflictKind::FileDirectory { inside: p("a/b") },
        ConflictKind::RenameRename { ours: p("x"), theirs: p("y") },
        ConflictKind::RenameDelete { ours_renamed: true },
        ConflictKind::DirectoryRenameAmbiguous { ours_renamed: true, directory: p("d"), candidates: vec![p("e")] },
        ConflictKind::DirectoryRenameCollision { ours_renamed: true, directory: p("d"), target: p("e") },
    ];
    // Adding a variant stops this match from compiling, which is the reminder to extend the list above and CODES.
    for k in &all {
        match k {
            ConflictKind::Content { .. }
            | ConflictKind::BothAdded
            | ConflictKind::DeleteModify { .. }
            | ConflictKind::Binary
            | ConflictKind::TypeChange
            | ConflictKind::TooLarge
            | ConflictKind::FileDirectory { .. }
            | ConflictKind::RenameRename { .. }
            | ConflictKind::RenameDelete { .. }
            | ConflictKind::DirectoryRenameAmbiguous { .. }
            | ConflictKind::DirectoryRenameCollision { .. } => {}
        }
    }
    let produced: Vec<&str> = all.iter().map(ConflictKind::code).collect();
    assert_eq!(produced, ConflictKind::CODES);
}
