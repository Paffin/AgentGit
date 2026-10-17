// Package textdiff provides the small amount of line-based diffing the
// advisory components need: a deterministic Myers diff and a three-way
// clustering that follows the same rule as the server's merge (hunks of the
// two sides that overlap or merely touch form one cluster).
//
// Nothing in this package is authoritative. The Rust core decides what merges;
// this code only helps explain and suggest.
package textdiff

import "strings"

// Limits keep the cost of a diff bounded regardless of input.
const (
	// MaxLines is the largest number of lines per side the diff will look at.
	MaxLines = 40000
	// MaxEdits bounds the Myers search; beyond it the differing middle of the
	// two inputs is reported as one replaced block.
	MaxEdits = 1500
)

// SplitLines splits s into lines, keeping each line's terminating "\n" so that
// joining the lines reproduces s exactly.
func SplitLines(s string) []string {
	if s == "" {
		return nil
	}
	var out []string
	for len(s) > 0 {
		i := strings.IndexByte(s, '\n')
		if i < 0 {
			out = append(out, s)
			break
		}
		out = append(out, s[:i+1])
		s = s[i+1:]
	}
	return out
}

// Join concatenates lines.
func Join(lines []string) string { return strings.Join(lines, "") }

// Hunk says that a[AStart:AEnd] was replaced by b[BStart:BEnd]. An insertion
// has AStart == AEnd, a deletion has BStart == BEnd.
type Hunk struct {
	AStart, AEnd int
	BStart, BEnd int
}

// Diff returns the minimal-ish set of hunks that turn a into b, in order.
func Diff(a, b []string) []Hunk {
	// Trim the common prefix and suffix; most real diffs are small.
	pre := 0
	for pre < len(a) && pre < len(b) && a[pre] == b[pre] {
		pre++
	}
	sa, sb := len(a), len(b)
	for sa > pre && sb > pre && a[sa-1] == b[sb-1] {
		sa--
		sb--
	}
	ma, mb := a[pre:sa], b[pre:sb]
	if len(ma) == 0 && len(mb) == 0 {
		return nil
	}
	if len(ma) == 0 || len(mb) == 0 || len(ma) > MaxLines || len(mb) > MaxLines {
		return []Hunk{{pre, sa, pre, sb}}
	}
	ops, ok := myers(intern(ma, mb))
	if !ok {
		return []Hunk{{pre, sa, pre, sb}}
	}
	// ops: true = common line, false = edit. Walk it to build hunks.
	var hunks []Hunk
	ai, bi := 0, 0
	var cur *Hunk
	flush := func() {
		if cur != nil {
			hunks = append(hunks, *cur)
			cur = nil
		}
	}
	for _, op := range ops {
		switch op {
		case opEqual:
			flush()
			ai++
			bi++
		case opDelete:
			if cur == nil {
				cur = &Hunk{AStart: pre + ai, BStart: pre + bi, AEnd: pre + ai, BEnd: pre + bi}
			}
			ai++
			cur.AEnd = pre + ai
		case opInsert:
			if cur == nil {
				cur = &Hunk{AStart: pre + ai, BStart: pre + bi, AEnd: pre + ai, BEnd: pre + bi}
			}
			bi++
			cur.BEnd = pre + bi
		}
	}
	flush()
	return hunks
}

type op byte

const (
	opEqual op = iota
	opDelete
	opInsert
)

type pair struct{ a, b []int }

// intern maps lines to small integers so comparisons are cheap.
func intern(a, b []string) pair {
	ids := make(map[string]int, len(a)+len(b))
	conv := func(in []string) []int {
		out := make([]int, len(in))
		for i, s := range in {
			id, ok := ids[s]
			if !ok {
				id = len(ids)
				ids[s] = id
			}
			out[i] = id
		}
		return out
	}
	return pair{conv(a), conv(b)}
}

// myers computes an edit script with the classic O(ND) algorithm. It reports
// ok=false when the edit distance exceeds MaxEdits.
func myers(p pair) ([]op, bool) {
	a, b := p.a, p.b
	n, m := len(a), len(b)
	max := n + m
	if max > MaxEdits {
		max = MaxEdits
	}
	offset := max + 1
	v := make([]int, 2*max+3)
	trace := make([][]int, 0, 16) // trace[d] = frontier after round d, diagonals -d..d
	for d := 0; d <= max; d++ {
		for k := -d; k <= d; k += 2 {
			var x int
			if k == -d || (k != d && v[offset+k-1] < v[offset+k+1]) {
				x = v[offset+k+1]
			} else {
				x = v[offset+k-1] + 1
			}
			y := x - k
			for x < n && y < m && a[x] == b[y] {
				x++
				y++
			}
			v[offset+k] = x
			if x >= n && y >= m {
				return backtrack(trace, n, m, d), true
			}
		}
		snap := make([]int, 2*d+1)
		for k := -d; k <= d; k += 2 {
			snap[k+d] = v[offset+k]
		}
		trace = append(trace, snap)
	}
	return nil, false
}

// backtrack rebuilds the edit script from the saved frontiers.
func backtrack(trace [][]int, n, m, dEnd int) []op {
	x, y := n, m
	var rev []op
	for d := dEnd; d > 0; d-- {
		prev := trace[d-1]
		get := func(k int) int { return prev[k+d-1] }
		k := x - y
		down := k == -d || (k != d && get(k-1) < get(k+1))
		prevK := k - 1
		if down {
			prevK = k + 1
		}
		prevX := get(prevK)
		prevY := prevX - prevK
		midX, midY := prevX+1, prevY
		if down {
			midX, midY = prevX, prevY+1
		}
		for x > midX && y > midY {
			rev = append(rev, opEqual)
			x--
			y--
		}
		if down {
			rev = append(rev, opInsert)
		} else {
			rev = append(rev, opDelete)
		}
		x, y = prevX, prevY
	}
	for x > 0 && y > 0 {
		rev = append(rev, opEqual)
		x--
		y--
	}
	for i, j := 0, len(rev)-1; i < j; i, j = i+1, j-1 {
		rev[i], rev[j] = rev[j], rev[i]
	}
	return rev
}

// Apply returns a with the hunks replaced by the corresponding lines of b.
// It is the inverse of Diff and is used by tests and by the advisor to build
// per-side texts.
func Apply(a, b []string, hunks []Hunk) []string {
	var out []string
	pos := 0
	for _, h := range hunks {
		out = append(out, a[pos:h.AStart]...)
		out = append(out, b[h.BStart:h.BEnd]...)
		pos = h.AEnd
	}
	return append(out, a[pos:]...)
}
