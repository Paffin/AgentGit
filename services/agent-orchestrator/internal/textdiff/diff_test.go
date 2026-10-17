package textdiff

import (
	"math/rand"
	"reflect"
	"strings"
	"testing"
)

func lines(s string) []string { return SplitLines(s) }

func TestSplitLinesRoundTrip(t *testing.T) {
	for _, s := range []string{"", "a", "a\n", "a\nb", "a\n\nb\n", "\n", "no newline at end"} {
		if got := Join(SplitLines(s)); got != s {
			t.Errorf("round trip of %q gave %q", s, got)
		}
	}
	if got := SplitLines("a\nb"); !reflect.DeepEqual(got, []string{"a\n", "b"}) {
		t.Errorf("SplitLines = %q", got)
	}
}

func TestDiffTable(t *testing.T) {
	tests := []struct {
		name string
		a, b string
		want []Hunk
	}{
		{"equal", "a\nb\n", "a\nb\n", nil},
		{"insert middle", "a\nc\n", "a\nb\nc\n", []Hunk{{1, 1, 1, 2}}},
		{"delete middle", "a\nb\nc\n", "a\nc\n", []Hunk{{1, 2, 1, 1}}},
		{"replace", "a\nb\nc\n", "a\nX\nc\n", []Hunk{{1, 2, 1, 2}}},
		{"append", "a\n", "a\nb\n", []Hunk{{1, 1, 1, 2}}},
		{"prepend", "b\n", "a\nb\n", []Hunk{{0, 0, 0, 1}}},
		{"empty to something", "", "a\n", []Hunk{{0, 0, 0, 1}}},
		{"something to empty", "a\n", "", []Hunk{{0, 1, 0, 0}}},
		{"two separate edits", "1\n2\n3\n4\n5\n", "1\nX\n3\nY\n5\n", []Hunk{{1, 2, 1, 2}, {3, 4, 3, 4}}},
	}
	for _, tt := range tests {
		got := Diff(lines(tt.a), lines(tt.b))
		if !reflect.DeepEqual(got, tt.want) {
			t.Errorf("%s: Diff = %+v, want %+v", tt.name, got, tt.want)
		}
	}
}

// The diff must always be a correct edit script, whatever the input.
func TestDiffAppliesBackProperty(t *testing.T) {
	rng := rand.New(rand.NewSource(7))
	alphabet := []string{"a\n", "b\n", "c\n", "d\n", "e\n", "\n", "f\n"}
	gen := func(n int) []string {
		out := make([]string, n)
		for i := range out {
			out[i] = alphabet[rng.Intn(len(alphabet))]
		}
		return out
	}
	for i := 0; i < 400; i++ {
		a, b := gen(rng.Intn(30)), gen(rng.Intn(30))
		hunks := Diff(a, b)
		if got := Apply(a, b, hunks); !reflect.DeepEqual(got, b) && (len(got) != 0 || len(b) != 0) {
			t.Fatalf("Apply(Diff) mismatch\na=%q\nb=%q\nhunks=%+v\ngot=%q", a, b, hunks, got)
		}
		prevEnd := -1
		for _, h := range hunks {
			if h.AStart < prevEnd || h.AStart > h.AEnd || h.BStart > h.BEnd {
				t.Fatalf("malformed hunks %+v", hunks)
			}
			prevEnd = h.AEnd
		}
	}
}

func TestDiffIsMinimalOnSimpleShift(t *testing.T) {
	// Moving one line to the top should be one deletion and one insertion, not a rewrite.
	a := lines("1\n2\n3\n4\n5\n6\n")
	b := lines("6\n1\n2\n3\n4\n5\n")
	hunks := Diff(a, b)
	changed := 0
	for _, h := range hunks {
		changed += (h.AEnd - h.AStart) + (h.BEnd - h.BStart)
	}
	if changed != 2 {
		t.Fatalf("expected 2 changed lines, got %d (%+v)", changed, hunks)
	}
}

func TestDiffFallsBackOnHugeEditDistance(t *testing.T) {
	var a, b []string
	for i := 0; i < MaxEdits+200; i++ {
		a = append(a, "a"+string(rune('A'+i%26))+strings.Repeat("x", i%7)+"\n")
		b = append(b, "b"+string(rune('A'+i%26))+strings.Repeat("y", i%5)+"\n")
	}
	hunks := Diff(a, b)
	if len(hunks) != 1 || hunks[0].AStart != 0 || hunks[0].AEnd != len(a) {
		t.Fatalf("expected a single whole-file hunk, got %+v", hunks)
	}
	if got := Apply(a, b, hunks); !reflect.DeepEqual(got, b) {
		t.Fatal("fallback hunk does not reproduce b")
	}
}

func TestDiffDeterministic(t *testing.T) {
	a := lines("a\nb\nc\nb\nc\nd\n")
	b := lines("b\nc\nd\nb\nc\n")
	first := Diff(a, b)
	for i := 0; i < 20; i++ {
		if !reflect.DeepEqual(Diff(a, b), first) {
			t.Fatal("Diff is not deterministic")
		}
	}
}

