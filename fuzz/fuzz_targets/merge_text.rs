#![no_main]
//! Line-level three-way merge. Invariants (the documented properties of `ag-merge`): deterministic;
//! one-sided changes always win; identical changes merge cleanly; conflict *status* and a clean result are
//! symmetric in `ours`/`theirs`; every conflict region's sides differ; the rendered conflict text contains
//! the marker lines.

use ag_merge::{TextMerge, merge_text};
use libfuzzer_sys::fuzz_target;

fn clean(m: &TextMerge) -> Option<&[u8]> {
    match m {
        TextMerge::Clean(b) => Some(b),
        TextMerge::Conflicted { .. } => None,
    }
}

fuzz_target!(|data: &[u8]| {
    // Three inputs separated by 0xff bytes (or split into thirds).
    let mut parts = data.splitn(3, |b| *b == 0xff);
    let (base, ours, theirs) = match (parts.next(), parts.next(), parts.next()) {
        (Some(b), Some(o), Some(t)) => (b, o, t),
        _ => {
            let n = data.len() / 3;
            (&data[..n], &data[n..2 * n], &data[2 * n..])
        }
    };

    let m = merge_text(base, ours, theirs);
    assert_eq!(m, merge_text(base, ours, theirs), "merge must be deterministic");

    if ours == base {
        assert_eq!(clean(&m), Some(theirs));
    }
    if theirs == base {
        assert_eq!(clean(&m), Some(ours));
    }
    if ours == theirs {
        assert_eq!(clean(&m), Some(ours));
    }

    let swapped = merge_text(base, theirs, ours);
    match (&m, &swapped) {
        (TextMerge::Clean(a), TextMerge::Clean(b)) => assert_eq!(a, b, "a clean merge must not depend on the side"),
        (TextMerge::Conflicted { regions: r1, .. }, TextMerge::Conflicted { regions: r2, .. }) => {
            assert_eq!(r1.len(), r2.len(), "conflict count must be symmetric");
            for (x, y) in r1.iter().zip(r2) {
                assert_eq!((x.base_line, &x.base, &x.ours, &x.theirs), (y.base_line, &y.base, &y.theirs, &y.ours));
            }
        }
        _ => panic!("conflict status depends on which side is ours"),
    }

    if let TextMerge::Conflicted { regions, rendered } = &m {
        assert!(!regions.is_empty());
        for r in regions {
            assert_ne!(r.ours, r.theirs, "a region whose sides agree is not a conflict");
        }
        let text = String::from_utf8_lossy(rendered);
        assert!(text.contains("<<<<<<< ours\n") && text.contains("=======\n") && text.contains(">>>>>>> theirs\n"));
    }
});
