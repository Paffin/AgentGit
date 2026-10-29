//! Render the source locations of a renamed conflict without changing its result path.
use ag_integrate::ConflictInfo;
use ag_protocol::v1;

fn sources(path: &str, origin: Option<(&str, &str, &str)>) -> String {
    match origin {
        Some((base, ours, theirs)) if [base, ours, theirs].iter().any(|source| *source != path) => {
            format!("\n    source paths: base={base}, ours={ours}, theirs={theirs}")
        }
        _ => String::new(),
    }
}

/// Source paths for an embedded repository conflict, absent for legacy outcomes.
pub fn local_sources(conflict: &ConflictInfo) -> String {
    sources(&conflict.path, conflict.origin.as_ref().map(|o| (o.base.as_str(), o.ours.as_str(), o.theirs.as_str())))
}

/// Source paths returned by a server, absent when talking to an older server.
pub fn wire_sources(conflict: &v1::Conflict) -> String {
    sources(&conflict.path, conflict.origin.as_ref().map(|o| (o.base.as_str(), o.ours.as_str(), o.theirs.as_str())))
}

/// Human-readable embedded conflict, preserving the engine's kind description.
pub fn local(conflict: &ConflictInfo) -> String {
    format!("{} ({}){}", conflict.path, conflict.describe(), local_sources(conflict))
}

/// Human-readable server conflict, including the legacy kind-only fallback.
pub fn wire(conflict: &v1::Conflict) -> String {
    let message = if conflict.message.is_empty() { &conflict.kind } else { &conflict.message };
    format!("{} ({}){}", conflict.path, message, wire_sources(conflict))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renamed_sources_keep_each_side_and_the_result_path() {
        let conflict = v1::Conflict {
            path: "result.txt".into(),
            kind: "content".into(),
            message: "conflicting lines".into(),
            origin: Some(v1::ConflictOrigin {
                base: "old.txt".into(),
                ours: "result.txt".into(),
                theirs: "other.txt".into(),
            }),
        };
        assert_eq!(
            wire(&conflict),
            "result.txt (conflicting lines)\n    source paths: base=old.txt, ours=result.txt, theirs=other.txt"
        );
    }

    #[test]
    fn legacy_and_same_path_conflicts_do_not_gain_noise() {
        let mut conflict = v1::Conflict { path: "a.txt".into(), kind: "content".into(), ..Default::default() };
        assert_eq!(wire(&conflict), "a.txt (content)");
        conflict.origin =
            Some(v1::ConflictOrigin { base: "a.txt".into(), ours: "a.txt".into(), theirs: "a.txt".into() });
        assert_eq!(wire(&conflict), "a.txt (content)");
    }
}
