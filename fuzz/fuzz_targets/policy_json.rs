#![no_main]
//! Policy documents (JSON, set by administrators over the API) and the glob patterns inside them.
//!
//! Invariants: parsing never panics or hangs, unknown fields are rejected; an accepted document is valid
//! and re-parses to the same value from its canonical form; its globs compile and matching is total.

use ag_policy::{PathMatcher, Policy};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    if let Ok(policy) = Policy::parse(text) {
        let canonical = policy.to_json();
        let again = Policy::parse(&canonical).expect("the canonical form of a valid policy parses");
        assert_eq!(again, policy);
        assert_eq!(again.to_json(), canonical, "to_json must be a fixed point");
        policy.validate().expect("parse validates");
        for p in &policy.protected_paths {
            let m = PathMatcher::new(std::slice::from_ref(&p.glob)).expect("validated glob");
            let _ = m.is_match("a/b/c.rs");
        }
    }
});
