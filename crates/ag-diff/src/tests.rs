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
    assert_eq!(
        diff_hunks(&[1, 2, 3], &[1, 9, 3]),
        vec![Hunk { a_start: 1, a_len: 1, b_start: 1, b_len: 1 }]
    );
    assert_eq!(
        diff_hunks(&[1, 2, 3], &[1, 2, 3, 4]),
        vec![Hunk { a_start: 3, a_len: 0, b_start: 3, b_len: 1 }]
    );
    assert_eq!(
        diff_hunks(&[], &[1, 2]),
        vec![Hunk { a_start: 0, a_len: 0, b_start: 0, b_len: 2 }]
    );
    assert_eq!(
        diff_hunks(&[1, 2], &[]),
        vec![Hunk { a_start: 0, a_len: 2, b_start: 0, b_len: 0 }]
    );
}

#[test]
fn split_lines_keeps_terminators() {
    assert_eq!(split_lines(b"a\nb\n"), vec![&b"a\n"[..], &b"b\n"[..]]);
    assert_eq!(split_lines(b"a\nb"), vec![&b"a\n"[..], &b"b"[..]]);
    assert!(split_lines(b"").is_empty());
    assert_eq!(split_lines(b"\n\n").len(), 2);
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
