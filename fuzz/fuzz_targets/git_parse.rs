#![no_main]
//! The Git bridge's importer: the parsers for what `git` prints (`diff-tree --raw -z` records, `log -z`
//! records of NUL-terminated fields) and the functions that turn a commit into a changeset's message and
//! timestamp. The bridge shells out to `git`, so these bytes come from a repository someone else may have
//! crafted (odd modes, NULs, paths and messages that are not UTF-8, absurd dates).
//!
//! Invariants: nothing panics; every path the importer would accept is a clean `RepoPath`; an imported message
//! is bounded and always ends with its `Imported-From` trailer; a timestamp is either exact or refused.

use ag_git::fuzzing;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((which, rest)) = data.split_first() else { return };
    match which % 2 {
        0 => {
            if let Ok(changes) = fuzzing::parse_raw(rest) {
                for c in changes {
                    // Whatever the parser hands over must be accepted or refused by RepoPath, never crash it.
                    let _ = ag_types::RepoPath::new(c.path);
                }
            }
        }
        _ => {
            let Ok(commits) = fuzzing::parse_log(rest) else { return };
            for c in commits {
                let msg = fuzzing::import_message(&c.message, &c.sha);
                assert!(msg.len() <= 60_000 + 200 + c.sha.len(), "an imported message must stay bounded");
                assert!(msg.ends_with(&format!("Imported-From: git {}", c.sha)));
                match fuzzing::import_timestamp(c.time) {
                    Ok(ts) => assert_eq!(i128::from(ts.as_millis()), i128::from(c.time) * 1000),
                    Err(_) => assert!(c.time.checked_mul(1000).is_none()),
                }
            }
        }
    }
    // A message of any shape, cut anywhere, must be handled (multi-byte characters straddling the limit).
    if let Ok(s) = std::str::from_utf8(rest) {
        let long = s.repeat(60_000 / s.len().max(1) + 2);
        let _ = fuzzing::import_message(&long, "0000");
    }
});
