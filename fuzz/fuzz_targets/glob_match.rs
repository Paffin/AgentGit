#![no_main]
//! Glob matching as used by policies (`protected_paths`, `agent_autonomy`, identity patterns).
//! Input: patterns separated by `\n`, then `\0\0` and paths separated by `\n`.
//!
//! Invariants: compiling a pattern is bounded in time and memory; matching is total and deterministic; `*`
//! never crosses a `/` in a path matcher; a pattern accepted once is accepted again.

use ag_policy::{IdentityMatcher, PathMatcher};
use ag_types::Identity;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    let (pats, paths) = text.split_once("\0\0").unwrap_or((text, "a/b.rs\nsrc/lib.rs\n.git/config"));
    let patterns: Vec<String> = pats.lines().take(16).map(str::to_string).collect();

    if let Ok(m) = PathMatcher::new(&patterns) {
        assert!(PathMatcher::new(&patterns).is_ok());
        for p in paths.lines().take(32) {
            assert_eq!(m.is_match(p), m.is_match(p));
        }
        assert_eq!(m.is_empty(), patterns.is_empty());
    }
    // A single `*` must not match across a separator.
    if let Ok(m) = PathMatcher::new(&["*".to_string()]) {
        assert!(!m.is_match("a/b"));
    }
    if let Ok(m) = IdentityMatcher::new(&patterns) {
        for id in ["user:alice", "agent:bot#run1", "service:ci"] {
            let id = Identity::new(id).unwrap();
            let _ = m.is_match(&id);
        }
    }
});
