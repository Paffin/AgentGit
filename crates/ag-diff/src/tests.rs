use super::*;
use proptest::prelude::*;

fn apply<T: Clone>(a: &[T], b: &[T], hunks: &[Hunk]) -> Vec<T> {
    let mut out = Vec::new();
    let mut ai = 0;
    for h in hunks {
        out.extend_from_slice(&a[ai..h.a_start]);
        out.extend_from_slice(&b[h.b_start..h.b_end()]);
        ai = h.a_end();
    }
    out.extend_from_slice(&a[ai..]);
    out
}

fn lcs_len<T: Eq>(a: &[T], b: &[T]) -> usize {
    let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            dp[i][j] = if a[i - 1] == b[j - 1] { dp[i - 1][j - 1] + 1 } else { dp[i - 1][j].max(dp[i][j - 1]) };
        }
    }
    dp[a.len()][b.len()]
}

#[test]
fn equal_inputs_have_no_hunks() {
    assert!(diff_hunks(&[1, 2, 3], &[1, 2, 3]).is_empty());
    assert!(diff_hunks::<u8>(&[], &[]).is_empty());
}

#[test]
fn simple_cases() {
    assert_eq!(diff_hunks(&[1, 2, 3], &[1, 9, 3]), vec![Hunk { a_start: 1, a_len: 1, b_start: 1, b_len: 1 }]);
    assert_eq!(diff_hunks(&[1, 2, 3], &[1, 2, 3, 4]), vec![Hunk { a_start: 3, a_len: 0, b_start: 3, b_len: 1 }]);
    assert_eq!(diff_hunks(&[], &[1, 2]), vec![Hunk { a_start: 0, a_len: 0, b_start: 0, b_len: 2 }]);
    assert_eq!(diff_hunks(&[1, 2], &[]), vec![Hunk { a_start: 0, a_len: 2, b_start: 0, b_len: 0 }]);
}

#[test]
fn split_lines_keeps_terminators() {
    assert_eq!(split_lines(b"a\nb\n"), vec![&b"a\n"[..], &b"b\n"[..]]);
    assert_eq!(split_lines(b"a\nb"), vec![&b"a\n"[..], &b"b"[..]]);
    assert!(split_lines(b"").is_empty());
    assert_eq!(split_lines(b"\n\n").len(), 2);
}

/// Every sequence over `alphabet` letters up to `max_len` long.
fn all_sequences(max_len: usize, alphabet: u8) -> Vec<Vec<u8>> {
    let mut out = vec![vec![]];
    let mut frontier: Vec<Vec<u8>> = vec![vec![]];
    for _ in 0..max_len {
        let mut next = Vec::new();
        for s in &frontier {
            for c in 0..alphabet {
                let mut t = s.clone();
                t.push(c);
                next.push(t);
            }
        }
        out.extend(next.iter().cloned());
        frontier = next;
    }
    out
}

/// Not a sample (the property tests below are): every pair of sequences of up to five elements over three letters,
/// which is where the ties and the off-by-one cases of the algorithm are. Each result must reproduce the target, be
/// minimal, and be made of ordered hunks separated by elements that really are equal.
#[test]
fn every_short_pair_is_diffed_correctly_and_minimally() {
    let seqs = all_sequences(5, 3);
    assert_eq!(seqs.len(), 364);
    for a in &seqs {
        for b in &seqs {
            let hunks = diff_hunks(a, b);
            assert_eq!(apply(a, b, &hunks), *b, "{a:?} -> {b:?}: {hunks:?}");
            let cost: usize = hunks.iter().map(|h| h.a_len + h.b_len).sum();
            assert_eq!(cost, a.len() + b.len() - 2 * lcs_len(a, b), "{a:?} -> {b:?}: {hunks:?}");
            let (mut ai, mut bi) = (0, 0);
            for h in &hunks {
                assert!(h.a_len + h.b_len > 0, "an empty hunk: {a:?} -> {b:?}");
                assert_eq!(a[ai..h.a_start], b[bi..h.b_start], "{a:?} -> {b:?}: the text between hunks is shared");
                assert!(h.a_end() <= a.len() && h.b_end() <= b.len());
                (ai, bi) = (h.a_end(), h.b_end());
            }
            assert_eq!(a[ai..], b[bi..], "{a:?} -> {b:?}: so is the tail");
        }
    }
}

#[test]
fn big_edit_distance_falls_back_to_one_correct_hunk() {
    let a: Vec<u32> = (0..3000).collect();
    let b: Vec<u32> = (10_000..13_000).collect();
    let hunks = diff_hunks(&a, &b);
    assert_eq!(hunks.len(), 1);
    assert_eq!(apply(&a, &b, &hunks), b);
}

#[test]
fn unified_output_shape() {
    let old = b"one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
    let new = b"one\ntwo\nTHREE\nfour\nfive\nsix\nseven\neight\n";
    let d = unified_diff("a/f", "b/f", old, new, 1);
    let expected = "--- a/f\n+++ b/f\n@@ -2,3 +2,3 @@\n two\n-three\n+THREE\n four\n@@ -7 +7,2 @@\n seven\n+eight\n";
    assert_eq!(d, expected);
    assert_eq!(unified_diff("a", "b", old, old, 3), "");
}

#[test]
fn unified_marks_missing_newline() {
    let d = unified_diff("a", "b", b"x\ny", b"x\nz", 3);
    assert!(d.contains("-y\n\\ No newline at end of file\n"), "{d}");
    assert!(d.contains("+z\n\\ No newline at end of file\n"), "{d}");
}