func kinds(cs []Chunk) []Kind {
	var out []Kind
	for _, c := range cs {
		out = append(out, c.Kind)
	}
	return out
}

func TestMerge3Classification(t *testing.T) {
	tests := []struct {
		name               string
		base, ours, theirs string
		want               []Kind
	}{
		{"disjoint edits", "1\n2\n3\n4\n5\n", "1\nO\n3\n4\n5\n", "1\n2\n3\nT\n5\n",
			[]Kind{Stable, OursOnly, Stable, TheirsOnly, Stable}},
		{"same edit both sides", "1\n2\n3\n", "1\nX\n3\n", "1\nX\n3\n", []Kind{Stable, Same, Stable}},
		{"same line edited differently", "1\n2\n3\n", "1\nO\n3\n", "1\nT\n3\n", []Kind{Stable, Conflict, Stable}},
		{"adjacent lines touch", "1\n2\n3\n4\n", "1\nO\n3\n4\n", "1\n2\nT\n4\n", []Kind{Stable, Conflict, Stable}},
		{"one line apart is fine", "1\n2\n3\n4\n5\n", "1\nO\n3\n4\n5\n", "1\n2\n3\nT\n5\n", []Kind{Stable, OursOnly, Stable, TheirsOnly, Stable}},
		{"both insert at the same spot", "1\n2\n", "1\nA\n2\n", "1\nB\n2\n", []Kind{Stable, Conflict, Stable}},
		{"ours unchanged", "1\n2\n", "1\n2\n", "1\nT\n", []Kind{Stable, TheirsOnly}},
	}
	for _, tt := range tests {
		got := kinds(Merge3(lines(tt.base), lines(tt.ours), lines(tt.theirs)))
		if !reflect.DeepEqual(got, tt.want) {
			t.Errorf("%s: kinds = %v, want %v", tt.name, got, tt.want)
		}
	}
}

func TestMerge3ChunkContents(t *testing.T) {
	cs := Merge3(lines("1\n2\n3\n"), lines("1\nO\n3\n"), lines("1\nT\n3\n"))
	var c Chunk
	for _, x := range cs {
		if x.Kind == Conflict {
			c = x
		}
	}
	if Join(c.Base) != "2\n" || Join(c.Ours) != "O\n" || Join(c.Theirs) != "T\n" {
		t.Fatalf("chunk = %+v", c)
	}
	if c.BaseStart != 1 || c.BaseEnd != 2 || len(c.OursHunks) != 1 || len(c.TheirsHunks) != 1 {
		t.Fatalf("chunk bounds = %+v", c)
	}
}

func TestResolveAndMarkers(t *testing.T) {
	base, ours, theirs := lines("1\n2\n3\n"), lines("1\nO\n3\n"), lines("1\nT\n3\n")
	cs := Merge3(base, ours, theirs)
	merged, ok := Resolve(cs, func(Chunk) ([]string, bool) { return nil, false })
	if ok {
		t.Fatal("unresolved conflict reported as resolved")
	}
	if got := Join(merged); got != "1\n<<<<<<< ours\nO\n=======\nT\n>>>>>>> theirs\n3\n" {
		t.Fatalf("markers = %q", got)
	}
	merged, ok = Resolve(cs, func(c Chunk) ([]string, bool) { return append(append([]string{}, c.Ours...), c.Theirs...), true })
	if !ok || Join(merged) != "1\nO\nT\n3\n" {
		t.Fatalf("resolved = %q ok=%v", Join(merged), ok)
	}
}

func TestResolveMarkersAddMissingNewline(t *testing.T) {
	cs := Merge3(lines("a\n"), lines("O"), lines("T"))
	merged, _ := Resolve(cs, func(Chunk) ([]string, bool) { return nil, false })
	if got := Join(merged); got != "<<<<<<< ours\nO\n=======\nT\n>>>>>>> theirs\n" {
		t.Fatalf("markers = %q", got)
	}
}

// Without conflicts, Merge3 + Resolve is a real three-way merge: applying both
// sides' edits to the base must contain each side's changed lines.
func TestMerge3CleanMergeProperty(t *testing.T) {
	rng := rand.New(rand.NewSource(11))
	for i := 0; i < 300; i++ {
		n := 5 + rng.Intn(25)
		base := make([]string, n)
		for j := range base {
			base[j] = string(rune('a'+j%26)) + "\n"
		}
		edit := func(lo, hi int) []string {
			out := append([]string(nil), base...)
			p := lo + rng.Intn(hi-lo)
			out[p] = "EDIT" + string(rune('0'+rng.Intn(10))) + "\n"
			return out
		}
		ours, theirs := edit(0, n/2), edit(n/2+1, n)
		merged, ok := Resolve(Merge3(base, ours, theirs), func(Chunk) ([]string, bool) { return nil, false })
		if !ok {
			t.Fatalf("far-apart single edits must merge cleanly (n=%d)", n)
		}
		for _, side := range [][]string{ours, theirs} {
			for _, l := range side {
				if strings.HasPrefix(l, "EDIT") && !strings.Contains(Join(merged), l) {
					t.Fatalf("merged result lost edit %q", l)
				}
			}
		}
	}
}
