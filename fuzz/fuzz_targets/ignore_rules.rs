#![no_main]
//! `.agignore` files: patterns from the working tree, which anyone with a checkout can edit. Each line is
//! expanded into up to four globs, so compile time and memory must stay bounded for any text.
//!
//! Invariants: parsing never panics or hangs; `.git` and `.ag` are ignored whatever the rules say, in any
//! letter case and at any depth.

use ag_integrate::workdir::IgnoreRules;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    let (rules_text, paths) = text.split_once("\0\0").unwrap_or((text, "a/b.rs\nsrc/lib.rs"));
    if let Ok(rules) = IgnoreRules::parse(rules_text) {
        for p in paths.lines().take(32) {
            let _ = rules.is_ignored(p);
            assert!(rules.is_ignored(&format!("{p}/.git/hooks/pre-commit")));
            assert!(rules.is_ignored(&format!("{p}/.AG/x")));
        }
    }
});
