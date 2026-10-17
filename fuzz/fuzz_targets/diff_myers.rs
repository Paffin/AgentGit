#![no_main]
//! Myers diff and unified-diff rendering on arbitrary byte inputs.
//!
//! Invariants: deterministic; hunks are ordered, in range, disjoint and separated by an unchanged line;
//! applying them to `a` gives exactly `b` (also when the edit-distance cap kicks in); a text diff of equal
//! inputs is empty; the unified diff never panics for any context, including absurd ones.

use ag_diff::{Hunk, diff_hunks, is_binary, split_lines, unified_diff};
use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;

fn apply<'a>(a: &[&'a [u8]], b: &[&'a [u8]], hunks: &[Hunk]) -> Vec<&'a [u8]> {
    let mut out = Vec::new();
    let mut pos = 0;
    for h in hunks {
        out.extend_from_slice(&a[pos..h.a_start]);
        out.extend_from_slice(&b[h.b_start..h.b_end()]);
        pos = h.a_end();
    }
    out.extend_from_slice(&a[pos..]);
    out
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let context: usize = match u.int_in_range(0..=5u8).unwrap_or(0) {
        0 => usize::MAX,
        1 => usize::MAX / 2 + 1,
        2 => u.arbitrary().unwrap_or(3),
        _ => u.int_in_range(0..=6).unwrap_or(3),
    };
    let rest = u.take_rest();
    // Split into two inputs at a delimiter so the fuzzer can shape both sides.
    let (old, new) = match rest.iter().position(|b| *b == 0xff) {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (&rest[..rest.len() / 2], &rest[rest.len() / 2..]),
    };

    let a = split_lines(old);
    let b = split_lines(new);
    assert_eq!(a.concat(), old);
    let hunks = diff_hunks(&a, &b);
    assert_eq!(hunks, diff_hunks(&a, &b), "diff must be deterministic");
    let mut prev_end: Option<(usize, usize)> = None;
    for h in &hunks {
        assert!(h.a_end() <= a.len() && h.b_end() <= b.len());
        assert!(h.a_len > 0 || h.b_len > 0, "empty hunk");
        if let Some((pa, pb)) = prev_end {
            assert!(h.a_start > pa && h.b_start > pb, "hunks must be separated by an unchanged element");
        }
        prev_end = Some((h.a_end(), h.b_end()));
    }
    assert_eq!(apply(&a, &b, &hunks), b, "applying the hunks must give the target");
    assert_eq!(hunks.is_empty(), a == b);

    let patch = unified_diff("a/f", "b/f", old, new, context);
    assert_eq!(patch.is_empty(), old == new);
    if old != new && !is_binary(old) && !is_binary(new) {
        assert!(patch.starts_with("--- a/f\n+++ b/f\n"));
    }
});
