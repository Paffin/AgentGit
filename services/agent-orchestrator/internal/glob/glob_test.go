package glob

import "testing"

func TestMatch(t *testing.T) {
	tests := []struct {
		pattern string
		path    string
		want    bool
	}{
		{"src/**", "src/a.rs", true},
		{"src/**", "src/a/b/c.rs", true},
		{"src/**", "src", false},
		{"src/**", "other/a.rs", false},
		{"**", "anything/at/all", true},
		{"**/*.rs", "lib.rs", true},
		{"**/*.rs", "src/deep/lib.rs", true},
		{"**/*.rs", "src/lib.go", false},
		{"*.md", "README.md", true},
		{"*.md", "docs/README.md", false},
		{"docs/*.md", "docs/a.md", true},
		{"docs/*.md", "docs/sub/a.md", false},
		{"a/**/b", "a/b", true},
		{"a/**/b", "a/x/b", true},
		{"a/**/b", "a/x/y/b", true},
		{"a/**/b", "a/x/y/c", false},
		{"src/?.rs", "src/a.rs", true},
		{"src/?.rs", "src/ab.rs", false},
		{"src/?.rs", "src//.rs", false},
		{"file[0-9].txt", "file7.txt", true},
		{"file[0-9].txt", "filex.txt", false},
		{"file[!0-9].txt", "filex.txt", true},
		{"file[^0-9].txt", "file7.txt", false},
		{"file[!0-9].txt", "file/.txt", false},
		{"*.{rs,go}", "main.go", true},
		{"*.{rs,go}", "main.rs", true},
		{"*.{rs,go}", "main.py", false},
		{"{src,tests}/**/*.rs", "tests/a/b.rs", true},
		{"{src,tests}/**/*.rs", "docs/a/b.rs", false},
		{`weird\*name`, "weird*name", true},
		{`weird\*name`, "weirdXname", false},
		{"a.b", "a.b", true},
		{"a.b", "axb", false},
		{".github/**", ".github/workflows/ci.yml", true},
		{"src/**", "./src/a", true},
	}
	for _, tt := range tests {
		g, err := Compile(tt.pattern)
		if err != nil {
			t.Fatalf("Compile(%q): %v", tt.pattern, err)
		}
		if got := g.Match(tt.path); got != tt.want {
			t.Errorf("Compile(%q).Match(%q) = %v, want %v", tt.pattern, tt.path, got, tt.want)
		}
	}
}

func TestCompileErrors(t *testing.T) {
	for _, p := range []string{"", "a[b", "a{b", `a\`, "{a,b}{c,d}{e,f}{g,h}{i,j}{k,l}{m,n}"} {
		if _, err := Compile(p); err == nil {
			t.Errorf("Compile(%q) should fail", p)
		}
	}
}

func TestNestedBraces(t *testing.T) {
	g := MustCompile("{a,{b,c}}/x")
	for _, p := range []string{"a/x", "b/x", "c/x"} {
		if !g.Match(p) {
			t.Errorf("%q should match", p)
		}
	}
	if g.Match("d/x") {
		t.Error("d/x should not match")
	}
}

func TestSetMatchAndFirst(t *testing.T) {
	s, err := CompileAll([]string{"docs/**", "*.lock"})
	if err != nil {
		t.Fatal(err)
	}
	if !s.Match("docs/a.md") || !s.Match("Cargo.lock") || s.Match("src/a.rs") {
		t.Fatal("unexpected Set.Match results")
	}
	if got := s.First("Cargo.lock"); got != "*.lock" {
		t.Fatalf("First = %q", got)
	}
	if got := s.First("zzz"); got != "" {
		t.Fatalf("First = %q", got)
	}
	if _, err := CompileAll([]string{"ok", "bad["}); err == nil {
		t.Fatal("expected error")
	}
}

func TestOverlapMirrorsServerRule(t *testing.T) {
	tests := []struct {
		a, b string
		want bool
	}{
		{"src/**", "src/lib.rs", true},
		{"src/a/**", "src/**", true},
		{"**/*.rs", "src/lib.rs", true},
		{"src/**", "docs/**", false},
		{"a/b/**", "a/c/**", false},
		{"src/auth/**", "src/auth/login.rs", true},
		{"README.md", "README.md", true},
		{"README.md", "docs/**", false},
	}
	for _, tt := range tests {
		if got := MayOverlap(tt.a, tt.b); got != tt.want {
			t.Errorf("MayOverlap(%q,%q) = %v, want %v", tt.a, tt.b, got, tt.want)
		}
		if got := MayOverlap(tt.b, tt.a); got != tt.want {
			t.Errorf("MayOverlap(%q,%q) = %v, want %v (symmetry)", tt.b, tt.a, got, tt.want)
		}
	}
	if !AnyOverlap([]string{"x/**", "y/**"}, []string{"z/**", "y/a"}) {
		t.Error("AnyOverlap should find y")
	}
	if AnyOverlap([]string{"x/**"}, []string{"y/**"}) || AnyOverlap(nil, []string{"y/**"}) {
		t.Error("AnyOverlap false positive")
	}
}

func TestLiteralPrefix(t *testing.T) {
	for in, want := range map[string]string{"src/**": "src/", "a/b.rs": "a/b.rs", "**/x": "", "a/[bc]/d": "a/", "a/{b,c}": "a/"} {
		if got := LiteralPrefix(in); got != want {
			t.Errorf("LiteralPrefix(%q) = %q, want %q", in, got, want)
		}
	}
}
