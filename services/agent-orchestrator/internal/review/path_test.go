package review

import "testing"

// Models name files loosely (a basename, a diff prefix). A finding keeps its location when the path identifies
// exactly one file of the change, and loses it, with a note, when it does not.
func TestAModelNamingAFileLooselyStillLocatesTheFinding(t *testing.T) {
	known := map[string]bool{"store/users.go": true, "web/index.ts": true, "web/util/index.ts": true}
	for _, c := range []struct {
		in, want string
		ok       bool
	}{
		{"store/users.go", "store/users.go", true},
		{"users.go", "store/users.go", true},
		{"b/store/users.go", "store/users.go", true},
		{"./store/users.go", "store/users.go", true},
		{"", "", true},
		{"index.ts", "index.ts", false}, // two files end in it: ambiguous
		{"util/index.ts", "web/util/index.ts", true},
		{"other.go", "other.go", false},
		{"ers.go", "ers.go", false}, // a suffix of a name is not a path component
	} {
		got, ok := resolvePath(known, c.in)
		if got != c.want || ok != c.ok {
			t.Errorf("resolvePath(%q) = %q, %v; want %q, %v", c.in, got, ok, c.want, c.ok)
		}
	}
}
