use ag_diff::{Hunk, diff_hunks, split_lines};

/// A region where both sides changed the same part of the file differently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictRegion {
    /// Zero-based line in the base where the conflicting cluster starts.
    pub base_line: usize,
    /// Base text of the cluster.
    pub base: Vec<u8>,
    /// Our version of the cluster.
    pub ours: Vec<u8>,
    /// Their version of the cluster.
    pub theirs: Vec<u8>,
}

/// Result of a three-way text merge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextMerge {
    /// The merge is unambiguous; contains the merged bytes.
    Clean(Vec<u8>),
    /// At least one cluster conflicts. `rendered` holds the file with
    /// conflict markers for humans; `regions` is the structured form.
    Conflicted {
        /// Structured conflict regions, in file order.
        regions: Vec<ConflictRegion>,
        /// The file with `<<<<<<<` / `=======` / `>>>>>>>` markers.
        rendered: Vec<u8>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Ours,
    Theirs,
}

/// Merges `ours` and `theirs`, both derived from `base`.
pub fn merge_text(base: &[u8], ours: &[u8], theirs: &[u8]) -> TextMerge {
    if ours == theirs {
        return TextMerge::Clean(ours.to_vec());
    }
    if ours == base {
        return TextMerge::Clean(theirs.to_vec());
    }
    if theirs == base {
        return TextMerge::Clean(ours.to_vec());
    }
    let b = split_lines(base);
    let o = split_lines(ours);
    let t = split_lines(theirs);
    let ho = diff_hunks(&b, &o);
    let ht = diff_hunks(&b, &t);

    // All hunks tagged with their side, ordered by base position. The sort is stable and ours come first, so hunks
    // of both sides at the same place stay in the order ours, theirs.
    let mut all: Vec<(Side, Hunk)> = ho.iter().map(|h| (Side::Ours, *h)).collect();
    all.extend(ht.iter().map(|h| (Side::Theirs, *h)));
    all.sort_by_key(|(_, h)| (h.a_start, h.a_end()));

    let mut clean: Vec<u8> = Vec::new();
    let mut rendered: Vec<u8> = Vec::new();
    let mut regions: Vec<ConflictRegion> = Vec::new();
    let mut base_pos = 0usize;
    let mut i = 0;
    while i < all.len() {
        // Build a cluster of hunks that overlap or touch.
        let lo = all[i].1.a_start;
        let mut hi = all[i].1.a_end();
        let mut j = i + 1;
        while j < all.len() && all[j].1.a_start <= hi {
            hi = hi.max(all[j].1.a_end());
            j += 1;
        }
        let cluster = &all[i..j];

        // Unchanged base lines before the cluster.
        let gap = join(&b[base_pos..lo]);
        clean.extend_from_slice(&gap);
        rendered.extend_from_slice(&gap);

        let has_ours = cluster.iter().any(|(s, _)| *s == Side::Ours);
        let has_theirs = cluster.iter().any(|(s, _)| *s == Side::Theirs);
        let ours_text = side_text(&b, &o, cluster, Side::Ours, lo, hi);
        let theirs_text = side_text(&b, &t, cluster, Side::Theirs, lo, hi);
        if !(has_ours && has_theirs) {
            let text = if has_ours { ours_text } else { theirs_text };
            clean.extend_from_slice(&text);
            rendered.extend_from_slice(&text);
        } else if ours_text == theirs_text {
            clean.extend_from_slice(&ours_text);
            rendered.extend_from_slice(&ours_text);
        } else {
            let base_text = join(&b[lo..hi]);
            rendered.extend_from_slice(&render_conflict_markers(&ours_text, &theirs_text));
            regions.push(ConflictRegion { base_line: lo, base: base_text, ours: ours_text, theirs: theirs_text });
        }
        base_pos = hi;
        i = j;
    }
    let tail = join(&b[base_pos..]);
    clean.extend_from_slice(&tail);
    rendered.extend_from_slice(&tail);

    if regions.is_empty() { TextMerge::Clean(clean) } else { TextMerge::Conflicted { regions, rendered } }
}

/// Renders one conflict block with git-style markers. Guarantees each marker
/// starts on its own line even if a side lacks a trailing newline.
pub fn render_conflict_markers(ours: &[u8], theirs: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"<<<<<<< ours\n");
    out.extend_from_slice(ours);
    if !ours.is_empty() && !ours.ends_with(b"\n") {
        out.push(b'\n');
    }
    out.extend_from_slice(b"=======\n");
    out.extend_from_slice(theirs);
    if !theirs.is_empty() && !theirs.ends_with(b"\n") {
        out.push(b'\n');
    }
    out.extend_from_slice(b">>>>>>> theirs\n");
    out
}

fn join(lines: &[&[u8]]) -> Vec<u8> {
    lines.iter().flat_map(|l| l.iter().copied()).collect()
}

/// The text one side has for base range `[lo, hi)`: base lines with that
/// side's hunks (inside the cluster) applied.
fn side_text(
    base: &[&[u8]],
    side_lines: &[&[u8]],
    cluster: &[(Side, Hunk)],
    side: Side,
    lo: usize,
    hi: usize,
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pos = lo;
    for (s, h) in cluster {
        if *s != side {
            continue;
        }
        out.extend_from_slice(&join(&base[pos..h.a_start]));
        out.extend_from_slice(&join(&side_lines[h.b_start..h.b_end()]));
        pos = h.a_end();
    }
    out.extend_from_slice(&join(&base[pos..hi]));
    out
}
