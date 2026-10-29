//! Deterministic line diff (Myers' O(ND) algorithm) and unified diff output.
//!
//! The algorithm is part of the specification of AgentGit merges: merges are
//! reproducible only if the same inputs always give the same hunks. This
//! crate therefore implements the diff itself rather than depending on a
//! library whose output may change between releases. Behavioural changes
//! bump [`ALGORITHM_VERSION`].
//!
//! Limits: the common prefix and suffix are stripped first. If the remaining
//! edit distance exceeds [`MAX_EDIT_DISTANCE`] the middle is reported as one
//! replaced region. That is always *correct* (applying the hunks reproduces
//! the target) but not minimal.

use std::fmt::Write as _;

/// Identifier of the diff algorithm, recorded alongside merge results.
pub const ALGORITHM_VERSION: &str = "ag-diff/1";

/// Maximum edit distance explored before falling back to one big hunk.
pub const MAX_EDIT_DISTANCE: usize = 2048;

/// A maximal run of changes: `a[a_start..a_start+a_len]` is replaced by
/// `b[b_start..b_start+b_len]`. Either side may be empty (pure insert/delete).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// Start index in the old sequence.
    pub a_start: usize,
    /// Number of old elements replaced.
    pub a_len: usize,
    /// Start index in the new sequence.
    pub b_start: usize,
    /// Number of new elements.
    pub b_len: usize,
}

impl Hunk {
    /// End index (exclusive) in the old sequence.
    pub fn a_end(&self) -> usize {
        self.a_start + self.a_len
    }

    /// End index (exclusive) in the new sequence.
    pub fn b_end(&self) -> usize {
        self.b_start + self.b_len
    }
}

/// Splits bytes into lines, keeping each line's `\n` terminator. The last
/// line has no terminator if the data does not end with a newline.
pub fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, b) in data.iter().enumerate() {
        if *b == b'\n' {
            lines.push(&data[start..=i]);
            start = i + 1;
        }
    }
    if start < data.len() {
        lines.push(&data[start..]);
    }
    lines
}

/// How much of a file [`is_binary`] looks at.
pub const BINARY_SNIFF_LEN: usize = 8192;

/// Heuristic binary detection: a NUL byte in the first [`BINARY_SNIFF_LEN`] bytes (8 KiB).
pub fn is_binary(data: &[u8]) -> bool {
    data[..data.len().min(BINARY_SNIFF_LEN)].contains(&0)
}

/// Computes the hunks that turn `a` into `b`. Hunks are ordered, disjoint and
/// separated by at least one unchanged element.
pub fn diff_hunks<T: Eq>(a: &[T], b: &[T]) -> Vec<Hunk> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let max_suffix = a.len().min(b.len()) - prefix;
    let suffix = a.iter().rev().zip(b.iter().rev()).take(max_suffix).take_while(|(x, y)| x == y).count();
    let am = &a[prefix..a.len() - suffix];
    let bm = &b[prefix..b.len() - suffix];
    if am.is_empty() && bm.is_empty() {
        return Vec::new();
    }
    let mut hunks = match myers(am, bm) {
        Some(h) => h,
        None => vec![Hunk { a_start: 0, a_len: am.len(), b_start: 0, b_len: bm.len() }],
    };
    for h in &mut hunks {
        h.a_start += prefix;
        h.b_start += prefix;
    }
    hunks
}

fn myers<T: Eq>(a: &[T], b: &[T]) -> Option<Vec<Hunk>> {
    let n = a.len() as isize;
    let m = b.len() as isize;
    if n == 0 || m == 0 {
        return Some(vec![Hunk { a_start: 0, a_len: n as usize, b_start: 0, b_len: m as usize }]);
    }
    let max = ((n + m) as usize).min(MAX_EDIT_DISTANCE) as isize;
    let off = max + 1;
    let mut v = vec![0i32; (2 * max + 3) as usize];
    let mut trace: Vec<Vec<i32>> = Vec::new();
    let mut found: Option<isize> = None;
    'outer: for d in 0..=max {
        let lo = (off - d).max(0) as usize;
        let hi = ((off + d) as usize).min(v.len() - 1);
        trace.push(v[lo..=hi].to_vec());
        let mut k = -d;
        while k <= d {
            let idx = (off + k) as usize;
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
                v[idx + 1] as isize
            } else {
                v[idx - 1] as isize + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x as i32;
            if x >= n && y >= m {
                found = Some(d);
                break 'outer;
            }
            k += 2;
        }
    }
    let d_final = found?;

    // Backtrack to a reversed edit script.
    #[derive(Clone, Copy, PartialEq)]
    enum E {
        Eq,
        Del,
        Ins,
    }
    let mut ops: Vec<E> = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..=d_final).rev() {
        let snap = &trace[d as usize];
        let base = (off - d).max(0);
        let get = |k: isize| snap[((off + k) - base) as usize] as isize;
        let k = x - y;
        let prev_k = if k == -d || (k != d && get(k - 1) < get(k + 1)) { k + 1 } else { k - 1 };
        let prev_x = get(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            ops.push(E::Eq);
        }
        if x == prev_x {
            y -= 1;
            ops.push(E::Ins);
        } else {
            x -= 1;
            ops.push(E::Del);
        }
    }
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        ops.push(E::Eq);
    }
    ops.reverse();

    // Group non-equal runs into hunks.
    let mut hunks = Vec::new();
    let (mut ai, mut bi) = (0usize, 0usize);
    let mut cur: Option<Hunk> = None;
    for op in ops {
        match op {
            E::Eq => {
                if let Some(h) = cur.take() {
                    hunks.push(h);
                }
                ai += 1;
                bi += 1;
            }
            E::Del | E::Ins => {
                let h = cur.get_or_insert(Hunk { a_start: ai, a_len: 0, b_start: bi, b_len: 0 });
                if op == E::Del {
                    h.a_len += 1;
                    ai += 1;
                } else {
                    h.b_len += 1;
                    bi += 1;
                }
            }
        }
    }
    if let Some(h) = cur {
        hunks.push(h);
    }
    Some(hunks)
}

