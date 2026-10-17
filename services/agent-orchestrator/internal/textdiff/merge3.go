package textdiff

import "sort"

// Kind says how one chunk of a three-way comparison resolves.
type Kind int

const (
	// Stable: neither side touched these base lines.
	Stable Kind = iota
	// OursOnly: only our side changed the chunk.
	OursOnly
	// TheirsOnly: only their side changed the chunk.
	TheirsOnly
	// Same: both sides changed it to identical text.
	Same
	// Conflict: both sides changed it, differently.
	Conflict
)

func (k Kind) String() string {
	return [...]string{"stable", "ours_only", "theirs_only", "same", "conflict"}[k]
}

// Chunk is one cluster of a three-way comparison. For Stable chunks Ours and
// Theirs equal Base.
type Chunk struct {
	Kind               Kind
	BaseStart, BaseEnd int // base[BaseStart:BaseEnd]
	Base               []string
	Ours               []string
	Theirs             []string
	// OursHunks and TheirsHunks are the individual edits (in base line numbers)
	// that make up the cluster; they let callers tell adjacent edits from
	// overlapping ones.
	OursHunks   []Hunk
	TheirsHunks []Hunk
}

type sideHunk struct {
	theirs bool
	h      Hunk
}

// Merge3 compares ours and theirs against their common base. Hunks of the two
// sides that overlap or touch are grouped into one cluster, exactly like the
// server's merge does, so a Conflict chunk here corresponds to a conflict
// region there.
func Merge3(base, ours, theirs []string) []Chunk {
	ho := Diff(base, ours)
	ht := Diff(base, theirs)
	all := make([]sideHunk, 0, len(ho)+len(ht))
	for _, h := range ho {
		all = append(all, sideHunk{false, h})
	}
	for _, h := range ht {
		all = append(all, sideHunk{true, h})
	}
	sort.SliceStable(all, func(i, j int) bool {
		a, b := all[i], all[j]
		if a.h.AStart != b.h.AStart {
			return a.h.AStart < b.h.AStart
		}
		if a.h.AEnd != b.h.AEnd {
			return a.h.AEnd < b.h.AEnd
		}
		return !a.theirs && b.theirs
	})

	var chunks []Chunk
	pos := 0
	for i := 0; i < len(all); {
		lo, hi := all[i].h.AStart, all[i].h.AEnd
		j := i + 1
		for j < len(all) && all[j].h.AStart <= hi {
			if all[j].h.AEnd > hi {
				hi = all[j].h.AEnd
			}
			j++
		}
		if lo > pos {
			stable := base[pos:lo]
			chunks = append(chunks, Chunk{Kind: Stable, BaseStart: pos, BaseEnd: lo, Base: stable, Ours: stable, Theirs: stable})
		}
		c := Chunk{BaseStart: lo, BaseEnd: hi, Base: base[lo:hi]}
		for _, sh := range all[i:j] {
			if sh.theirs {
				c.TheirsHunks = append(c.TheirsHunks, sh.h)
			} else {
				c.OursHunks = append(c.OursHunks, sh.h)
			}
		}
		c.Ours = sideText(base, ours, c.OursHunks, lo, hi)
		c.Theirs = sideText(base, theirs, c.TheirsHunks, lo, hi)
		switch {
		case len(c.TheirsHunks) == 0:
			c.Kind = OursOnly
		case len(c.OursHunks) == 0:
			c.Kind = TheirsOnly
		case equalLines(c.Ours, c.Theirs):
			c.Kind = Same
		default:
			c.Kind = Conflict
		}
		chunks = append(chunks, c)
		pos = hi
		i = j
	}
	if pos < len(base) {
		stable := base[pos:]
		chunks = append(chunks, Chunk{Kind: Stable, BaseStart: pos, BaseEnd: len(base), Base: stable, Ours: stable, Theirs: stable})
	}
	return chunks
}

// sideText is base[lo:hi] with one side's hunks applied.
func sideText(base, side []string, hunks []Hunk, lo, hi int) []string {
	var out []string
	pos := lo
	for _, h := range hunks {
		out = append(out, base[pos:h.AStart]...)
		out = append(out, side[h.BStart:h.BEnd]...)
		pos = h.AEnd
	}
	return append(out, base[pos:hi]...)
}

func equalLines(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// Resolver picks the lines for a Conflict chunk; ok=false leaves it unresolved.
type Resolver func(c Chunk) (lines []string, ok bool)

// Resolve rebuilds the merged file. Every Conflict chunk is passed to pick;
// the returned bool reports whether all of them were resolved. Unresolved
// conflicts are rendered with the server's marker layout so the output is
// still a usable (if conflicted) file.
func Resolve(chunks []Chunk, pick Resolver) (merged []string, allResolved bool) {
	allResolved = true
	for _, c := range chunks {
		switch c.Kind {
		case Stable, OursOnly, Same:
			merged = append(merged, c.Ours...)
		case TheirsOnly:
			merged = append(merged, c.Theirs...)
		case Conflict:
			if lines, ok := pick(c); ok {
				merged = append(merged, lines...)
				continue
			}
			allResolved = false
			merged = append(merged, "<<<<<<< ours\n")
			merged = append(merged, ensureNL(c.Ours)...)
			merged = append(merged, "=======\n")
			merged = append(merged, ensureNL(c.Theirs)...)
			merged = append(merged, ">>>>>>> theirs\n")
		}
	}
	return merged, allResolved
}

func ensureNL(lines []string) []string {
	if n := len(lines); n > 0 && lines[n-1] != "" && lines[n-1][len(lines[n-1])-1] != '\n' {
		out := append([]string(nil), lines...)
		out[n-1] += "\n"
		return out
	}
	return lines
}