/// Found by the `diff_myers` fuzz target: `2 * context` and `end + context` overflowed (a panic in debug builds,
/// a wrong hunk header in release) for a huge context such as `ag diff -U 18446744073709551615`.
#[test]
fn unified_diff_survives_an_absurd_context() {
    let old = b"a\nb\nc\nd\ne\nf\ng\nh\n";
    let new = b"a\nB\nc\nd\ne\nf\nG\nh\n";
    for context in [usize::MAX, usize::MAX / 2, usize::MAX / 2 + 1, usize::MAX - 1] {
        let huge = unified_diff("a", "b", old, new, context);
        // Context larger than the file means: the whole file, as one hunk.
        assert_eq!(huge, unified_diff("a", "b", old, new, 1000), "context {context}");
        assert!(huge.contains("@@ -1,8 +1,8 @@"), "{huge}");
    }
}

fn lines_of(lines: &[&str]) -> Vec<u8> {
    lines.iter().flat_map(|l| format!("{l}\n").into_bytes()).collect()
}

#[test]
fn changes_are_one_hunk_exactly_when_their_context_touches() {
    // Changes to line 2 and to a later line; with one line of context two unchanged lines between them are shared
    // context (one hunk), three are not (two hunks).
    let old = lines_of(&["1", "2", "3", "4", "5", "6", "7", "8"]);
    let hunks = |new: &[u8], context: usize| unified_diff("a", "b", &old, new, context).matches("@@ -").count();
    assert_eq!(hunks(&lines_of(&["1", "X", "3", "4", "Y", "6", "7", "8"]), 1), 1, "lines 3 and 4 between the changes");
    assert_eq!(hunks(&lines_of(&["1", "X", "3", "4", "5", "Y", "7", "8"]), 1), 2, "lines 3, 4 and 5 between them");
    // with no context at all, changes that are not adjacent are separate hunks
    assert_eq!(hunks(&lines_of(&["1", "X", "3", "4", "5", "Y", "7", "8"]), 0), 2);
    assert_eq!(hunks(&lines_of(&["1", "X", "3", "Y", "5", "6", "7", "8"]), 0), 2);
}

#[test]
fn one_binary_side_is_enough_to_call_it_binary() {
    for (old, new) in [(&b"text\n"[..], &b"\0bin"[..]), (&b"\0bin"[..], &b"text\n"[..])] {
        let d = unified_diff("a", "b", old, new, 3);
        assert_eq!(d, "--- a\n+++ b\nBinary files differ\n");
    }
}

#[test]
fn a_missing_final_newline_is_marked_on_the_side_it_is_missing_from_only() {
    // only the old side lacks it
    assert_eq!(
        unified_diff("a", "b", b"x\ny", b"x\ny\n", 0),
        "--- a\n+++ b\n@@ -2 +2 @@\n-y\n\\ No newline at end of file\n+y\n"
    );
    // only the new side lacks it
    assert_eq!(
        unified_diff("a", "b", b"x\ny\n", b"x\ny", 0),
        "--- a\n+++ b\n@@ -2 +2 @@\n-y\n+y\n\\ No newline at end of file\n"
    );
    // a context line at the end of a file without the newline carries the marker too, and only the last line does
    assert_eq!(
        unified_diff("a", "b", b"a\nb\nc", b"A\nb\nc", 3),
        "--- a\n+++ b\n@@ -1,3 +1,3 @@\n-a\n+A\n b\n c\n\\ No newline at end of file\n"
    );
}

#[test]
fn hunk_headers_name_empty_ranges_the_way_diff_does() {
    // a pure insertion has an empty old range that starts BEFORE the inserted text (line 0 at the top) ...
    assert_eq!(unified_diff("a", "b", b"b\n", b"a\nb\n", 0), "--- a\n+++ b\n@@ -0,0 +1 @@\n+a\n");
    assert_eq!(unified_diff("a", "b", b"a\n", b"a\nb\nc\n", 0), "--- a\n+++ b\n@@ -1,0 +2,2 @@\n+b\n+c\n");
    // ... a pure deletion has an empty new range
    assert_eq!(unified_diff("a", "b", b"a\nb\n", b"b\n", 0), "--- a\n+++ b\n@@ -1 +0,0 @@\n-a\n");
    // one line is "N", several are "N,len"
    assert_eq!(
        unified_diff("a", "b", b"1\n2\n3\n", b"1\nX\nY\n", 0),
        "--- a\n+++ b\n@@ -2,2 +2,2 @@\n-2\n-3\n+X\n+Y\n"
    );
}

#[test]
fn unified_binary() {
    let d = unified_diff("a", "b", b"\0\x01", b"\0\x02", 3);
    assert!(d.contains("Binary files differ"));
}

proptest! {
    #[test]
    fn hunks_reconstruct_target(a in proptest::collection::vec(0u8..4, 0..30), b in proptest::collection::vec(0u8..4, 0..30)) {
        let hunks = diff_hunks(&a, &b);
        prop_assert_eq!(apply(&a, &b, &hunks), b.clone());
        // ordered, disjoint, separated
        for w in hunks.windows(2) {
            prop_assert!(w[0].a_end() < w[1].a_start);
            prop_assert!(w[0].b_end() < w[1].b_start);
        }
    }

    #[test]
    fn edit_script_is_minimal(a in proptest::collection::vec(0u8..4, 0..30), b in proptest::collection::vec(0u8..4, 0..30)) {
        let hunks = diff_hunks(&a, &b);
        let cost: usize = hunks.iter().map(|h| h.a_len + h.b_len).sum();
        prop_assert_eq!(cost, a.len() + b.len() - 2 * lcs_len(&a, &b));
    }

    #[test]
    fn diff_is_deterministic(a in proptest::collection::vec(0u8..3, 0..40), b in proptest::collection::vec(0u8..3, 0..40)) {
        prop_assert_eq!(diff_hunks(&a, &b), diff_hunks(&a, &b));
    }
}