/// Renders a unified diff of two byte strings, line based. Returns an empty
/// string if they are equal. Binary data is reported in one line.
pub fn unified_diff(old_name: &str, new_name: &str, old: &[u8], new: &[u8], context: usize) -> String {
    unified_diff_limited(old_name, new_name, old, new, context, usize::MAX).expect("unlimited output")
}

/// Renders a complete patch within `max_bytes`, or returns `None` without retaining oversized output.
/// Callers handling untrusted files should also bound the input before splitting it into lines.
pub fn unified_diff_limited(
    old_name: &str,
    new_name: &str,
    old: &[u8],
    new: &[u8],
    context: usize,
    max_bytes: usize,
) -> Option<String> {
    if old == new {
        return Some(String::new());
    }
    let mut out = LimitedOutput { text: String::new(), limit: max_bytes };
    writeln!(out, "--- {old_name}").ok()?;
    writeln!(out, "+++ {new_name}").ok()?;
    if is_binary(old) || is_binary(new) {
        writeln!(out, "Binary files differ").ok()?;
        return Some(out.text);
    }
    let a = split_lines(old);
    let b = split_lines(new);
    let hunks = diff_hunks(&a, &b);

    // Group hunks whose context regions touch or overlap.
    let mut i = 0;
    while i < hunks.len() {
        let mut j = i;
        while j + 1 < hunks.len() && hunks[j + 1].a_start - hunks[j].a_end() <= context.saturating_mul(2) {
            j += 1;
        }
        let first = hunks[i];
        let last = hunks[j];
        let a_from = first.a_start.saturating_sub(context);
        let b_from = first.b_start.saturating_sub(context);
        let a_to = last.a_end().saturating_add(context).min(a.len());
        let b_to = last.b_end().saturating_add(context).min(b.len());
        writeln!(out, "@@ -{} +{} @@", range(a_from, a_to - a_from), range(b_from, b_to - b_from)).ok()?;
        let mut ai = a_from;
        for h in &hunks[i..=j] {
            while ai < h.a_start {
                push_line(&mut out, ' ', a[ai], ai + 1 == a.len()).ok()?;
                ai += 1;
            }
            for idx in h.a_start..h.a_end() {
                push_line(&mut out, '-', a[idx], idx + 1 == a.len()).ok()?;
            }
            for idx in h.b_start..h.b_end() {
                push_line(&mut out, '+', b[idx], idx + 1 == b.len()).ok()?;
            }
            ai = h.a_end();
        }
        while ai < a_to {
            push_line(&mut out, ' ', a[ai], ai + 1 == a.len()).ok()?;
            ai += 1;
        }
        i = j + 1;
    }
    Some(out.text)
}

fn range(start: usize, len: usize) -> String {
    match len {
        0 => format!("{start},0"),
        1 => format!("{}", start + 1),
        _ => format!("{},{}", start + 1, len),
    }
}

struct LimitedOutput {
    text: String,
    limit: usize,
}
impl std::fmt::Write for LimitedOutput {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        if text.len() > self.limit.saturating_sub(self.text.len()) {
            return Err(std::fmt::Error);
        }
        self.text.push_str(text);
        Ok(())
    }
}

fn push_line(out: &mut impl std::fmt::Write, tag: char, line: &[u8], is_last: bool) -> std::fmt::Result {
    let text = String::from_utf8_lossy(line);
    writeln!(out, "{tag}{}", text.trim_end_matches('\n'))?;
    if is_last && !line.ends_with(b"\n") {
        writeln!(out, "\\ No newline at end of file")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
